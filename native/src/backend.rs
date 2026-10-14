use crate::platform::{self, OwnedHandle, wide};
#[path = "profile_store.rs"]
mod profile_store;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::windows::io::AsRawHandle,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::Authorization::*,
    Security::*,
    Storage::FileSystem::*,
    System::{
        IO::*, JobObjects::*, Pipes::*, Services::*, Threading::*, Time::FileTimeToSystemTime,
    },
};
// A 512 KiB profile can expand sixfold when JSON escapes control characters.
const MAX: usize = 4 * 1024 * 1024;
const MAX_BRIDGE_FRAME: usize = MAX;
const RPC_TIMEOUT: Duration = Duration::from_secs(100);
const ACTIONS: [&str; 12] = [
    "Import",
    "Select",
    "Rename",
    "Remove",
    "Connect",
    "Disconnect",
    "Exit",
    "Autoconnect",
    "ApplySettings",
    "ValidateSettings",
    "Update",
    "Diagnose",
];
const CANCELLABLE_ACTIONS: [&str; 4] = ["Connect", "Select", "Update", "UpdateDns"];

// Every mutable access invalidates the token, including error/recovery updates.
// Unchanged polls never clone or serialize the potentially multi-MiB snapshot.
struct Snapshot {
    value: Value,
    epoch: String,
    revision: u64,
}
impl Snapshot {
    fn new(value: Value) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            value,
            epoch: format!(
                "{:x}-{:x}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            revision: 0,
        }
    }
    fn token(&self) -> String {
        format!("{}-{:x}", self.epoch, self.revision)
    }
    fn response(&self, request: &Value) -> Value {
        let revision = self.token();
        if request["if_revision"].as_str() == Some(revision.as_str()) {
            json!({"ok":true,"not_modified":true,"revision":revision})
        } else {
            json!({"ok":true,"revision":revision,"data":self.value})
        }
    }
    fn replace(&mut self, value: Value) {
        self.revision = self.revision.wrapping_add(1);
        self.value = value;
    }
}
impl std::ops::Deref for Snapshot {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.value
    }
}
impl std::ops::DerefMut for Snapshot {
    fn deref_mut(&mut self) -> &mut Value {
        self.revision = self.revision.wrapping_add(1);
        &mut self.value
    }
}
type SharedSnapshot = Arc<Mutex<Snapshot>>;

fn bundled_core_path(base: &Path) -> PathBuf {
    let bin = base.join("bin");
    // The bridge verifies the manifest and executable hash before launching it.
    // A broken experimental package fails closed instead of selecting stock core.
    if bin.join("mycelium-build.json").is_file() {
        bin.join("mihomo-mycelium.exe")
    } else {
        bin.join("mihomo-windows-amd64-compatible.exe")
    }
}

fn format_creation_time(time: &SYSTEMTIME, ticks: u64) -> String {
    // .NET's UTC round-trip format preserves all seven 100 ns fraction digits.
    // SYSTEMTIME alone rounds that fraction to milliseconds.
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:07}Z",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        ticks % 10_000_000,
    )
}

fn creation_time_utc(creation: &FILETIME) -> Option<String> {
    let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { FileTimeToSystemTime(creation, &mut time) } == 0 || time.wYear > 9999 {
        return None;
    }
    let ticks = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    Some(format_creation_time(&time, ticks))
}

fn process_creation_time(handle: HANDLE) -> Option<String> {
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return None;
    }
    creation_time_utc(&creation)
}

