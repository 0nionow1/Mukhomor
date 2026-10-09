use crate::platform::{OwnedHandle, user_sid, wide};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Com::*, Services::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub fn folder(id: u32) -> Result<PathBuf, String> {
    let mut buffer = [0u16; 260];
    if unsafe { SHGetFolderPathW(null_mut(), id as i32, null_mut(), 0, buffer.as_mut_ptr()) } < 0 {
        return Err("Не удалось найти системную папку Windows".into());
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

pub fn data_root() -> PathBuf {
    folder(CSIDL_COMMON_APPDATA)
        .expect("Windows ProgramData folder")
        .join("Mukhomor")
}

pub(crate) fn installed(root: &Path) -> Result<PathBuf, String> {
    if std::fs::read_to_string(root.join("owner.sid"))
        .map_err(|_| "Нет доступа к данным Mukhomor")?
        .trim()
        != user_sid()?
    {
        return Err("Эта установка Mukhomor принадлежит другому пользователю Windows".into());
    }
    let record: Value = serde_json::from_slice(
        &std::fs::read(root.join("installation.json"))
            .map_err(|_| "Установка Mukhomor ещё не завершена".to_string())?,
    )
    .map_err(|_| "Повреждены сведения об установке".to_string())?;
    let path = PathBuf::from(record["executable"].as_str().ok_or("Нет пути приложения")?);
    let program = folder(CSIDL_PROGRAM_FILES)?.join("Mukhomor");
    let canonical = path
        .canonicalize()
        .map_err(|_| "Файлы Mukhomor не найдены".to_string())?;
    let program = program
        .canonicalize()
        .map_err(|_| "Папка установки не найдена".to_string())?;
    if canonical.parent().and_then(Path::parent) != Some(program.as_path())
        || path.file_name().and_then(|x| x.to_str()) != Some("mukhomor.exe")
        || path
            .parent()
            .and_then(|x| x.file_name())
            .and_then(|x| x.to_str())
            .is_none_or(|x| {
                let Some((version, id)) = x.strip_prefix("app-").and_then(|x| x.rsplit_once('-'))
                else {
                    return true;
                };
                id.len() != 32
                    || !id.chars().all(|c| c.is_ascii_hexdigit())
                    || version.split('.').count() != 3
                    || !version
                        .split('.')
                        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            })
    {
        return Err("Недопустимый путь установленного приложения".into());
    }
    Ok(path)
}

fn shell(path: &Path, args: &str, elevated: bool, wait: bool) -> Result<(), String> {
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }
    let _com =
        ComGuard(unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32) } >= 0);
    let path_w = wide(&path.to_string_lossy());
    let args_w = wide(args);
    let verb = wide(if elevated { "runas" } else { "open" });
    let directory = wide(&path.parent().unwrap_or(Path::new(".")).to_string_lossy());
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = path_w.as_ptr();
    info.lpParameters = args_w.as_ptr();
    info.lpDirectory = directory.as_ptr();
    info.nShow = if elevated { SW_HIDE } else { SW_SHOWNORMAL };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            "Установка отменена. При первом запуске Windows запрашивает разрешение на настройку VPN.".into()
        } else {
            "Не удалось запустить Mukhomor".into()
        });
    }
    let process = OwnedHandle(info.hProcess);
    if wait {
        if process.0.is_null()
            || unsafe { WaitForSingleObject(process.0, 180_000) } != WAIT_OBJECT_0
        {
            return Err("Настройка Mukhomor не завершилась вовремя".into());
        }
        let mut exit_code = 1;
        unsafe { GetExitCodeProcess(process.0, &mut exit_code) };
        if exit_code != 0 {
            return Err(
                "Не удалось настроить Mukhomor. Проверь права администратора и целостность архива."
                    .into(),
            );
        }
    }
    Ok(())
}

fn release_files(manifest: &Value) -> Option<BTreeMap<&str, &str>> {
    let entries = manifest["files"].as_array()?;
    let files: BTreeMap<_, _> = entries
        .iter()
        .map(|entry| Some((entry["path"].as_str()?, entry["sha256"].as_str()?)))
        .collect::<Option<_>>()?;
    (!files.is_empty() && files.len() == entries.len()).then_some(files)
}

