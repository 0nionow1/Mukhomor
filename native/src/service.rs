use crate::{backend, platform};
use std::{path::PathBuf, ptr::null_mut, sync::OnceLock};
use windows_sys::Win32::{Foundation::*, System::Services::*};
static CONFIG: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
static STOP: OnceLock<isize> = OnceLock::new();
static STATUS: OnceLock<isize> = OnceLock::new();
const NAME: &str = "Mukhomor";
fn cleanup_start(args: &[&str]) -> Result<bool, String> {
    match args {
        [NAME] => Ok(false),
        [NAME, "cleanup-only"] => Ok(true),
        _ => Err("Invalid controller startup arguments".into()),
    }
}
unsafe fn startup_arguments(argc: u32, argv: *mut *mut u16) -> Result<bool, String> {
    if !(1..=2).contains(&argc) || argv.is_null() {
        return Err("Invalid controller startup arguments".into());
    }
    let mut arguments = Vec::new();
    for index in 0..argc as usize {
        let value = unsafe { *argv.add(index) };
        if value.is_null() {
            return Err("Invalid controller startup arguments".into());
        }
        let mut count = 0;
        while count < 64 && unsafe { *value.add(count) } != 0 {
            count += 1;
        }
        if count == 64 {
            return Err("Invalid controller startup arguments".into());
        }
        arguments.push(
            String::from_utf16(unsafe { std::slice::from_raw_parts(value, count) })
                .map_err(|_| "Invalid controller startup arguments")?,
        );
    }
    cleanup_start(&arguments.iter().map(String::as_str).collect::<Vec<_>>())
}

fn is_system_boot(handle: SERVICE_STATUS_HANDLE) -> bool {
    unsafe {
        let mut information = null_mut();
        if QueryServiceDynamicInformation(
            handle,
            SERVICE_DYNAMIC_INFORMATION_LEVEL_START_REASON,
            &mut information,
        ) == 0
            || information.is_null()
        {
            // Unknown start reasons never override a user's persisted choice.
            return false;
        }
        let reason = (*(information as *const SERVICE_START_REASON)).dwReason;
        LocalFree(information);
        reason & (SERVICE_START_REASON_AUTO | SERVICE_START_REASON_DELAYEDAUTO) != 0
            && reason & SERVICE_START_REASON_RESTART_ON_FAILURE == 0
    }
}

fn status(state: u32, failed: bool) {
    unsafe {
        if let Some(handle) = STATUS.get() {
            let status = SERVICE_STATUS {
                dwServiceType: SERVICE_WIN32_OWN_PROCESS,
                dwCurrentState: state,
                dwControlsAccepted: if state == SERVICE_RUNNING {
                    SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_PRESHUTDOWN
                } else {
                    0
                },
                dwWin32ExitCode: if failed {
                    ERROR_SERVICE_SPECIFIC_ERROR
                } else {
                    0
                },
                dwServiceSpecificExitCode: if failed { 1 } else { 0 },
                dwCheckPoint: if state == SERVICE_STOP_PENDING { 1 } else { 0 },
                dwWaitHint: if state == SERVICE_STOP_PENDING {
                    90000
                } else {
                    0
                },
            };
            SetServiceStatus(*handle as SERVICE_STATUS_HANDLE, &status);
        }
    }
}
pub fn begin_shutdown() {
    // A successful authenticated Exit has already recovered DNS and stopped
    // the tunnel. Mark SCM before its reply so reopening waits for a clean
    // stopped service instead of attaching to the closing IPC endpoint.
    status(SERVICE_STOP_PENDING, false);
}
unsafe extern "system" fn control(
    code: u32,
    _kind: u32,
    _data: *mut std::ffi::c_void,
    _ctx: *mut std::ffi::c_void,
) -> u32 {
    if code == SERVICE_CONTROL_STOP
        || code == SERVICE_CONTROL_SHUTDOWN
        || code == SERVICE_CONTROL_PRESHUTDOWN
    {
        status(SERVICE_STOP_PENDING, false);
        if let Some((_, root)) = CONFIG.get() {
            let marker = root.join("runtime/stop.request");
            if !marker.exists() {
                let _ = std::fs::write(marker, b"stop");
            }
        }
        if let Some(event) = STOP.get() {
            platform::signal(*event);
        }
    }
    NO_ERROR
}
unsafe extern "system" fn entry(argc: u32, argv: *mut *mut u16) {
    let name = platform::wide(NAME);
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null_mut()) };
    if handle.is_null() {
        return;
    }
    let _ = STATUS.set(handle as isize);
    let stop = platform::new_event();
    let _ = STOP.set(stop);
    status(SERVICE_RUNNING, false);
    let arguments = unsafe { startup_arguments(argc, argv) };
    let cleanup_only = arguments.as_ref().is_ok_and(|cleanup| *cleanup);
    let result = arguments.and_then(|cleanup| {
        if let Some((base, root)) = CONFIG.get() {
            if cleanup {
                backend::run_cleanup_only(base.clone(), root.clone(), stop)
            } else {
                backend::run_with_startup(base.clone(), root.clone(), stop, is_system_boot(handle))
            }
        } else {
            Err("Missing service configuration".into())
        }
    });
    if let (Err(error), Some((_, root))) = (&result, CONFIG.get()) {
        let _ = std::fs::create_dir_all(root.join("runtime/logs"));
        let _ = std::fs::write(root.join("runtime/logs/native-service-error.log"), error);
    }
    // Retain the diagnostic, but cleanup startup errors must not trigger an
    // ordinary SCM recovery that could re-enable saved connection desire.
    status(SERVICE_STOPPED, result.is_err() && !cleanup_only);
    unsafe {
        CloseHandle(stop as HANDLE);
    }
}
pub fn dispatch(base: PathBuf, root: PathBuf) -> Result<(), String> {
    let _ = CONFIG.set((base, root));
    let mut name = platform::wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(entry),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err("Windows could not start the Mukhomor service".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_cleanup_start_is_exact_and_normal_start_stays_normal() {
        assert!(!cleanup_start(&[NAME]).unwrap());
        assert!(cleanup_start(&[NAME, "cleanup-only"]).unwrap());
        for args in [
            vec![],
            vec!["Other"],
            vec![NAME, "Cleanup-only"],
            vec![NAME, "cleanup-only", "extra"],
            vec!["Other", "cleanup-only"],
        ] {
            assert!(cleanup_start(&args).is_err());
        }
        assert!(unsafe { startup_arguments(0, null_mut()) }.is_err());
        assert!(unsafe { startup_arguments(2, null_mut()) }.is_err());
        let mut name = platform::wide(NAME);
        let mut flag = platform::wide("cleanup-only");
        let mut pointers = [name.as_mut_ptr(), flag.as_mut_ptr()];
        assert!(unsafe { startup_arguments(2, pointers.as_mut_ptr()) }.unwrap());
    }
}