fn read_bridge_frame(reader: impl Read) -> Result<Vec<u8>, String> {
    // A persistent engine may inherit an otherwise unused stdout writer.
    // EOF therefore describes descendant lifetime, not helper completion.
    // NativeBridge emits exactly one compact UTF-8 JSON record plus LF.
    let mut bytes = Vec::new();
    BufReader::new(reader)
        .take((MAX_BRIDGE_FRAME + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BRIDGE_FRAME {
        return Err("Bridge response is too large".into());
    }
    if bytes.pop() != Some(b'\n') {
        return Err("Фоновая операция не вернула корректный ответ".into());
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(bytes)
}

fn decode_bridge_reply(bytes: &[u8], helper_succeeded: bool) -> Result<Value, String> {
    let invalid = || "Фоновая операция не вернула корректный ответ".to_owned();
    let reply: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    match reply["ok"].as_bool() {
        Some(true) if helper_succeeded && reply["data"].is_object() => Ok(reply["data"].clone()),
        Some(false) => Err(reply["error"].as_str().ok_or_else(invalid)?.to_owned()),
        _ => Err(invalid()),
    }
}

fn installed_service_status(
    root: &Path,
    executable: &Path,
) -> Result<SERVICE_STATUS_PROCESS, String> {
    struct ServiceHandle(SC_HANDLE);
    impl Drop for ServiceHandle {
        fn drop(&mut self) {
            unsafe {
                CloseServiceHandle(self.0);
            }
        }
    }
    unsafe {
        let manager = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err("Cannot query Windows services".into());
        }
        let manager = ServiceHandle(manager);
        let service = OpenServiceW(
            manager.0,
            wide("Mukhomor").as_ptr(),
            SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG,
        );
        if service.is_null() {
            return Err("Не удалось проверить установленный контроллер Mukhomor".into());
        }
        let service = ServiceHandle(service);
        let mut needed = 0;
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
        if needed == 0 || needed > 32768 {
            return Err("Не удалось прочитать сведения о контроллере Mukhomor".into());
        }
        let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        if QueryServiceConfigW(service.0, config, needed, &mut needed) == 0
            || (*config).lpBinaryPathName.is_null()
        {
            return Err("Не удалось прочитать сведения о контроллере Mukhomor".into());
        }
        let binary = (*config).lpBinaryPathName;
        let mut count = 0;
        while count < 32768 && *binary.add(count) != 0 {
            count += 1;
        }
        let actual = String::from_utf16_lossy(std::slice::from_raw_parts(binary, count));
        let expected = format!(
            "\"{}\" --service --root \"{}\"",
            executable.display(),
            root.display()
        );
        if (*config).dwServiceType != SERVICE_WIN32_OWN_PROCESS
            || actual.to_lowercase() != expected.to_lowercase()
        {
            return Err("Контроллер не принадлежит этой установке Mukhomor".into());
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut size = 0;
        let ok = QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            &mut status as *mut _ as *mut u8,
            std::mem::size_of_val(&status) as u32,
            &mut size,
        );
        if ok == 0 {
            return Err("Не удалось проверить состояние контроллера Mukhomor".into());
        }
        Ok(status)
    }
}
fn pipe_name(root: &Path) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for b in root.to_string_lossy().to_lowercase().bytes() {
        hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("\\\\.\\pipe\\Mukhomor-{hash:016x}")
}
fn transfer(
    handle: HANDLE,
    bytes: *mut u8,
    length: u32,
    write: bool,
    deadline: Instant,
) -> Result<usize, String> {
    unsafe {
        let event = OwnedHandle(CreateEventW(null(), 1, 0, null()));
        if event.0.is_null() {
            return Err("Не удалось создать ожидание IPC".into());
        }
        let mut operation: OVERLAPPED = std::mem::zeroed();
        operation.hEvent = event.0;
        let mut transferred = 0;
        let ok = if write {
            WriteFile(handle, bytes, length, &mut transferred, &mut operation)
        } else {
            ReadFile(handle, bytes, length, &mut transferred, &mut operation)
        };
        if ok == 0 && GetLastError() != ERROR_IO_PENDING {
            return Err("Соединение с Mukhomor прервано".into());
        }
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u32::MAX as u128) as u32;
        if WaitForSingleObject(event.0, remaining) != WAIT_OBJECT_0 {
            CancelIoEx(handle, &operation);
            GetOverlappedResult(handle, &operation, &mut transferred, 1);
            return Err(
                "Mukhomor не ответил вовремя. Состояние подключения проверяется отдельно.".into(),
            );
        }
        if GetOverlappedResult(handle, &operation, &mut transferred, 0) == 0 || transferred == 0 {
            return Err("Соединение с Mukhomor прервано".into());
        }
        Ok(transferred as usize)
    }
}
fn write_bytes(handle: HANDLE, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        offset += transfer(
            handle,
            bytes[offset..].as_ptr() as *mut u8,
            (bytes.len() - offset) as u32,
            true,
            deadline,
        )?;
    }
    Ok(())
}
fn read_bytes(handle: HANDLE, bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        offset += transfer(
            handle,
            bytes[offset..].as_mut_ptr(),
            (bytes.len() - offset) as u32,
            false,
            deadline,
        )?;
    }
    Ok(())
}
fn send(handle: HANDLE, value: &Value, deadline: Instant) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX {
        return Err("IPC response too large".into());
    }
    write_bytes(handle, &(bytes.len() as u32).to_le_bytes(), deadline)?;
    write_bytes(handle, &bytes, deadline)
}
fn receive(handle: HANDLE, deadline: Instant) -> Result<Value, String> {
    let mut length = [0u8; 4];
    read_bytes(handle, &mut length, deadline)?;
    let n = u32::from_le_bytes(length) as usize;
    if n > MAX {
        return Err("IPC request too large".into());
    }
    let mut bytes = vec![0; n];
    read_bytes(handle, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid IPC message".into())
}
pub fn rpc(root: &Path, request: &Value) -> Result<Value, String> {
    let deadline = Instant::now()
        + if request["action"] == "Status" {
            Duration::from_secs(5)
        } else {
            RPC_TIMEOUT
        };
    unsafe {
        let name = wide(&pipe_name(root));
        WaitNamedPipeW(name.as_ptr(), 3000);
        let handle = OwnedHandle(CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION | FILE_FLAG_OVERLAPPED,
            null_mut(),
        ));
        if handle.0 == INVALID_HANDLE_VALUE {
            return Err("Mukhomor ещё запускается. Подожди несколько секунд.".into());
        }
        let mut pid = 0;
        if GetNamedPipeServerProcessId(handle.0, &mut pid) == 0 {
            return Err("Cannot verify the local service".into());
        }
        if root.join("qa.marker").exists() {
            // Isolated workers are ordinary same-user processes, so verify
            // their image directly without depending on a real SCM service.
            let process = OwnedHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid));
            let mut image = vec![0u16; 32768];
            let mut len = image.len() as u32;
            if process.0.is_null()
                || QueryFullProcessImageNameW(process.0, 0, image.as_mut_ptr(), &mut len) == 0
            {
                return Err("Cannot verify the local service identity".into());
            }
            let actual = PathBuf::from(String::from_utf16_lossy(&image[..len as usize]));
            let expected = std::env::current_exe().map_err(|e| e.to_string())?;
            if actual
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_lowercase()
                != expected
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .to_lowercase()
            {
                return Err(
                    "The IPC endpoint does not belong to the installed Mukhomor service".into(),
                );
            }
        } else {
            // Standard users cannot necessarily OpenProcess on a SYSTEM
            // controller. The kernel pipe PID plus SCM's exact protected
            // registration authenticates it without granting process access.
            let executable = crate::setup::installed(root)?;
            if installed_service_status(root, &executable)?.dwProcessId != pid {
                return Err("The local endpoint does not belong to the Windows service".into());
            }
        }
        send(handle.0, request, deadline)?;
        let result = receive(handle.0, deadline)?;
        // Explicit receipt replaces unbounded server FlushFileBuffers. Older
        // controllers simply discard this compatibility acknowledgement.
        let _ = write_bytes(handle.0, &[1], Instant::now() + Duration::from_secs(1));
        Ok(result)
    }
}
struct Request {
    value: Value,
    reply: mpsc::Sender<Value>,
    generation: Option<u64>,
    persistence_error: Option<String>,
    id: u64,
    request_id: String,
}
#[derive(Default)]
struct ConnectionIntent {
    generation: u64,
    desired: bool,
}
fn read_intent(root: &Path) -> Result<Option<ConnectionIntent>, String> {
    let path = root.join("runtime/connection-intent.json");
    let file =
        match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(
                "Не удалось прочитать состояние подключения; автоматическое подключение отменено"
                    .into(),
            ),
        };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "Не удалось прочитать состояние подключения")?;
    if bytes.len() > 4096 {
        return Err("Повреждено состояние подключения; автоматическое подключение отменено".into());
    }
    let data: Value = crate::traffic_guard_cli::parse(&bytes).map_err(|_| {
        "Повреждено состояние подключения; автоматическое подключение отменено".to_string()
    })?;
    if data["schema"] != 1 || data.as_object().is_none_or(|object| object.len() != 3) {
        return Err(
            "Неизвестный формат состояния подключения; автоматическое подключение отменено".into(),
        );
    }
    Ok(Some(ConnectionIntent {
        generation: data["generation"]
            .as_u64()
            .ok_or("Повреждено состояние подключения")?,
        desired: data["desired"]
            .as_bool()
            .ok_or("Повреждено состояние подключения")?,
    }))
}
fn save_intent(root: &Path, intent: &ConnectionIntent) -> Result<(), String> {
    let path = root.join("runtime/connection-intent.json");
    let pending = root.join("runtime/connection-intent.json.pending");
    let bytes = serde_json::to_vec(
        &json!({"schema":1,"generation":intent.generation,"desired":intent.desired}),
    )
    .map_err(|e| e.to_string())?;
    write_atomic_state(&path, &pending, &bytes).map_err(|_| {
        "Не удалось сохранить состояние подключения. Проверь доступ к данным Mukhomor.".to_string()
    })
}
fn write_atomic_state(path: &Path, pending: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let result = (|| {
        // The containing runtime directory is protected by the installer.
        // A stale temporary file is not connection state and can be discarded.
        let _ = std::fs::remove_file(&pending);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if unsafe {
            MoveFileExW(
                wide(&pending.to_string_lossy()).as_ptr(),
                wide(&path.to_string_lossy()).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&pending);
    result
}

fn startup_target(data: &Value) -> Option<&str> {
    let id = data
        .get("last_connected")
        .unwrap_or(&data["selected"])
        .as_str()?;
    data["profiles"]
        .as_array()?
        .iter()
        .any(|p| p["id"] == id)
        .then_some(id)
        .filter(|id| !id.is_empty())
}
fn remember_connection(
    root: &Path,
    data: &mut Value,
    current: &ConnectionIntent,
    generation: u64,
) -> Result<(), String> {
    if !current.desired || current.generation != generation || data["running"] != true {
        return Ok(());
    }
    let error =
        "Не удалось запомнить последний подключённый сервер. Проверь доступ к данным Mukhomor.";
    let id = data["selected"]
        .as_str()
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(error)?;
    let path = root.join("private/profiles.json");
    let bytes = std::fs::read(&path).map_err(|_| error)?;
    if bytes.len() > 524288 {
        return Err(error.into());
    }
    let mut index: Value = serde_json::from_slice(&bytes).map_err(|_| error)?;
    if index["schema"] != 1
        || index["selected"] != id
        || !index["profiles"]
            .as_array()
            .is_some_and(|profiles| profiles.iter().any(|p| p["id"] == id))
    {
        return Err(error.into());
    }
    if index["last_connected"] != id {
        index["last_connected"] = json!(id);
        let bytes = serde_json::to_vec(&index).map_err(|_| error)?;
        write_atomic_state(&path, &root.join("private/profiles.json.pending"), &bytes)
            .map_err(|_| error)?;
    }
    data["last_connected"] = json!(id);
    Ok(())
}
fn allow_login_connect(data: &Value, current: &ConnectionIntent, available: &mut bool) -> bool {
    let once = std::mem::replace(available, false);
    once && !current.desired
        && data["autoconnect"] == true
        && startup_target(data).is_some()
        && data["service_ready"] == true
        && data["busy"] != true
        && data["running"] != true
        && data["starting"] != true
        && data["phase"] == "idle"
        && data["status_uncertain"] != true
        && data["shutting_down"] != true
        && (data["last_error"].is_null() || data["last_error"].as_str() == Some(""))
}
fn persist_startup_intent(
    root: &Path,
    intent: &mut ConnectionIntent,
    initial: &mut Value,
    cleanup_only: bool,
) -> Result<(), String> {
    if let Err(error) = save_intent(root, intent) {
        intent.desired = false;
        if cleanup_only {
            // No listener/readiness or explicit guard release with stale disk
            // reconnect intent. The recovery service stops without SCM retry.
            return Err(error);
        }
        initial["last_error"] = json!(error);
    }
    Ok(())
}
fn finish_startup_intent(
    root: &Path,
    intent: &mut ConnectionIntent,
    initial: &mut Value,
    cleanup_only: bool,
    cancellation: Option<crate::setup::cleanup_request::Pending>,
) -> Result<(), String> {
    persist_startup_intent(
        root,
        intent,
        initial,
        cleanup_only || cancellation.is_some(),
    )?;
    if let Some(cancellation) = cancellation {
        // A crash before this point retains the durable request. A crash after
        // it leaves durable desired=false. Boot autoconnect never skips it.
        cancellation.consume()?;
    }
    Ok(())
}
fn permit_connect(root: &Path) -> Result<(), String> {
    match crate::setup::cleanup_request::pending(root)? {
        None => Ok(()),
        Some(_) => Err("CLEANUP_REQUEST_PENDING".into()),
    }
}
fn consume_cancellation(root: &Path) -> Result<(), String> {
    if let Some(cancellation) = crate::setup::cleanup_request::pending(root)? {
        cancellation.consume()?;
    }
    Ok(())
}
fn admit_connection_intent(
    root: &Path,
    intent: &mut ConnectionIntent,
    action: &str,
) -> Result<(), String> {
    if !["Connect", "Disconnect", "Exit"].contains(&action) {
        return Err("Invalid connection intent action".into());
    }
    intent.generation = intent.generation.wrapping_add(1);
    let cancel_error = if action == "Connect" {
        permit_connect(root).err()
    } else {
        None
    };
    intent.desired = action == "Connect" && cancel_error.is_none();
    if let Err(error) = save_intent(root, intent) {
        intent.desired = false;
        return Err(error);
    }
    if let Some(error) = cancel_error {
        return Err(error);
    }
    if ["Disconnect", "Exit"].contains(&action) {
        // Only the confirmed durable false intent can consume cancellation.
        // Read/delete errors leave it pending and prohibit terminal success.
        consume_cancellation(root)?;
    }
    Ok(())
}
fn decorate(data: &mut Value, intent: &ConnectionIntent, operation: &str) {
    // Verified WFP rules alone do not prove an active VPN. A stale or orphaned
    // adapter may remain after the owned core has stopped or during recovery.
    if data["traffic_guard"]["protected"] == true
        && (data["running"] != true || data["starting"] == true)
    {
        data["traffic_guard"]["protected"] = json!(false);
        data["traffic_guard"]["recovery_pending"] =
            json!(data["traffic_guard"]["configured"] == true);
    }
    data["service_ready"] = json!(true);
    data["operation"] = json!(operation);
    data["busy"] = json!(!operation.is_empty());
    data["phase"] = json!(
        if !intent.desired && ["Disconnect", "Exit"].contains(&operation) {
            "disconnecting"
        } else if (intent.desired && ["Connect", "Select"].contains(&operation))
            || data["starting"] == true
        {
            "connecting"
        } else if data["traffic_guard"]["configured"] == true
            && data["traffic_guard"]["protected"] != true
        {
            "recovery"
        } else if data["running"] == true {
            "connected"
        } else if data["dns_recovery_pending"] == true {
            "recovery"
        } else {
            "idle"
        }
    );
    data["can_disconnect"] = json!(
        data["running"] == true
            || data["starting"] == true
            || data["dns_recovery_pending"] == true
            || data["traffic_guard"]["configured"] == true
            || (intent.desired && ["Connect", "Select"].contains(&operation))
    );
}
fn listener(
    root: PathBuf,
    cache: SharedSnapshot,
    intent: Arc<Mutex<ConnectionIntent>>,
    tx: mpsc::Sender<Request>,
    signals: (isize, isize, isize),
    counter: Arc<AtomicU64>,
    login_available: Arc<AtomicBool>,
) -> Result<(), String> {
    let (_, stop, _) = signals;
    let owner = std::fs::read_to_string(root.join("owner.sid")).unwrap_or(platform::user_sid()?);
    if !owner.trim().starts_with("S-1-")
        || owner
            .trim()
            .chars()
            .any(|c| !c.is_ascii_digit() && c != 'S' && c != '-')
    {
        return Err("Invalid owner identity".into());
    }
    unsafe {
        let mut descriptor = null_mut();
        let sddl = wide(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;{})",
            owner.trim()
        ));
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err("Cannot secure local IPC".into());
        }
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // A pending Connect must never occupy the only IPC endpoint. Status and
        // Disconnect have their own available instances and remain responsive.
        let mut handles = Vec::new();
        for index in 0..4 {
            let handle = OwnedHandle(CreateNamedPipeW(
                wide(&pipe_name(&root)).as_ptr(),
                PIPE_ACCESS_DUPLEX
                    | FILE_FLAG_OVERLAPPED
                    | if index == 0 {
                        FILE_FLAG_FIRST_PIPE_INSTANCE
                    } else {
                        0
                    },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                4,
                65536,
                65536,
                5000,
                &security,
            ));
            if handle.0 == INVALID_HANDLE_VALUE {
                let error = GetLastError();
                LocalFree(descriptor);
                return Err(format!(
                    "Cannot create the secured IPC channel (instance {index}, Windows error {error})"
                ));
            }
            handles.push(handle);
        }
        LocalFree(descriptor);
        for handle in handles {
            let root = root.clone();
            let cache = cache.clone();
            let intent = intent.clone();
            let tx = tx.clone();
            let counter = counter.clone();
            let login_available = login_available.clone();
            thread::spawn(move || {
                if serve_client(
                    handle,
                    root,
                    cache,
                    intent,
                    tx,
                    signals,
                    counter,
                    login_available,
                )
                .is_err()
                {
                    platform::signal(stop);
                }
            });
        }
        WaitForSingleObject(stop as HANDLE, INFINITE);
    }
    Ok(())
}
fn accept(handle: HANDLE, stop: isize) -> Result<bool, String> {
    unsafe {
        loop {
            if platform::signaled(stop) {
                return Ok(false);
            }
            let event = OwnedHandle(CreateEventW(null(), 1, 0, null()));
            if event.0.is_null() {
                return Err("Cannot create IPC accept event".into());
            }
            let mut operation: OVERLAPPED = std::mem::zeroed();
            operation.hEvent = event.0;
            if ConnectNamedPipe(handle, &mut operation) != 0 {
                return Ok(true);
            }
            match GetLastError() {
                ERROR_PIPE_CONNECTED => return Ok(true),
                ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED => {
                    DisconnectNamedPipe(handle);
                    continue;
                }
                ERROR_IO_PENDING => {
                    let handles = [event.0, stop as HANDLE];
                    let outcome = WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
                    let mut count = 0;
                    if outcome != WAIT_OBJECT_0 {
                        CancelIoEx(handle, &operation);
                        GetOverlappedResult(handle, &operation, &mut count, 1);
                        return Ok(false);
                    }
                    if GetOverlappedResult(handle, &operation, &mut count, 0) != 0 {
                        return Ok(true);
                    }
                    DisconnectNamedPipe(handle);
                }
                _ => return Err("The local IPC listener stopped unexpectedly".into()),
            }
        }
    }
}
fn serve_client(
    handle: OwnedHandle,
    root: PathBuf,
    cache: SharedSnapshot,
    intent: Arc<Mutex<ConnectionIntent>>,
    tx: mpsc::Sender<Request>,
    signals: (isize, isize, isize),
    counter: Arc<AtomicU64>,
    login_available: Arc<AtomicBool>,
) -> Result<(), String> {
    let (wake, stop, cancel) = signals;
    unsafe {
        while !platform::signaled(stop) {
            if !accept(handle.0, stop)? {
                break;
            }
            if let Ok(mut value) = receive(handle.0, Instant::now() + Duration::from_secs(5)) {
                let exiting = value["action"] == "Exit";
                let response = if value["action"] == "Status" {
                    cache.lock().unwrap().response(&value)
                } else if value["action"] == "ShutdownWorker" && root.join("qa.marker").exists() {
                    platform::signal(stop);
                    json!({"ok":true})
                } else if !ACTIONS.contains(&value["action"].as_str().unwrap_or("")) {
                    json!({"ok":false,"error":"Неизвестная операция","data":cache.lock().unwrap().clone()})
                } else {
                    let action_name = value["action"].as_str().unwrap_or("").to_owned();
                    let action = action_name.as_str();
                    let request_id = value["request_id"]
                        .as_str()
                        .filter(|x| x.len() <= 96 && x.chars().all(|x| !x.is_control()))
                        .unwrap_or("")
                        .to_owned();
                    // Only cancellation may preempt an accepted mutation.
                    // Serializing admission bounds the queue and prevents a
                    // burst of repeated Connects from outliving the RPC limit.
                    let mut current = intent.lock().unwrap();
                    let mut data = cache.lock().unwrap();
                    if action == "Connect" && value["startup"] == true {
                        let mut available = login_available.swap(false, Ordering::Relaxed);
                        let allowed = allow_login_connect(&data, &current, &mut available)
                            && crate::setup::cleanup_request::pending(&root)
                                .is_ok_and(|pending| pending.is_none());
                        if !allowed {
                            let response =
                                json!({"ok":true,"request_id":request_id,"data":data.clone()});
                            drop(data);
                            drop(current);
                            let _ =
                                send(handle.0, &response, Instant::now() + Duration::from_secs(5));
                            let _ = read_bytes(
                                handle.0,
                                &mut [0],
                                Instant::now() + Duration::from_secs(2),
                            );
                            DisconnectNamedPipe(handle.0);
                            continue;
                        }
                        value["id"] = json!(startup_target(&data));
                    }
                    if data["shutting_down"] == true
                        || (data["busy"] == true && !["Disconnect", "Exit"].contains(&action))
                        || data["operation"] == "Exit"
                    {
                        let response = json!({"ok":false,"request_id":request_id,"error":"Дождись завершения текущей операции или отключи VPN.","data":data.clone()});
                        drop(data);
                        drop(current);
                        let _ = send(handle.0, &response, Instant::now() + Duration::from_secs(5));
                        let _ =
                            read_bytes(handle.0, &mut [0], Instant::now() + Duration::from_secs(2));
                        DisconnectNamedPipe(handle.0);
                        continue;
                    }
                    let id = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if ["Connect", "Disconnect", "Exit", "Select"].contains(&action) {
                        login_available.store(false, Ordering::Relaxed);
                    }
                    let mut generation = None;
                    let mut persistence_error = None;
                    if ["Connect", "Disconnect", "Exit", "Select", "Update"].contains(&action) {
                        if ["Connect", "Disconnect", "Exit"].contains(&action)
                            && let Err(error) = admit_connection_intent(&root, &mut current, action)
                        {
                            // An unsaved Connect is rejected. Disconnect
                            // still cleans up the tunnel and reports the
                            // persistence error instead of claiming success.
                            current.desired = false;
                            persistence_error = Some(error);
                        }
                        generation = Some(current.generation);
                        if ["Disconnect", "Exit"].contains(&action) {
                            platform::signal(cancel);
                            // Signal cancellation immediately, before waiting
                            // for the serialized privileged bridge operation.
                            if std::fs::write(root.join("runtime/stop.request"), b"disconnect")
                                .is_err()
                            {
                                persistence_error = Some("Не удалось записать сигнал отмены. Отключение выполняется; проверь доступ к данным Mukhomor.".into());
                            }
                        }
                    }
                    decorate(&mut data, &current, action);
                    data["operation_id"] = json!(id);
                    data["request_id"] = json!(&request_id);
                    drop(data);
                    drop(current);
                    let (reply, rx) = mpsc::channel();
                    let _ = tx.send(Request {
                        value,
                        reply,
                        generation,
                        persistence_error,
                        id,
                        request_id,
                    });
                    platform::signal(wake);
                    rx.recv_timeout(Duration::from_secs(95))
                        .unwrap_or(json!({"ok":false,"error":"Операция превысила время ожидания"}))
                };
                let _ = send(handle.0, &response, Instant::now() + Duration::from_secs(5));
                let _ = read_bytes(handle.0, &mut [0], Instant::now() + Duration::from_secs(2));
                // The clean terminal reply reaches the UI before the service
                // exits. DNS recovery errors leave it available for retry.
                if exiting && response["ok"] == true {
                    platform::signal(stop);
                }
            }
            DisconnectNamedPipe(handle.0);
        }
    }
    Ok(())
}
struct Bridge {
    base: PathBuf,
    root: PathBuf,
    job: Mutex<OwnedHandle>,
    cancel: OwnedHandle,
    stop: isize,
}
impl Bridge {
    fn new_job() -> Result<OwnedHandle, String> {
        unsafe {
            let job = OwnedHandle(CreateJobObjectW(null(), null()));
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.0.is_null()
                || SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
            {
                return Err("Cannot create a supervised process group".into());
            }
            Ok(job)
        }
    }
    fn new(base: PathBuf, root: PathBuf, stop: isize) -> Result<Self, String> {
        unsafe {
            let job = Self::new_job()?;
            let cancel = OwnedHandle(CreateEventW(null(), 1, 0, null()));
            if cancel.0.is_null() {
                return Err("Cannot create the cancellation event".into());
            }
            Ok(Self {
                base,
                root,
                job: Mutex::new(job),
                cancel,
                stop,
            })
        }
    }
    fn limit(&self, action: &str) -> Duration {
        let seconds = match action {
            "Connect" | "Select" | "Import" | "ApplySettings" | "ValidateSettings" => 60,
            "Init" => 60,
            "Disconnect" | "Exit" | "Recover" => 20,
            "Snapshot" => 15,
            "Update" | "UpdateDns" => 25,
            _ => 30,
        };
        let normal = Duration::from_secs(seconds);
        if self.root.join("qa.marker").exists()
            && let Ok(bytes) = std::fs::read(self.root.join("runtime/qa-operation-limits.json"))
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && let Some(milliseconds) = value[action].as_u64()
        {
            return Duration::from_millis(milliseconds.clamp(200, normal.as_millis() as u64));
        }
        normal
    }
    fn terminate(&self) -> Result<(), String> {
        // A terminated job can still be finishing its children when the next
        // cleanup helper is spawned. Windows refuses assignment during that
        // interval, so cleanup and future connections use a fresh private job.
        let next = Self::new_job()?;
        let mut job = self.job.lock().unwrap();
        if unsafe { TerminateJobObject(job.0, ERROR_CANCELLED) } == 0 {
            return Err("Не удалось остановить процессы Mukhomor".into());
        }
        *job = next;
        Ok(())
    }
    fn call(&self, value: &Value) -> Result<Value, String> {
        let action = value["action"].as_str().unwrap_or("");
        let limit = self.limit(action);
        let script = self.base.join("NativeBridge.ps1");
        let system = std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into());
        let mut child = Command::new(
            PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        )
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Root")
        .arg(&self.root)
        .env("MUKHOMOR_ASSETS", &self.base)
        .env("MUKHOMOR_CORE_PATH", bundled_core_path(&self.base))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
        if unsafe {
            AssignProcessToJobObject(self.job.lock().unwrap().0, child.as_raw_handle() as HANDLE)
        } == 0
        {
            let error = unsafe { GetLastError() };
            let _ = child.kill();
            return Err(format!(
                "Cannot supervise the background process safely (Windows error {error})"
            ));
        }
        let stdout = child.stdout.take().unwrap();
        let (output_tx, output_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let result = read_bridge_frame(stdout);
            let _ = output_tx.send(result);
        });
        let mut input = child.stdin.take().unwrap();
        let payload = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        let (input_tx, input_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let result = input.write_all(&payload);
            drop(input);
            let _ = input_tx.send(result);
        });
        let cancellable = CANCELLABLE_ACTIONS.contains(&action);
        let mut handles = vec![child.as_raw_handle() as HANDLE];
        if cancellable {
            handles.extend([self.cancel.0, self.stop as HANDLE]);
        }
        let wait = unsafe {
            WaitForMultipleObjects(
                handles.len() as u32,
                handles.as_ptr(),
                0,
                limit.as_millis() as u32,
            )
        };
        if cancellable && (wait == WAIT_OBJECT_0 + 1 || wait == WAIT_OBJECT_0 + 2) {
            // The private job contains only this application's trusted helper
            // and inherited engine children. This also covers cancellation
            // between spawning Mihomo and writing its owned session record.
            let _ = self.terminate();
            let _ = child.kill();
            unsafe {
                CancelSynchronousIo(writer.as_raw_handle() as HANDLE);
                CancelSynchronousIo(reader.as_raw_handle() as HANDLE);
            }
            let _ = input_rx.recv_timeout(Duration::from_millis(200));
            let _ = output_rx.recv_timeout(Duration::from_millis(200));
            return Err("Подключение отменено".into());
        }
        if wait == WAIT_TIMEOUT {
            if ["Connect", "Select", "Disconnect", "Exit", "Init"].contains(&action) {
                let _ = self.terminate();
            }
            let _ = child.kill();
            unsafe {
                CancelSynchronousIo(writer.as_raw_handle() as HANDLE);
                CancelSynchronousIo(reader.as_raw_handle() as HANDLE);
            }
            let _ = input_rx.recv_timeout(Duration::from_millis(200));
            let _ = output_rx.recv_timeout(Duration::from_millis(200));
            return Err(format!(
                "Операция {action} превысила {} секунд. Подключение завершено с ошибкой; проверь диагностику.",
                limit.as_secs()
            ));
        }
        if wait == WAIT_FAILED {
            if ["Connect", "Select", "Disconnect", "Exit", "Init"].contains(&action) {
                let _ = self.terminate();
            }
            let _ = child.kill();
            unsafe {
                CancelSynchronousIo(writer.as_raw_handle() as HANDLE);
                CancelSynchronousIo(reader.as_raw_handle() as HANDLE);
            }
            return Err("Не удалось дождаться фоновой операции Mukhomor".into());
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let _ = input_rx.recv_timeout(Duration::from_millis(200));
        let output = match output_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => result?,
            Err(_) => {
                unsafe {
                    CancelSynchronousIo(reader.as_raw_handle() as HANDLE);
                }
                return Err("Фоновый процесс не закрыл ответ; операция остановлена".into());
            }
        };
        decode_bridge_reply(&output, status.success())
    }
    fn core(&self) -> Option<OwnedHandle> {
        let bytes = std::fs::read(self.root.join("runtime/session.json")).ok()?;
        let session: Value = serde_json::from_slice(&bytes).ok()?;
        let pid = u32::try_from(session["pid"].as_u64()?).ok()?;
        unsafe {
            let handle = OwnedHandle(OpenProcess(
                SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            ));
            if handle.0.is_null() {
                return None;
            }
            if process_creation_time(handle.0)?.as_str() != session["started"].as_str()? {
                return None;
            }
            let mut buffer = vec![0u16; 32768];
            let mut length = buffer.len() as u32;
            if QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut length) == 0 {
                return None;
            }
            let actual = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
            let bin = self.base.join("bin");
            let allowed = [
                bin.join("mihomo-windows-amd64-compatible.exe"),
                bin.join("mihomo-mycelium.exe"),
            ];
            // QueryFullProcessImageName supplies the running image path even
            // when the current package has changed; don't require the file to exist.
            let actual_text = actual.to_string_lossy().to_lowercase();
            if !allowed
                .iter()
                .any(|path| path.to_string_lossy().to_lowercase() == actual_text)
            {
                return None;
            }
            if let Some(recorded) = session.get("core_path")
                && recorded.as_str()?.to_lowercase() != actual_text
            {
                return None;
            }
            Some(handle)
        }
    }
}
fn cached_snapshot(bridge: &Bridge, cache: &SharedSnapshot) -> Value {
    match bridge.call(&json!({"action":"Snapshot"})) {
        Ok(data) => data,
        Err(_) => {
            let mut data = cache.lock().unwrap().clone();
            let running = bridge
                .core()
                .is_some_and(|core| unsafe { WaitForSingleObject(core.0, 0) == WAIT_TIMEOUT });
            data["running"] = json!(running);
            data["starting"] = json!(false);
            data["dns_recovery_pending"] =
                json!(!running && bridge.root.join("runtime/dns-backup.json").exists());
            data["status_uncertain"] = json!(true);
            let configured = data["traffic_guard"]["configured"] == true
                || bridge.root.join("private/traffic-guard-plan.json").exists();
            data["traffic_guard"] = json!({"configured":configured,"verified":false,"protected":false,"recovery_pending":configured});
            data
        }
    }
}
fn complete(
    cache: &SharedSnapshot,
    intent: &Arc<Mutex<ConnectionIntent>>,
    request: &Request,
    mut snapshot: Value,
    result: &Result<(), String>,
) -> Value {
    let current = intent.lock().unwrap();
    let mut data = cache.lock().unwrap();
    if snapshot.get("rule_issues").is_none() && snapshot["settings"] == data["settings"] {
        snapshot["rule_issues"] = data["rule_issues"].clone();
        snapshot["rule_issues_total"] = data["rule_issues_total"].clone();
    }
    let newer = data["operation_id"].as_u64().unwrap_or(0) > request.id;
    let pending = if newer {
        data["operation"].as_str().unwrap_or("").to_owned()
    } else {
        String::new()
    };
    let active_id = if newer {
        data["operation_id"].clone()
    } else {
        json!(request.id)
    };
    let active_request = if newer {
        data["request_id"].clone()
    } else {
        json!(&request.request_id)
    };
    let old_last = data["last_operation"].clone();
    let completed = data["completed_operation_id"]
        .as_u64()
        .unwrap_or(0)
        .max(request.id);
    let error = result.as_ref().err().cloned().unwrap_or_default();
    let cancelled = error == "Подключение отменено";
    let mut terminal = json!({"id":request.id,"request_id":request.request_id,"action":request.value["action"],"ok":result.is_ok(),"cancelled":cancelled,"error":error});
    if request.value["action"] == "Import"
        && result.is_ok()
        && let Some(count) = snapshot["imported_count"]
            .as_u64()
            .filter(|n| (1..=128).contains(n))
    {
        terminal["imported_count"] = json!(count);
    }
    // A completed action is never a pending startup. Only a newer accepted
    // action may decorate it as connecting or disconnecting.
    snapshot["starting"] = json!(false);
    decorate(&mut snapshot, &current, &pending);
    snapshot["operation_id"] = active_id;
    snapshot["request_id"] = active_request;
    snapshot["completed_operation_id"] = json!(completed);
    snapshot["last_operation"] = if old_last["id"].as_u64().unwrap_or(0) > request.id {
        old_last
    } else {
        terminal
    };
    snapshot["last_error"] = json!(if result.is_err() && !cancelled {
        error.clone()
    } else {
        String::new()
    });
    if request.value["action"] == "Exit" && result.is_ok() {
        snapshot["shutting_down"] = json!(true);
    }
    data.replace(snapshot.clone());
    json!({"ok":result.is_ok(),"error":error,"cancelled":cancelled,"request_id":request.request_id,"operation_id":request.id,"data":snapshot})
}
fn background_snapshot(
    cache: &SharedSnapshot,
    intent: &Arc<Mutex<ConnectionIntent>>,
    mut snapshot: Value,
) {
    let current = intent.lock().unwrap();
    let mut data = cache.lock().unwrap();
    if snapshot.get("rule_issues").is_none() && snapshot["settings"] == data["settings"] {
        snapshot["rule_issues"] = data["rule_issues"].clone();
        snapshot["rule_issues_total"] = data["rule_issues_total"].clone();
    }
    let pending = data["operation_id"].as_u64().unwrap_or(0)
        > data["completed_operation_id"].as_u64().unwrap_or(0);
    let operation = if pending {
        data["operation"].as_str().unwrap_or("").to_owned()
    } else {
        String::new()
    };
    decorate(&mut snapshot, &current, &operation);
    for key in [
        "operation_id",
        "completed_operation_id",
        "last_operation",
        "request_id",
        "last_error",
    ] {
        snapshot[key] = data[key].clone();
    }
    data.replace(snapshot);
}
fn startup_snapshot(bridge: &Bridge) -> Value {
    let index = std::fs::read(bridge.root.join("private/profiles.json"))
        .ok()
        .and_then(|x| serde_json::from_slice::<Value>(&x).ok())
        .unwrap_or(json!({"profiles":[],"selected":"","autoconnect":false}));
    let mut settings = std::fs::read(bridge.root.join("settings.json"))
        .or_else(|_| std::fs::read(bridge.base.join("assets/settings.default.json")))
        .ok()
        .and_then(|x| serde_json::from_slice::<Value>(&x).ok())
        .unwrap_or(json!({"dns_update":{"enabled":false}}));
    crate::presets::normalize(&mut settings);
    json!({"profiles":index["profiles"],"selected":index["selected"],"autoconnect":index["autoconnect"],"settings":settings,"running":false,"starting":false,"dns_recovery_pending":bridge.root.join("runtime/dns-backup.json").exists(),"traffic_guard":{"configured":bridge.root.join("private/traffic-guard-plan.json").exists(),"verified":false,"protected":false},"status_uncertain":true})
}
pub fn run(base: PathBuf, root: PathBuf, stop: isize) -> Result<(), String> {
    // Only an isolated test worker can model SCM's actual boot-start reason.
    let system_boot =
        root.join("qa.marker").exists() && root.join("runtime/qa-system-boot.marker").exists();
    run_with_startup(base, root, stop, system_boot)
}
pub fn run_with_startup(
    base: PathBuf,
    root: PathBuf,
    stop: isize,
    system_boot: bool,
) -> Result<(), String> {
    run_with_startup_mode(base, root, stop, system_boot, false)
}
pub fn run_cleanup_only(base: PathBuf, root: PathBuf, stop: isize) -> Result<(), String> {
    run_with_startup_mode(base, root, stop, false, true)
}
fn initial_connection_desired(
    initial: &Value,
    saved: Option<&ConnectionIntent>,
    system_boot: bool,
    interrupted_disconnect: bool,
    cleanup_only: bool,
    durable_cancel: bool,
) -> bool {
    !cleanup_only
        && !durable_cancel
        && initial["status_uncertain"] != true
        && if system_boot {
            initial["autoconnect"] == true && startup_target(initial).is_some()
        } else {
            initial["selected"].as_str().is_some_and(|s| !s.is_empty())
                && !interrupted_disconnect
                && saved.is_some_and(|saved| saved.desired)
        }
}
fn run_with_startup_mode(
    base: PathBuf,
    root: PathBuf,
    stop: isize,
    system_boot: bool,
    cleanup_only: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let bridge = Bridge::new(base, root.clone(), stop)?;
    // A manual cancellation can survive a crash before the worker performs its
    // cleanup. SCM's real boot-start reason deliberately overrides it next boot.
    let interrupted_disconnect =
        std::fs::read(root.join("runtime/stop.request")).is_ok_and(|bytes| bytes == b"disconnect");
    // A shutdown marker belongs to the preceding service lifetime.
    let _ = std::fs::remove_file(root.join("runtime/stop.request"));
    let mut initial = match bridge.call(&json!({"action":"Init"})) {
        Ok(data) => data,
        Err(error) => {
            let mut data = startup_snapshot(&bridge);
            data["last_error"] = json!(error);
            data
        }
    };
    let cancellation = crate::setup::cleanup_request::pending(&root)?;
    let durable_cancel = cancellation.is_some();
    if cleanup_only && cancellation.is_none() {
        return Err("CLEANUP_REQUEST_MISSING".into());
    }
    let (saved, persistence_error) = match read_intent(&root) {
        Ok(saved) => (saved, None),
        Err(error) if cancellation.is_some() => return Err(error),
        Err(error) => (None, Some(error)),
    };
    let initial_desired = initial_connection_desired(
        &initial,
        saved.as_ref(),
        system_boot,
        interrupted_disconnect,
        cleanup_only,
        durable_cancel,
    );
    let intent = Arc::new(Mutex::new(ConnectionIntent {
        generation: saved.as_ref().map_or(0, |saved| saved.generation),
        desired: initial_desired,
    }));
    if let Some(error) = persistence_error {
        initial["last_error"] = json!(error);
        intent.lock().unwrap().desired = false;
    }
    {
        let mut current = intent.lock().unwrap();
        finish_startup_intent(
            &root,
            &mut current,
            &mut initial,
            cleanup_only,
            cancellation,
        )?;
    }
    if !intent.lock().unwrap().desired
        && (initial["running"] == true || initial["starting"] == true)
    {
        let result = bridge.call(&json!({"action":"Recover"}));
        match bridge.call(&json!({"action":"Snapshot"})) {
            Ok(data) => initial = data,
            Err(_) => {
                initial["running"] = json!(
                    bridge
                        .core()
                        .is_some_and(|h| unsafe { WaitForSingleObject(h.0, 0) == WAIT_TIMEOUT })
                );
                initial["starting"] = json!(false);
                initial["dns_recovery_pending"] =
                    json!(root.join("runtime/dns-backup.json").exists());
                initial["status_uncertain"] = json!(true);
                let configured = initial["traffic_guard"]["configured"] == true
                    || root.join("private/traffic-guard-plan.json").exists();
                initial["traffic_guard"] = json!({"configured":configured,"verified":false,"protected":false,"recovery_pending":configured});
            }
        }
        if let Err(error) = result {
            initial["last_error"] = json!(error);
        }
    }
    decorate(&mut initial, &intent.lock().unwrap(), "");
    initial["operation_id"] = json!(0);
    initial["completed_operation_id"] = json!(0);
    initial["last_operation"] = Value::Null;
    let cache = Arc::new(Mutex::new(Snapshot::new(initial)));
    let login_available = Arc::new(AtomicBool::new({
        let data = cache.lock().unwrap();
        !cleanup_only
            && !durable_cancel
            && !interrupted_disconnect
            && (data["last_error"].is_null() || data["last_error"].as_str() == Some(""))
    }));
    let mut boot_target = if system_boot {
        startup_target(&cache.lock().unwrap()).map(str::to_owned)
    } else {
        None
    };
    let wake_handle = OwnedHandle(unsafe { CreateEventW(null(), 0, 0, null()) });
    if wake_handle.0.is_null() {
        return Err("Cannot create the request event".into());
    }
    let wake = wake_handle.0 as isize;
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let listener_root = root.clone();
    let listener_cache = cache.clone();
    let listener_intent = intent.clone();
    let cancel = bridge.cancel.0 as isize;
    let counter = Arc::new(AtomicU64::new(0));
    thread::spawn(move || {
        let result = listener(
            listener_root,
            listener_cache,
            listener_intent,
            tx,
            (wake, stop, cancel),
            counter,
            login_available,
        );
        if result.is_err() {
            platform::signal(stop);
        }
        let _ = ready_tx.send(result);
    });
    // Listener errors surface immediately; healthy listener blocks on events.
    if let Ok(result) = ready_rx.recv_timeout(Duration::from_millis(100)) {
        result?;
    }
    let mut retry = if intent.lock().unwrap().desired {
        Some(Instant::now())
    } else {
        None
    };
    let mut failures = 0;
    let mut core = bridge.core();
    let mut stable_since = core.as_ref().map(|_| Instant::now());
    let mut next_dns = Instant::now();
    let mut clean_exit = false;
    while !platform::signaled(stop) {
        if let Some(at) = retry
            && at <= Instant::now()
        {
            if let Err(error) = permit_connect(&root) {
                let mut current = intent.lock().unwrap();
                current.desired = false;
                let _ = save_intent(&root, &current);
                let mut data = cache.lock().unwrap();
                data["last_error"] = json!(error);
                decorate(&mut data, &current, "");
                retry = None;
                continue;
            }
            let can_start = {
                let current = intent.lock().unwrap();
                if current.desired {
                    unsafe {
                        ResetEvent(bridge.cancel.0);
                    }
                    let _ = std::fs::remove_file(root.join("runtime/stop.request"));
                    decorate(&mut cache.lock().unwrap(), &current, "Connect");
                }
                current.desired
            };
            if !can_start {
                retry = None;
                continue;
            }
            let retry_generation = intent.lock().unwrap().generation;
            let mut value = json!({"action":"Connect"});
            if let Some(id) = &boot_target {
                value["id"] = json!(id);
            }
            let mut result = bridge.call(&value);
            if let Ok(data) = &mut result {
                let current = intent.lock().unwrap();
                if let Err(error) = remember_connection(&root, data, &current, retry_generation) {
                    data["connection_history_warning"] = json!(error);
                }
            }
            match result {
                Ok(mut data) => {
                    boot_target = None;
                    if !intent.lock().unwrap().desired {
                        let pending_stop = ["Disconnect", "Exit"]
                            .contains(&cache.lock().unwrap()["operation"].as_str().unwrap_or(""));
                        if !pending_stop {
                            let _ = bridge.call(&json!({"action":"Recover"}));
                        }
                        data = cached_snapshot(&bridge, &cache);
                    }
                    background_snapshot(&cache, &intent, data);
                    core = bridge.core();
                    retry = None;
                    stable_since = core.as_ref().map(|_| Instant::now());
                }
                Err(error) => {
                    let _ = bridge.terminate();
                    if !["Disconnect", "Exit"]
                        .contains(&cache.lock().unwrap()["operation"].as_str().unwrap_or(""))
                    {
                        let _ = bridge.call(&json!({"action":"Recover"}));
                    }
                    let data = cached_snapshot(&bridge, &cache);
                    background_snapshot(&cache, &intent, data);
                    if intent.lock().unwrap().desired {
                        cache.lock().unwrap()["last_error"] = json!(error);
                    }
                    failures += 1;
                    retry = if intent.lock().unwrap().desired && failures < 3 {
                        Some(Instant::now() + Duration::from_secs(15))
                    } else {
                        None
                    };
                }
            }
        }
        while let Ok(request) = rx.try_recv() {
            if platform::signaled(stop) {
                let _ = request
                    .reply
                    .send(json!({"ok":false,"error":"Service is stopping"}));
                break;
            }
            let action = request.value["action"].as_str().unwrap_or("").to_owned();
            if ["Connect", "Select", "Disconnect", "Exit"].contains(&action.as_str()) {
                boot_target = None;
            }
            if !ACTIONS.contains(&action.as_str()) {
                let _ = request
                    .reply
                    .send(json!({"ok":false,"error":"Unknown operation"}));
                continue;
            }
            if ["Disconnect", "Exit"].contains(&action.as_str()) {
                retry = None;
            } else if action == "Connect" {
                retry = None;
                failures = 0;
            }
            if action == "Connect"
                && let Some(error) = &request.persistence_error
            {
                let current = intent.lock().unwrap();
                decorate(&mut cache.lock().unwrap(), &current, "");
                drop(current);
                let data = cache.lock().unwrap().clone();
                let response = complete(&cache, &intent, &request, data, &Err(error.clone()));
                let _ = request.reply.send(response);
                continue;
            }
            let stale = {
                let current = intent.lock().unwrap();
                let stale = request
                    .generation
                    .is_some_and(|generation| generation != current.generation);
                // Keep the generation check and marker reset under the same
                // lock. A concurrent Disconnect must never lose its marker.
                if !stale {
                    // Cancellation belongs to the previous accepted intent,
                    // not the controller lifetime. Select may run while idle
                    // before Connect; rule updates also remain useful offline.
                    // Their admission generation prevents this reset from
                    // erasing a newer Disconnect queued concurrently.
                    if CANCELLABLE_ACTIONS.contains(&action.as_str()) {
                        unsafe {
                            ResetEvent(bridge.cancel.0);
                        }
                        let _ = std::fs::remove_file(root.join("runtime/stop.request"));
                    }
                    let mut data = cache.lock().unwrap();
                    if data["operation_id"] == request.id {
                        decorate(&mut data, &current, &action);
                    }
                }
                stale
            };
            if stale {
                let data = cache.lock().unwrap().clone();
                let response = complete(
                    &cache,
                    &intent,
                    &request,
                    data,
                    &Err("Подключение отменено".into()),
                );
                let _ = request.reply.send(response);
                continue;
            }
            let local_metadata = ["Rename", "Autoconnect"].contains(&action.as_str())
                && cache.lock().unwrap()["status_uncertain"] != true;
            let mut result = if local_metadata {
                let snapshot = cache.lock().unwrap().clone();
                profile_store::call(&root, &request.value, snapshot)
            } else {
                bridge.call(&request.value)
            };
            if let Some(error) = &request.persistence_error {
                result = Err(error.clone());
            }
            if CANCELLABLE_ACTIONS.contains(&action.as_str())
                && request
                    .generation
                    .is_some_and(|generation| generation != intent.lock().unwrap().generation)
            {
                // The user cancelled while PowerShell was checking the tunnel.
                // Its own rollback normally already stopped the core. Repeating
                // cleanup is safe and covers cancellation at the final commit.
                if !intent.lock().unwrap().desired {
                    retry = None;
                }
                result = Err("Подключение отменено".into());
            }
            if ["Connect", "Select"].contains(&action.as_str())
                && let Ok(data) = &mut result
            {
                let current = intent.lock().unwrap();
                if let Some(generation) = request.generation
                    && let Err(error) = remember_connection(&root, data, &current, generation)
                {
                    data["connection_history_warning"] = json!(error);
                }
            }
            let response = match result {
                Ok(data) => {
                    if action == "Exit" {
                        clean_exit = true;
                        crate::service::begin_shutdown();
                    }
                    core = bridge.core();
                    if ["Connect", "Select"].contains(&action.as_str()) {
                        stable_since = core.as_ref().map(|_| Instant::now());
                    }
                    complete(&cache, &intent, &request, data, &Ok(()))
                }
                Err(error) => {
                    if action == "Connect" {
                        // Covers failures before the helper registered its new
                        // engine. Cleanup is scoped to this controller's job.
                        let _ = bridge.terminate();
                        let pending_disconnect = ["Disconnect", "Exit"]
                            .contains(&cache.lock().unwrap()["operation"].as_str().unwrap_or(""));
                        if !pending_disconnect {
                            let _ = bridge.call(&json!({"action":"Recover"}));
                        }
                        let mut current = intent.lock().unwrap();
                        if request.generation == Some(current.generation)
                            && error != "Подключение отменено"
                        {
                            current.desired = false;
                            let _ = save_intent(&root, &current);
                        }
                    }
                    let data = if local_metadata {
                        cache.lock().unwrap().clone()
                    } else {
                        cached_snapshot(&bridge, &cache)
                    };
                    core = bridge.core();
                    complete(&cache, &intent, &request, data, &Err(error))
                }
            };
            let _ = request.reply.send(response);
        }
        if stable_since.is_some_and(|at| at.elapsed() >= Duration::from_secs(300)) {
            failures = 0;
            stable_since = None;
        }
        if core
            .as_ref()
            .is_some_and(|h| unsafe { WaitForSingleObject(h.0, 0) == WAIT_OBJECT_0 })
        {
            core = None;
            stable_since = None;
            cache.lock().unwrap()["running"] = json!(false);
            let _ = bridge.call(&json!({"action":"Recover"}));
            let data = cached_snapshot(&bridge, &cache);
            background_snapshot(&cache, &intent, data);
            if intent.lock().unwrap().desired && failures < 3 {
                failures += 1;
                retry = Some(Instant::now() + Duration::from_secs(5));
            }
        }
        let dns =
            cache.lock().unwrap()["settings"]["dns_update"]["enabled"] == true && core.is_some();
        if dns && next_dns <= Instant::now() {
            if let Ok(data) = bridge.call(&json!({"action":"UpdateDns"})) {
                background_snapshot(&cache, &intent, data);
            }
            next_dns = Instant::now() + Duration::from_secs(20);
        }
        let mut timeout = u32::MAX;
        if let Some(at) = retry {
            timeout = at
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX as u128) as u32;
        }
        if dns {
            timeout = timeout.min(
                next_dns
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(u32::MAX as u128) as u32,
            );
        }
        let mut handles = vec![stop as HANDLE, wake as HANDLE];
        if let Some(ref handle) = core {
            handles.push(handle.0);
        }
        unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout);
        }
    }
    if !clean_exit {
        let _ = bridge.call(&json!({"action":"Recover"}));
    }
    if let Ok(result) = ready_rx.try_recv() {
        result?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_status_invalidates_on_all_mutations_and_new_instances() {
        let mut cache =
            Snapshot::new(json!({"settings":{"raw":"x".repeat(2*1024*1024)},"running":false}));
        let full = cache.response(&json!({"action":"Status"}));
        assert!(full["data"].is_object());
        let request = json!({"action":"Status","if_revision":full["revision"]});
        let unchanged = cache.response(&request);
        assert_eq!(unchanged["not_modified"], true);
        assert!(unchanged.get("data").is_none());
        assert!(serde_json::to_vec(&unchanged).unwrap().len() < 160);
        let token = cache.token();
        let _read_only = cache["settings"]["raw"].as_str().unwrap();
        assert_eq!(cache.token(), token);
        cache["last_error"] = json!("recovery failed");
        assert!(cache.response(&request)["data"].is_object());
        let request = json!({"if_revision":cache.token()});
        cache.replace(json!({"running":true}));
        assert!(cache.response(&request)["data"].is_object());
        let restarted = Snapshot::new(cache.clone());
        assert_ne!(cache.token(), restarted.token());
        for revision in [Value::Null, json!(0), json!("old-version")] {
            assert!(cache.response(&json!({"if_revision":revision}))["data"].is_object());
        }
    }

    #[test]
    #[ignore = "explicit release-mode performance probe"]
    fn measure_conditional_status() {
        let cache =
            Snapshot::new(json!({"settings":{"raw":"x".repeat(2*1024*1024)},"running":false}));
        for (name, request) in [
            ("full", json!({"action":"Status"})),
            (
                "conditional",
                json!({"action":"Status","if_revision":cache.token()}),
            ),
        ] {
            let started = Instant::now();
            let mut bytes = 0;
            for _ in 0..100 {
                let reply = cache.response(std::hint::black_box(&request));
                bytes = serde_json::to_vec(std::hint::black_box(&reply))
                    .unwrap()
                    .len();
            }
            println!(
                "{{\"case\":\"{name}\",\"response_bytes\":{bytes},\"mean_us\":{}}}",
                started.elapsed().as_micros() / 100
            );
        }
    }

    #[test]
    fn process_creation_format_preserves_epoch_fraction_and_utc_invariants() {
        let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
        time.wYear = 1601;
        time.wMonth = 1;
        time.wDay = 1;
        assert_eq!(
            format_creation_time(&time, 0),
            "1601-01-01T00:00:00.0000000Z"
        );
        assert_eq!(
            format_creation_time(&time, 1),
            "1601-01-01T00:00:00.0000001Z"
        );
        time.wYear = 2026;
        time.wMonth = 10;
        time.wDay = 10;
        time.wHour = 17;
        time.wMinute = 5;
        time.wSecond = 9;
        time.wMilliseconds = 999;
        assert_eq!(
            format_creation_time(&time, 12_345_678),
            "2026-10-10T17:05:09.2345678Z"
        );
        assert_ne!(
            format_creation_time(&time, 12_345_679),
            format_creation_time(&time, 12_345_678)
        );
        let epoch = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        assert_eq!(
            creation_time_utc(&epoch).as_deref(),
            Some("1601-01-01T00:00:00.0000000Z")
        );
        let fraction = FILETIME {
            dwLowDateTime: 9_999_999,
            dwHighDateTime: 0,
        };
        assert_eq!(
            creation_time_utc(&fraction).as_deref(),
            Some("1601-01-01T00:00:00.9999999Z")
        );
        let invalid = FILETIME {
            dwLowDateTime: u32::MAX,
            dwHighDateTime: u32::MAX,
        };
        assert!(creation_time_utc(&invalid).is_none());
    }

    #[test]
    fn process_creation_query_fails_closed_for_an_invalid_handle() {
        assert!(process_creation_time(null_mut()).is_none());
        assert!(process_creation_time(unsafe { GetCurrentProcess() }).is_some());
    }

    #[test]
    fn owned_core_checks_the_start_time_of_a_real_bundled_child() {
        let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let source = bundled_core_path(&package);
        if !source.is_file() {
            eprintln!("Skipping real ownership fixture: fetch a bundled core first.");
            return;
        }
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let tests = package.join("tests");
        let root = tests.join(format!(
            "native-owned-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("create isolated ownership fixture");
        struct Fixture {
            root: PathBuf,
            parent: PathBuf,
            child: Option<std::process::Child>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Some(child) = &mut self.child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if self.root.is_absolute()
                    && self.root.parent() == Some(self.parent.as_path())
                    && self
                        .root
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("native-owned-")
                {
                    let _ = std::fs::remove_dir_all(&self.root);
                }
            }
        }
        let mut fixture = Fixture {
            root: root.clone(),
            parent: tests,
            child: None,
        };
        std::fs::create_dir(root.join("bin")).unwrap();
        std::fs::create_dir(root.join("runtime")).unwrap();
        std::fs::create_dir(root.join("private")).unwrap();
        let image = root.join("bin").join("mihomo-mycelium.exe");
        std::fs::copy(source, &image).expect("copy isolated core image");
        let config = root.join("fixture.json");
        std::fs::write(&config, json!({"mixed-port":0,"external-controller":"","allow-lan":false,
            "log-level":"silent","geo-auto-update":false,"dns":{"enable":false},"tun":{"enable":false},
            "proxies":[],"rules":["MATCH,REJECT"]}).to_string()).unwrap();
        fixture.child = Some(
            Command::new(&image)
                .arg("-d")
                .arg(&root)
                .arg("-f")
                .arg(config)
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start isolated core child"),
        );
        let child = fixture.child.as_ref().unwrap();
        let created = process_creation_time(child.as_raw_handle() as HANDLE).unwrap();
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let output = Command::new(shell).args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("[Diagnostics.Process]::GetProcessById({}).StartTime.ToUniversalTime().ToString('o')", child.id()))
            .creation_flags(CREATE_NO_WINDOW).output().expect("query fixture start time through .NET");
        assert!(
            output.status.success(),
            "fixture process metadata query failed"
        );
        assert!(
            String::from_utf8(output.stdout).unwrap().trim() == created,
            "Rust and PowerShell creation timestamps must match exactly"
        );
        let bridge = Bridge::new(root.clone(), root.clone(), 0).unwrap();
        let session = root.join("runtime/session.json");
        let mut record =
            json!({"pid":child.id(),"started":created,"core_path":image.to_string_lossy()});
        let save = |value: &Value| std::fs::write(&session, value.to_string()).unwrap();
        save(&record);
        assert!(
            bridge.core().is_some(),
            "the real recorded bundled child must be owned"
        );
        record["started"] = json!("1601-01-01T00:00:00.0000000Z");
        save(&record);
        assert!(
            bridge.core().is_none(),
            "a real PID with a stale creation timestamp must be rejected"
        );
        record["started"] = json!(created.replace('Z', "+00:00"));
        save(&record);
        assert!(
            bridge.core().is_none(),
            "the persisted UTC timestamp must use the PowerShell round-trip format"
        );
        record.as_object_mut().unwrap().remove("started");
        save(&record);
        assert!(
            bridge.core().is_none(),
            "missing start time must fail closed"
        );
        record["started"] = json!(created);
        record["pid"] = json!(u64::from(child.id()) + (1_u64 << 32));
        save(&record);
        assert!(
            bridge.core().is_none(),
            "a PID outside the Windows range must never wrap to a live child"
        );
        record["pid"] = json!(child.id());
        record["core_path"] = Value::Null;
        save(&record);
        assert!(
            bridge.core().is_none(),
            "invalid recorded path must fail closed"
        );
        record["core_path"] = json!(image.to_string_lossy());
        save(&record);
        std::fs::write(root.join("bin/mycelium-build.json"), "{damaged manifest").unwrap();
        assert!(
            bridge.core().is_some(),
            "damaged manifest cannot hide the running owned child"
        );
        std::fs::remove_file(root.join("bin/mycelium-build.json")).unwrap();
        assert!(
            bridge.core().is_some(),
            "removed manifest cannot hide the running owned child"
        );
    }

    #[test]
    fn bridge_frame_handles_fragmented_unicode_and_exact_size_boundaries() {
        struct Fragmented<'a>(&'a [u8]);
        impl Read for Fragmented<'_> {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() || target.is_empty() {
                    return Ok(0);
                }
                target[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }
        let message = "Тест\r\n日本🍄";
        let raw = format!("{}\r\n", json!({"ok":false,"error":message}));
        let frame = read_bridge_frame(Fragmented(raw.as_bytes())).unwrap();
        assert_eq!(decode_bridge_reply(&frame, false).unwrap_err(), message);
        let valid = format!("{}\n", json!({"ok":true,"data":{"running":true}}));
        let frame = read_bridge_frame(valid.as_bytes()).unwrap();
        assert_eq!(decode_bridge_reply(&frame, true).unwrap()["running"], true);
        assert!(
            decode_bridge_reply(&frame, false).is_err(),
            "a success frame cannot hide helper exit failure"
        );
        let exact = [vec![b' '; MAX_BRIDGE_FRAME - 1], vec![b'\n']].concat();
        assert_eq!(
            read_bridge_frame(exact.as_slice()).unwrap().len(),
            MAX_BRIDGE_FRAME - 1
        );
        let over = [vec![b' '; MAX_BRIDGE_FRAME], vec![b'\n']].concat();
        assert_eq!(
            read_bridge_frame(over.as_slice()).unwrap_err(),
            "Bridge response is too large"
        );
    }

    #[test]
    fn bridge_frame_rejects_truncation_noise_invalid_utf8_and_wrong_schema() {
        assert!(read_bridge_frame(&b"{\"ok\":true,\"data\":{}}"[..]).is_err());
        assert!(read_bridge_frame(&b""[..]).is_err());
        for bytes in [
            &b"warning\n{\"ok\":true,\"data\":{}}\n"[..],
            &b"{\"ok\":true,\"data\":{\"x\":\"\xff\"}}\n"[..],
            &b"{\"ok\":true,\"data\":null}\n"[..],
            &b"{\"data\":{}}\n"[..],
            &b"{\"ok\":false}\n"[..],
        ] {
            let frame = read_bridge_frame(bytes).unwrap();
            assert!(decode_bridge_reply(&frame, true).is_err());
        }
    }

    #[test]
    fn stopped_or_starting_core_cannot_report_protected_even_with_verified_filters() {
        for running in [json!(false), Value::Null, json!("true"), json!(true)] {
            for starting in [false, true] {
                let mut snapshot = json!({"running":running,"starting":starting,
                    "traffic_guard":{"configured":true,"armed":true,"verified":true,
                        "protected":true,"active_owned_tun":true}});
                let expected = running == true && !starting;
                decorate(&mut snapshot, &ConnectionIntent::default(), "");
                assert_eq!(snapshot["traffic_guard"]["protected"], expected);
                assert_eq!(snapshot["traffic_guard"]["verified"], true);
                assert_eq!(snapshot["traffic_guard"]["armed"], true);
                assert_eq!(snapshot["can_disconnect"], true);
                if !expected && !starting {
                    assert_eq!(snapshot["phase"], "recovery");
                    assert_eq!(snapshot["traffic_guard"]["recovery_pending"], true);
                }
            }
        }
    }
    #[test]
    fn cleanup_only_start_never_resumes_saved_or_boot_connect_intent() {
        let saved = ConnectionIntent {
            generation: 7,
            desired: true,
        };
        let initial = json!({"selected":"existing-profile","profiles":[{"id":"existing-profile"}],"autoconnect":true});
        assert!(initial_connection_desired(
            &initial,
            Some(&saved),
            false,
            false,
            false,
            false
        ));
        assert!(initial_connection_desired(
            &initial,
            Some(&saved),
            true,
            false,
            false,
            false
        ));
        assert!(!initial_connection_desired(
            &initial,
            Some(&saved),
            false,
            true,
            false,
            false
        ));
        for boot in [false, true] {
            for cancelled in [false, true] {
                assert!(!initial_connection_desired(
                    &initial,
                    Some(&saved),
                    boot,
                    cancelled,
                    true,
                    false
                ));
            }
        }
        assert!(!initial_connection_desired(
            &json!({"selected":"existing-profile","status_uncertain":true}),
            Some(&saved),
            false,
            false,
            false,
            false
        ));
        assert!(!initial_connection_desired(
            &json!({"selected":""}),
            Some(&saved),
            false,
            false,
            false,
            false
        ));
    }
    #[test]
    fn startup_uses_last_success_and_never_falls_back_from_explicit_empty_history() {
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let mut data = json!({"selected":b,"last_connected":a,"profiles":[{"id":a},{"id":b}],"autoconnect":true});
        assert_eq!(startup_target(&data), Some(a.as_str()));
        data["selected"] = json!("");
        data["profiles"] = json!([{"id":a}]);
        assert!(initial_connection_desired(
            &data, None, true, false, false, false
        ));
        data["last_connected"] = json!("");
        assert_eq!(startup_target(&data), None);
        data["selected"] = json!(a);
        data.as_object_mut().unwrap().remove("last_connected");
        assert_eq!(startup_target(&data), Some(a.as_str()));
        data["last_connected"] = json!(b);
        assert_eq!(startup_target(&data), None);
    }
    #[test]
    fn login_admission_is_once_only_and_respects_boot_manual_and_recovery_states() {
        let a = "a".repeat(32);
        let data = json!({"last_connected":a,"profiles":[{"id":a}],"autoconnect":true,"service_ready":true,"phase":"idle","last_error":null});
        let idle = ConnectionIntent {
            generation: 1,
            desired: false,
        };
        let mut available = true;
        assert!(allow_login_connect(&data, &idle, &mut available));
        assert!(!allow_login_connect(&data, &idle, &mut available));
        let mut empty_error = data.clone();
        empty_error["last_error"] = json!("");
        let mut available = true;
        assert!(allow_login_connect(&empty_error, &idle, &mut available));
        for (field, value) in [
            ("busy", json!(true)),
            ("running", json!(true)),
            ("starting", json!(true)),
            ("phase", json!("recovery")),
            ("service_ready", json!(false)),
            ("status_uncertain", json!(true)),
            ("shutting_down", json!(true)),
            ("last_error", json!("failure")),
            ("autoconnect", json!(false)),
            ("last_connected", json!("")),
        ] {
            let mut blocked = data.clone();
            blocked[field] = value;
            let mut available = true;
            assert!(
                !allow_login_connect(&blocked, &idle, &mut available),
                "{field}"
            );
            assert!(!available);
        }
        let mut available = true;
        assert!(!allow_login_connect(
            &data,
            &ConnectionIntent {
                generation: 2,
                desired: true
            },
            &mut available
        ));
        assert!(!available);
    }
    #[test]
    fn connection_history_commits_only_the_successful_current_generation() {
        let root = std::env::temp_dir().join(format!(
            "mukhomor-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("private")).unwrap();
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let path = root.join("private/profiles.json");
        let index =
            json!({"schema":1,"selected":b,"last_connected":a,"profiles":[{"id":a},{"id":b}]});
        let original = serde_json::to_vec(&index).unwrap();
        std::fs::write(&path, &original).unwrap();
        let current = ConnectionIntent {
            generation: 3,
            desired: true,
        };
        let mut data = json!({"selected":b,"last_connected":a,"running":false});
        remember_connection(&root, &mut data, &current, 3).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        data["running"] = json!(true);
        remember_connection(&root, &mut data, &current, 2).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        remember_connection(
            &root,
            &mut data,
            &ConnectionIntent {
                generation: 3,
                desired: false,
            },
            3,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        remember_connection(&root, &mut data, &current, 3).unwrap();
        assert_eq!(data["last_connected"], b);
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["last_connected"], b);
        assert_eq!(saved["profiles"], index["profiles"]);
        std::fs::write(&path, &original).unwrap();
        std::fs::create_dir(root.join("private/profiles.json.pending")).unwrap();
        data["last_connected"] = json!(a);
        assert!(remember_connection(&root, &mut data, &current, 3).is_err());
        assert_eq!(data["running"], true);
        assert_eq!(data["last_connected"], a);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn cleanup_startup_rejects_actual_intent_persistence_failure_before_readiness() {
        let root = std::env::temp_dir().join(format!(
            "mukhomor-intent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let previous = br#"{"schema":1,"generation":7,"desired":true}"#;
        std::fs::write(runtime.join("connection-intent.json"), previous).unwrap();
        // A directory at the temporary file name causes a real create_new
        // failure without changing permissions or invoking SCM/WFP.
        std::fs::create_dir(runtime.join("connection-intent.json.pending")).unwrap();
        let mut intent = ConnectionIntent {
            generation: 7,
            desired: false,
        };
        let mut initial = json!({});
        assert!(persist_startup_intent(&root, &mut intent, &mut initial, true).is_err());
        assert_eq!(initial["service_ready"], Value::Null);
        assert_eq!(
            std::fs::read(runtime.join("connection-intent.json")).unwrap(),
            previous
        );
        assert!(read_intent(&root).unwrap().unwrap().desired);
        intent.desired = true;
        assert!(persist_startup_intent(&root, &mut intent, &mut initial, false).is_ok());
        assert!(!intent.desired);
        assert!(initial["last_error"].is_string());
        std::fs::remove_dir(runtime.join("connection-intent.json.pending")).unwrap();
        assert!(persist_startup_intent(&root, &mut intent, &mut initial, true).is_ok());
        assert!(!read_intent(&root).unwrap().unwrap().desired);
        assert_eq!(read_intent(&root).unwrap().unwrap().generation, 7);
        std::fs::remove_file(runtime.join("connection-intent.json")).unwrap();
        std::fs::remove_dir(runtime).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn durable_cancellation_survives_failed_persistence_and_is_consumed_after_false() {
        let tree = crate::setup::cleanup_request::fixtures::Tree::new();
        crate::setup::cleanup_request::publish(&tree.root).unwrap();
        let cancel = crate::setup::cleanup_request::pending(&tree.root)
            .unwrap()
            .unwrap();
        let saved = read_intent(&tree.root).unwrap().unwrap();
        for boot in [false, true] {
            assert!(!initial_connection_desired(
                &json!({"selected":"existing-profile","autoconnect":true}),
                Some(&saved),
                boot,
                false,
                false,
                true
            ));
        }
        let pending_path = tree.root.join("runtime/connection-intent.json.pending");
        std::fs::create_dir(&pending_path).unwrap();
        let mut intent = ConnectionIntent {
            generation: 7,
            desired: false,
        };
        let mut initial = json!({});
        assert!(
            finish_startup_intent(&tree.root, &mut intent, &mut initial, true, Some(cancel))
                .is_err()
        );
        assert_eq!(initial["service_ready"], Value::Null);
        assert!(read_intent(&tree.root).unwrap().unwrap().desired);
        let cancel = crate::setup::cleanup_request::pending(&tree.root)
            .unwrap()
            .unwrap();
        std::fs::remove_dir(&pending_path).unwrap();
        // A subsequent ordinary SCM restart has no cleanup startup argument,
        // but still completes the durable request before any ready/Connect.
        finish_startup_intent(&tree.root, &mut intent, &mut initial, false, Some(cancel)).unwrap();
        let saved = read_intent(&tree.root).unwrap().unwrap();
        assert!(!saved.desired);
        assert_eq!(saved.generation, 7);
        assert!(
            crate::setup::cleanup_request::pending(&tree.root)
                .unwrap()
                .is_none()
        );
        assert_eq!(initial["service_ready"], Value::Null);
    }
    #[test]
    fn cleanup_cancellation_admission_blocks_connect_until_durable_false_and_consume() {
        let tree = crate::setup::cleanup_request::fixtures::Tree::new();
        let mut intent = read_intent(&tree.root).unwrap().unwrap();
        crate::setup::cleanup_request::publish(&tree.root).unwrap();
        assert_eq!(
            admit_connection_intent(&tree.root, &mut intent, "Connect").unwrap_err(),
            "CLEANUP_REQUEST_PENDING"
        );
        assert!(!intent.desired);
        assert!(!read_intent(&tree.root).unwrap().unwrap().desired);
        assert!(permit_connect(&tree.root).is_err());
        admit_connection_intent(&tree.root, &mut intent, "Disconnect").unwrap();
        assert!(
            crate::setup::cleanup_request::pending(&tree.root)
                .unwrap()
                .is_none()
        );
        assert!(permit_connect(&tree.root).is_ok());
        admit_connection_intent(&tree.root, &mut intent, "Connect").unwrap();
        assert!(intent.desired);
        assert!(read_intent(&tree.root).unwrap().unwrap().desired);
        crate::setup::cleanup_request::publish(&tree.root).unwrap();
        let path = tree.root.join("runtime/connection-intent.json.pending");
        std::fs::create_dir(&path).unwrap();
        assert!(admit_connection_intent(&tree.root, &mut intent, "Exit").is_err());
        assert!(!intent.desired);
        assert!(read_intent(&tree.root).unwrap().unwrap().desired);
        assert!(
            crate::setup::cleanup_request::pending(&tree.root)
                .unwrap()
                .is_some()
        );
        assert!(permit_connect(&tree.root).is_err());
        std::fs::remove_dir(&path).unwrap();
        admit_connection_intent(&tree.root, &mut intent, "Exit").unwrap();
        assert!(!read_intent(&tree.root).unwrap().unwrap().desired);
        assert!(permit_connect(&tree.root).is_ok());
        std::fs::write(
            tree.root.join("recovery-request/disconnect.request"),
            b"malformed",
        )
        .unwrap();
        assert!(admit_connection_intent(&tree.root, &mut intent, "Connect").is_err());
        assert!(admit_connection_intent(&tree.root, &mut intent, "Disconnect").is_err());
        assert_eq!(
            std::fs::read(tree.root.join("recovery-request/disconnect.request")).unwrap(),
            b"malformed"
        );
        assert!(!read_intent(&tree.root).unwrap().unwrap().desired);
        assert!(permit_connect(&tree.root).is_err());
    }
}