fn same_release(package: &Value, installed: &Value) -> bool {
    package["version"].as_str().is_some()
        && package["version"] == installed["version"]
        && release_files(package).is_some_and(|files| Some(files) == release_files(installed))
}

/// The public EXE is also the one-click installer. Normal installed launches stay unelevated.
pub fn ensure_installed(base: &Path, root: &Path) -> Result<bool, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let package_manifest = std::fs::read(base.join("release-manifest.json"))
        .ok()
        .and_then(|x| serde_json::from_slice::<Value>(&x).ok());
    let installation = std::fs::read(root.join("installation.json"))
        .ok()
        .and_then(|x| serde_json::from_slice::<Value>(&x).ok());
    let installed_version = installation.as_ref().and_then(|x| x["version"].as_str());
    let executable = installation
        .as_ref()
        .and_then(|x| x["executable"].as_str().map(PathBuf::from));
    let installed_binary_exists = executable.as_ref().is_some_and(|path| path.is_file());
    let installed_manifest = executable
        .as_ref()
        .and_then(|path| path.parent())
        .and_then(|folder| std::fs::read(folder.join("release-manifest.json")).ok())
        .and_then(|x| serde_json::from_slice::<Value>(&x).ok());
    // A locally rebuilt package can retain its version. Compare the reviewed
    // payload hashes too, so launching it cannot silently reopen an older build.
    let package_changed = package_manifest.as_ref().is_some_and(|package| {
        package["version"].as_str() != installed_version
            || installed_manifest
                .as_ref()
                .is_none_or(|installed| !same_release(package, installed))
    });
    if installed_version.is_none() || !installed_binary_exists || package_changed {
        if !base.join("release-manifest.json").exists() {
            return Err("Запусти mukhomor.exe из распакованного релиза. Для разработки используй режим --smoke.".into());
        }
        let sid = user_sid()?;
        shell(&current, &format!("--setup --owner {sid}"), true, true)?;
    }
    let mut target = installed(root)?;
    match controller_state()? {
        None => {
            shell(
                &target,
                &format!("--setup --owner {}", user_sid()?),
                true,
                true,
            )?;
            target = installed(root)?;
        }
        Some(SERVICE_STOPPED | SERVICE_STOP_PENDING) => start_controller(&target, root)?,
        _ => {}
    }
    if current.canonicalize().map_err(|e| e.to_string())?
        != target.canonicalize().map_err(|e| e.to_string())?
    {
        shell(&target, "", false, false)?;
        return Ok(false);
    }
    Ok(true)
}

