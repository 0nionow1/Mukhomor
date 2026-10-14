//! Restart only the already-installed controller for an explicit cleanup.
//! This path cannot install, upgrade, elevate, or start a normal reconnect.
//! Local I/O is synchronous. Deadline checks bound our loops, not every OS call;
//! StartServiceW itself may wait up to 30 seconds for another service's control.
use super::{data_root, folder, installed};
use crate::{
    backend,
    platform::{user_sid, wide},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{File, OpenOptions},
    io::Read,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::Services::*,
    UI::Shell::CSIDL_PROGRAM_FILES,
};
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const ENTRY_LIMIT: usize = 4096;
const VALIDATION_BUDGET: Duration = Duration::from_secs(15);
const READINESS_BUDGET: Duration = Duration::from_secs(15);
const CLEANUP_ARGUMENT: &str = "cleanup-only";
const FILE_WRITE_MASK: u32 = 0x000d_0156 | GENERIC_ALL | GENERIC_WRITE;
const SERVICE_WRITE_MASK: u32 =
    SERVICE_CHANGE_CONFIG | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE;

struct LocalAllocation(*mut std::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
struct Algorithm(BCRYPT_ALG_HANDLE);
impl Drop for Algorithm {
    fn drop(&mut self) {
        unsafe {
            BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}
struct Hash(BCRYPT_HASH_HANDLE);
impl Drop for Hash {
    fn drop(&mut self) {
        unsafe {
            BCryptDestroyHash(self.0);
        }
    }
}
fn remaining(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("RECOVERY_VALIDATION_TIMEOUT".into())
    } else {
        Ok(())
    }
}
fn trusted(sid: PSID, owner: Option<PSID>) -> bool {
    !sid.is_null()
        && unsafe {
            IsValidSid(sid) != 0
                && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                    || owner.is_some_and(|expected| EqualSid(sid, expected) != 0))
        }
}
fn safe_descriptor(
    sd: PSECURITY_DESCRIPTOR,
    owner: Option<PSID>,
    write_mask: u32,
) -> Result<(), String> {
    unsafe {
        let mut actual_owner = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut acl = null_mut();
        if sd.is_null()
            || IsValidSecurityDescriptor(sd) == 0
            || GetSecurityDescriptorOwner(sd, &mut actual_owner, &mut defaulted) == 0
            || !trusted(actual_owner, owner)
            || GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0
            || present == 0
            || acl.is_null()
        {
            return Err("RECOVERY_OBJECT_SECURITY".into());
        }
        let mut info = ACL_SIZE_INFORMATION::default();
        if GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
            AclSizeInformation,
        ) == 0
            || info.AceCount > 4096
        {
            return Err("RECOVERY_OBJECT_SECURITY".into());
        }
        for index in 0..info.AceCount {
            let mut raw = null_mut();
            if GetAce(acl, index, &mut raw) == 0 || raw.is_null() {
                return Err("RECOVERY_OBJECT_SECURITY".into());
            }
            let header = &*raw.cast::<ACE_HEADER>();
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            match header.AceType {
                0 => {
                    if usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>() {
                        return Err("RECOVERY_OBJECT_SECURITY".into());
                    }
                    let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
                    let sid = (&ace.SidStart as *const u32).cast_mut().cast();
                    if !trusted(sid, owner) && ace.Mask & write_mask != 0 {
                        return Err("RECOVERY_OBJECT_SECURITY".into());
                    }
                }
                1 => {} // Deny ACEs cannot grant mutation.
                _ => return Err("RECOVERY_OBJECT_SECURITY".into()),
            }
        }
    }
    Ok(())
}
pub(super) fn handle_security(
    handle: HANDLE,
    kind: SE_OBJECT_TYPE,
    owner: Option<PSID>,
    mask: u32,
) -> Result<(), String> {
    let mut sd = null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            kind,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    } != 0
    {
        return Err("RECOVERY_SECURITY_READ".into());
    }
    let allocated = LocalAllocation(sd);
    safe_descriptor(allocated.0, owner, mask)
}
pub(super) fn pin(path: &Path, directory: bool, owner: Option<PSID>) -> Result<File, String> {
    // READ_DATA/LIST_DIRECTORY, not just READ_ATTRIBUTES, excludes existing
    // writers. No delete sharing pins ordinary identities through verification.
    let file = OpenOptions::new()
        .read(true)
        .access_mode(FILE_GENERIC_READ)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|_| "RECOVERY_OBJECT_OPEN")?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err("RECOVERY_OBJECT_IDENTITY".into());
    }
    handle_security(file.as_raw_handle(), SE_FILE_OBJECT, owner, FILE_WRITE_MASK)?;
    Ok(file)
}
fn bounded_json(file: &File, limit: u64) -> Result<Value, String> {
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "RECOVERY_METADATA_READ")?;
    if bytes.len() as u64 > limit {
        return Err("RECOVERY_METADATA_SIZE".into());
    }
    crate::traffic_guard_cli::parse(&bytes).map_err(|_| "RECOVERY_METADATA_JSON".into())
}
fn safe_relative(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    if normalized.len() > 240 || normalized.is_empty() {
        return None;
    }
    for part in normalized.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return None;
        }
        let stem = part.split('.').next()?.to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return None;
        }
    }
    Some(normalized.to_ascii_lowercase())
}
fn manifest_files(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let version = value["version"].as_str().ok_or("RECOVERY_MANIFEST")?;
    if version.split('.').count() != 3
        || !version
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("RECOVERY_MANIFEST".into());
    }
    let entries = value["files"].as_array().ok_or("RECOVERY_MANIFEST")?;
    if entries.is_empty() || entries.len() > ENTRY_LIMIT {
        return Err("RECOVERY_MANIFEST".into());
    }
    let mut files = BTreeMap::new();
    for entry in entries {
        let path = safe_relative(entry["path"].as_str().ok_or("RECOVERY_MANIFEST")?)
            .ok_or("RECOVERY_MANIFEST_PATH")?;
        let digest = entry["sha256"].as_str().ok_or("RECOVERY_MANIFEST")?;
        if path == "release-manifest.json"
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || files.insert(path, digest.to_ascii_lowercase()).is_some()
        {
            return Err("RECOVERY_MANIFEST".into());
        }
    }
    if !files.contains_key("mukhomor.exe") {
        return Err("RECOVERY_MANIFEST".into());
    }
    Ok(files)
}
fn stream_hash(mut input: impl Read, deadline: Instant) -> Result<String, String> {
    unsafe {
        let mut alg = null_mut();
        if BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, null(), 0) < 0 {
            return Err("RECOVERY_HASH_PROVIDER".into());
        }
        let alg = Algorithm(alg);
        let mut hash = null_mut();
        if BCryptCreateHash(alg.0, &mut hash, null_mut(), 0, null(), 0, 0) < 0 {
            return Err("RECOVERY_HASH_CREATE".into());
        }
        let hash = Hash(hash);
        let mut buffer = [0u8; 65536];
        loop {
            remaining(deadline)?;
            let count = input.read(&mut buffer).map_err(|_| "RECOVERY_HASH_READ")?;
            if count == 0 {
                break;
            }
            if BCryptHashData(hash.0, buffer.as_ptr(), count as u32, 0) < 0 {
                return Err("RECOVERY_HASH_DATA".into());
            }
        }
        let mut digest = [0; 32];
        if BCryptFinishHash(hash.0, digest.as_mut_ptr(), 32, 0) < 0 {
            return Err("RECOVERY_HASH_FINISH".into());
        }
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    }
}
fn payload(target: &Path, record: &Value, deadline: Instant) -> Result<Vec<File>, String> {
    let program = folder(CSIDL_PROGRAM_FILES)?.join("Mukhomor");
    let base = target.parent().ok_or("RECOVERY_BUNDLE")?;
    let mut pins = vec![pin(&program, true, None)?, pin(base, true, None)?];
    let manifest_file = pin(&base.join("release-manifest.json"), false, None)?;
    let manifest = bounded_json(&manifest_file, MANIFEST_LIMIT)?;
    if manifest["version"] != record["version"] {
        return Err("RECOVERY_MANIFEST_VERSION".into());
    }
    let files = manifest_files(&manifest)?;
    pins.push(manifest_file);
    let mut directories = BTreeSet::new();
    for name in files.keys() {
        let mut current = Path::new(name).parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() {
                break;
            }
            directories.insert(parent.to_string_lossy().replace('\\', "/"));
            current = parent.parent();
        }
    }
    let mut queue = VecDeque::from([PathBuf::new()]);
    let mut seen_files = BTreeSet::new();
    let mut seen_dirs = BTreeSet::new();
    let mut count = 0;
    while let Some(relative) = queue.pop_front() {
        remaining(deadline)?;
        // Its ordinary protected parent is pinned before enumeration/lookup.
        for entry in std::fs::read_dir(base.join(&relative)).map_err(|_| "RECOVERY_BUNDLE_READ")? {
            let entry = entry.map_err(|_| "RECOVERY_BUNDLE_READ")?;
            count += 1;
            if count > ENTRY_LIMIT * 2 + 1 {
                return Err("RECOVERY_BUNDLE_SIZE".into());
            }
            let name = relative.join(entry.file_name());
            let key = safe_relative(name.to_str().ok_or("RECOVERY_BUNDLE_PATH")?)
                .ok_or("RECOVERY_BUNDLE_PATH")?;
            let attributes =
                std::fs::symlink_metadata(entry.path()).map_err(|_| "RECOVERY_BUNDLE_READ")?;
            use std::os::windows::fs::MetadataExt;
            if attributes.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err("RECOVERY_OBJECT_IDENTITY".into());
            }
            if attributes.is_dir() {
                if !directories.contains(&key) || !seen_dirs.insert(key) {
                    return Err("RECOVERY_BUNDLE_SET".into());
                }
                pins.push(pin(&entry.path(), true, None)?);
                queue.push_back(name);
            } else {
                if key == "release-manifest.json" {
                    continue;
                }
                let expected = files.get(&key).ok_or("RECOVERY_BUNDLE_SET")?;
                if !seen_files.insert(key) {
                    return Err("RECOVERY_BUNDLE_SET".into());
                }
                let file = pin(&entry.path(), false, None)?;
                if stream_hash(&file, deadline)? != *expected {
                    return Err("RECOVERY_BUNDLE_HASH".into());
                }
                pins.push(file);
            }
        }
    }
    if seen_files.len() != files.len() || seen_dirs != directories {
        return Err("RECOVERY_BUNDLE_SET".into());
    }
    Ok(pins)
}
fn config_identity(kind: u32, account: &str, command: &str, expected: &str) -> bool {
    kind == SERVICE_WIN32_OWN_PROCESS
        && account.eq_ignore_ascii_case("LocalSystem")
        && command.eq_ignore_ascii_case(expected)
}
unsafe fn config_string(pointer: *const u16) -> Result<String, String> {
    if pointer.is_null() {
        return Err("RECOVERY_SERVICE_CONFIG".into());
    }
    let mut count = 0;
    while count < 32768 && unsafe { *pointer.add(count) } != 0 {
        count += 1;
    }
    if count == 32768 {
        return Err("RECOVERY_SERVICE_CONFIG".into());
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, count) })
        .map_err(|_| "RECOVERY_SERVICE_CONFIG".into())
}
fn config(service: &Service, target: &Path, root: &Path) -> Result<(), String> {
    unsafe {
        let mut needed = 0;
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
        if needed == 0 || needed > 32768 {
            return Err("RECOVERY_SERVICE_CONFIG".into());
        }
        let mut bytes = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let value = bytes.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        if QueryServiceConfigW(service.0, value, needed, &mut needed) == 0 {
            return Err("RECOVERY_SERVICE_CONFIG".into());
        }
        let expected = format!(
            "\"{}\" --service --root \"{}\"",
            target.display(),
            root.display()
        );
        if !config_identity(
            (*value).dwServiceType,
            &config_string((*value).lpServiceStartName)?,
            &config_string((*value).lpBinaryPathName)?,
            &expected,
        ) {
            return Err("RECOVERY_SERVICE_IDENTITY".into());
        }
    }
    Ok(())
}
fn status(service: &Service) -> Result<SERVICE_STATUS_PROCESS, String> {
    let mut value = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    if unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut value as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of_val(&value) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err("RECOVERY_SERVICE_STATUS".into());
    }
    Ok(value)
}
pub(super) fn ensure(root: &Path) -> Result<(), String> {
    let deadline = Instant::now() + VALIDATION_BUDGET;
    if root.canonicalize().map_err(|_| "RECOVERY_ROOT")?
        != data_root().canonicalize().map_err(|_| "RECOVERY_ROOT")?
    {
        return Err("RECOVERY_ROOT".into());
    }
    let mut owner = null_mut();
    if unsafe { ConvertStringSidToSidW(wide(&user_sid()?).as_ptr(), &mut owner) } == 0 {
        return Err("RECOVERY_OWNER".into());
    }
    let owner = LocalAllocation(owner);
    let root_pin = pin(root, true, Some(owner.0))?;
    let record_file = pin(&root.join("installation.json"), false, Some(owner.0))?;
    let owner_file = pin(&root.join("owner.sid"), false, Some(owner.0))?;
    if owner_file.metadata().map_err(|_| "RECOVERY_OWNER")?.len() > 512 {
        return Err("RECOVERY_OWNER".into());
    }
    let record = bounded_json(&record_file, 16384)?;
    let target = installed(root)?;
    if std::env::current_exe()
        .map_err(|_| "RECOVERY_CURRENT_IMAGE")?
        .canonicalize()
        .map_err(|_| "RECOVERY_CURRENT_IMAGE")?
        != target
            .canonicalize()
            .map_err(|_| "RECOVERY_CURRENT_IMAGE")?
    {
        return Err("RECOVERY_CURRENT_IMAGE".into());
    }
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err("RECOVERY_MANAGER".into());
    }
    let manager = Service(manager);
    let handle = unsafe {
        OpenServiceW(
            manager.0,
            wide("Mukhomor").as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
        )
    };
    if handle.is_null() {
        return Err("RECOVERY_SERVICE_OPEN".into());
    }
    let service = Service(handle);
    config(&service, &target, root)?;
    if status(&service)?.dwCurrentState == SERVICE_RUNNING {
        // Existing authenticated cleanup must remain possible after on-disk
        // payload damage. Nothing is launched or changed on this path.
        return Ok(());
    }
    let handle = unsafe {
        OpenServiceW(
            manager.0,
            wide("Mukhomor").as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START | READ_CONTROL,
        )
    };
    if handle.is_null() {
        return Err("RECOVERY_SERVICE_OPEN".into());
    }
    let service = Service(handle);
    config(&service, &target, root)?;
    handle_security(service.0, SE_SERVICE, None, SERVICE_WRITE_MASK)?;
    let _payload_pins = payload(&target, &record, deadline)?;
    let _metadata_pins = (root_pin, record_file, owner_file);
    let mut started = false;
    loop {
        remaining(deadline)?;
        config(&service, &target, root)?;
        let state = status(&service)?;
        if state.dwCurrentState == SERVICE_RUNNING && state.dwProcessId != 0 {
            break;
        }
        if state.dwCurrentState == SERVICE_STOPPED && state.dwProcessId == 0 {
            handle_security(service.0, SE_SERVICE, None, SERVICE_WRITE_MASK)?;
            // The normal user cannot write SYSTEM's runtime intent. A separate
            // narrow cancellation directory persists the choice before a new
            // process can crash or SCM can restart it without startup arguments.
            super::cleanup_request::publish(root)?;
            remaining(deadline)?;
            config(&service, &target, root)?;
            handle_security(service.0, SE_SERVICE, None, SERVICE_WRITE_MASK)?;
            let latest = status(&service)?;
            if latest.dwCurrentState != SERVICE_STOPPED || latest.dwProcessId != 0 {
                return Err("RECOVERY_SERVICE_CHANGED".into());
            }
            let argument = wide(CLEANUP_ARGUMENT);
            let arguments = [argument.as_ptr()];
            // Synchronous: never abandon a thread that could start late.
            if unsafe { StartServiceW(service.0, 1, arguments.as_ptr()) } == 0 {
                return Err("RECOVERY_SERVICE_START".into());
            }
            started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let readiness = Instant::now() + READINESS_BUDGET;
    loop {
        if Instant::now() >= readiness {
            return Err("RECOVERY_SERVICE_TIMEOUT".into());
        }
        config(&service, &target, root)?;
        handle_security(service.0, SE_SERVICE, None, SERVICE_WRITE_MASK)?;
        let state = status(&service)?;
        if state.dwCurrentState == SERVICE_RUNNING
            && state.dwProcessId != 0
            && backend::rpc(root, &serde_json::json!({"action":"Status"}))
                .is_ok_and(|reply| reply["ok"] == true && reply["data"]["service_ready"] == true)
        {
            return Ok(());
        }
        if started && state.dwCurrentState == SERVICE_STOPPED {
            return Err("RECOVERY_SERVICE_STOPPED".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn descriptor(sddl: &str) -> LocalAllocation {
        let mut sd = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide(sddl).as_ptr(),
                    1,
                    &mut sd,
                    null_mut(),
                )
            },
            0
        );
        LocalAllocation(sd)
    }
    #[test]
    fn only_exact_local_system_own_process_controller_can_be_recovered() {
        let command = "\"C:\\Program Files\\Mukhomor\\app-0.7.0-abcd\\mukhomor.exe\" --service --root \"C:\\ProgramData\\Mukhomor\"";
        assert!(config_identity(
            SERVICE_WIN32_OWN_PROCESS,
            "LocalSystem",
            command,
            command
        ));
        assert!(!config_identity(
            SERVICE_WIN32_SHARE_PROCESS,
            "LocalSystem",
            command,
            command
        ));
        assert!(!config_identity(
            SERVICE_WIN32_OWN_PROCESS | 0x100, // Legacy interactive-service flag.
            "LocalSystem",
            command,
            command
        ));
        for account in ["LocalService", "NetworkService", "User", ""] {
            assert!(!config_identity(
                SERVICE_WIN32_OWN_PROCESS,
                account,
                command,
                command
            ));
        }
        for changed in [
            command.replace("app-0.7.0", "app-0.6.0"),
            command.replace("ProgramData", "Temp"),
            format!("{command} --extra"),
        ] {
            assert!(!config_identity(
                SERVICE_WIN32_OWN_PROCESS,
                "LocalSystem",
                &changed,
                command
            ));
        }
    }
    #[test]
    fn metadata_rejects_ambiguous_paths_duplicates_and_unpinned_executable() {
        let digest = "00".repeat(32);
        let good = json!({"version":"0.7.0","files":[{"path":"mukhomor.exe","sha256":digest},{"path":"bin\\core.exe","sha256":digest}]});
        assert_eq!(manifest_files(&good).unwrap().len(), 2);
        for path in [
            "../x",
            "C:\\x",
            "x:stream",
            "/x",
            "x//y",
            "x/../y",
            "x\\.\\y",
            "NUL.txt",
            "COM1",
            "x.",
            "x/кириллица",
        ] {
            assert!(safe_relative(path).is_none(), "{path}");
        }
        let mut duplicate = good.clone();
        duplicate["files"][1]["path"] = json!("MUKHOMOR.EXE");
        assert!(manifest_files(&duplicate).is_err());
        let mut omitted = good.clone();
        omitted["files"][0]["path"] = json!("other.exe");
        assert!(manifest_files(&omitted).is_err());
        let mut bad = good.clone();
        bad["files"][0]["sha256"] = json!("bad");
        assert!(manifest_files(&bad).is_err());
    }
    #[test]
    fn read_only_public_acl_is_allowed_but_mutable_objects_are_rejected() {
        let good = descriptor("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;BU)");
        assert!(safe_descriptor(good.0, None, FILE_WRITE_MASK).is_ok());
        for sddl in [
            "O:BAG:BAD:P(A;;FA;;;WD)",
            "O:BUG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)",
            "O:BAG:BAD:NO_ACCESS_CONTROL",
        ] {
            assert!(safe_descriptor(descriptor(sddl).0, None, FILE_WRITE_MASK).is_err());
        }
        let query_start = descriptor("O:SYG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x20015;;;BU)");
        assert!(safe_descriptor(query_start.0, None, SERVICE_WRITE_MASK).is_ok());
        for mask in [
            SERVICE_CHANGE_CONFIG,
            DELETE,
            WRITE_DAC,
            WRITE_OWNER,
            GENERIC_ALL,
            GENERIC_WRITE,
        ] {
            let sddl = format!("O:SYG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{mask:x};;;BU)");
            assert!(safe_descriptor(descriptor(&sddl).0, None, SERVICE_WRITE_MASK).is_err());
        }
    }
    #[test]
    fn streaming_integrity_uses_sha256_and_deadlines() {
        assert_eq!(
            stream_hash(&b"abc"[..], Instant::now() + Duration::from_secs(1)).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(stream_hash(&b"abc"[..], Instant::now() - Duration::from_secs(1)).is_err());
    }
}