fn start_controller(target: &Path, root: &Path) -> Result<(), String> {
    struct ServiceHandle(SC_HANDLE);
    impl Drop for ServiceHandle {
        fn drop(&mut self) {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
    unsafe {
        let manager = OpenSCManagerW(null_mut(), null_mut(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err("Не удалось открыть контроллер Mukhomor".into());
        }
        let manager = ServiceHandle(manager);
        let service = OpenServiceW(
            manager.0,
            wide("Mukhomor").as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START,
        );
        if service.is_null() {
            return Err("Не удалось открыть Mukhomor. Запусти обновлённый архив для восстановления установки.".into());
        }
        let service = ServiceHandle(service);
        let mut needed = 0;
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
        if needed == 0 || needed > 32768 {
            return Err("Не удалось проверить установленный контроллер".into());
        }
        let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        if QueryServiceConfigW(service.0, config, needed, &mut needed) == 0 {
            return Err("Не удалось проверить установленный контроллер".into());
        }
        let binary = (*config).lpBinaryPathName;
        if binary.is_null() {
            return Err("Не указан путь установленного контроллера".into());
        }
        let mut count = 0;
        while count < 32768 && *binary.add(count) != 0 {
            count += 1;
        }
        let actual = String::from_utf16_lossy(std::slice::from_raw_parts(binary, count));
        let expected = format!(
            "\"{}\" --service --root \"{}\"",
            target.display(),
            root.display()
        );
        if actual.to_lowercase() != expected.to_lowercase() {
            return Err("Контроллер не принадлежит этой установке Mukhomor".into());
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
            let mut size = 0;
            if QueryServiceStatusEx(
                service.0,
                SC_STATUS_PROCESS_INFO,
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
                std::mem::size_of_val(&status) as u32,
                &mut size,
            ) == 0
            {
                return Err("Не удалось проверить состояние Mukhomor".into());
            }
            if status.dwCurrentState == SERVICE_RUNNING {
                return Ok(());
            }
            if status.dwCurrentState == SERVICE_STOPPED
                && StartServiceW(service.0, 0, null_mut()) == 0
                && GetLastError() != ERROR_SERVICE_ALREADY_RUNNING
            {
                return Err("Не удалось запустить Mukhomor. Запусти обновлённый архив для восстановления установки.".into());
            }
            if Instant::now() >= deadline {
                return Err("Mukhomor не запустился вовремя. Повтори запуск приложения.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn controller_state() -> Result<Option<u32>, String> {
    unsafe {
        let manager = OpenSCManagerW(null_mut(), null_mut(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err("Не удалось проверить состояние Mukhomor".into());
        }
        let service = OpenServiceW(manager, wide("Mukhomor").as_ptr(), SERVICE_QUERY_STATUS);
        if service.is_null() {
            let error = GetLastError();
            CloseServiceHandle(manager);
            return if error == ERROR_SERVICE_DOES_NOT_EXIST {
                Ok(None)
            } else {
                Err("Не удалось проверить контроллер Mukhomor".into())
            };
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut needed = 0;
        let success = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of_val(&status) as u32,
            &mut needed,
        );
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        if success == 0 {
            Err("Не удалось получить состояние Mukhomor".into())
        } else {
            Ok(Some(status.dwCurrentState))
        }
    }
}

pub fn install(base: &Path, owner: &str, start_only: bool) -> Result<(), String> {
    if !owner.starts_with("S-1-")
        || !owner
            .chars()
            .all(|c| c.is_ascii_digit() || c == 'S' || c == '-')
    {
        return Err("Некорректный пользователь Windows".into());
    }
    run_script(base, "Install.ps1", Some(owner), start_only)
}

pub fn uninstall(base: &Path, elevated: bool) -> Result<(), String> {
    if elevated {
        run_script(base, "Uninstall.ps1", None, false)
    } else {
        shell(
            &std::env::current_exe().map_err(|e| e.to_string())?,
            "--remove",
            true,
            false,
        )
    }
}

fn run_script(
    base: &Path,
    script: &str,
    owner: Option<&str>,
    start_only: bool,
) -> Result<(), String> {
    let windows = std::env::var_os("SystemRoot").ok_or("Windows folder unavailable")?;
    let powershell = PathBuf::from(windows).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(base.join(script))
        .creation_flags(CREATE_NO_WINDOW);
    if let Some(sid) = owner {
        command.arg("-OwnerSid").arg(sid);
    }
    if start_only {
        command.arg("-StartOnly");
    }
    let result = command.output().map_err(|e| e.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&result.stderr);
        Err(format!(
            "Не удалось завершить настройку Mukhomor.\n{}",
            detail.chars().take(1600).collect::<String>()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rebuilt_release_requires_updated_payload() {
        let installed = json!({"version":"0.4.1","files":[
            {"path":"mukhomor.exe","sha256":"original"},
            {"path":"SplitWindows.psm1","sha256":"module"}
        ]});
        let reordered = json!({"version":"0.4.1","files":[
            {"path":"SplitWindows.psm1","sha256":"module"},
            {"path":"mukhomor.exe","sha256":"original"}
        ]});
        assert!(same_release(&reordered, &installed));
        let mut rebuilt = reordered.clone();
        rebuilt["files"][1]["sha256"] = json!("rebuilt");
        assert!(!same_release(&rebuilt, &installed));
        rebuilt = reordered.clone();
        rebuilt["version"] = json!("0.4.2");
        assert!(!same_release(&rebuilt, &installed));
        assert!(!same_release(
            &json!({"version":"0.4.1","files":[]}),
            &installed
        ));
        let duplicate = json!({"version":"0.4.1","files":[
            {"path":"mukhomor.exe","sha256":"original"},
            {"path":"mukhomor.exe","sha256":"original"}
        ]});
        assert!(!same_release(&duplicate, &installed));
    }
}
