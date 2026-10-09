use crate::{backend, platform};
use std::{path::PathBuf, ptr::null_mut, sync::OnceLock};
use windows_sys::Win32::{Foundation::*, System::Services::*};
static CONFIG: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
static STOP: OnceLock<isize> = OnceLock::new();
static STATUS: OnceLock<isize> = OnceLock::new();
const NAME: &str = "Mukhomor";

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
unsafe extern "system" fn entry(_argc: u32, _argv: *mut *mut u16) {
    let name = platform::wide(NAME);
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null_mut()) };
    if handle.is_null() {
        return;
    }
    let _ = STATUS.set(handle as isize);
    let stop = platform::new_event();
    let _ = STOP.set(stop);
    status(SERVICE_RUNNING, false);
    let result = if let Some((base, root)) = CONFIG.get() {
        backend::run_with_startup(base.clone(), root.clone(), stop, is_system_boot(handle))
    } else {
        Err("Missing service configuration".into())
    };
    if let (Err(error), Some((_, root))) = (&result, CONFIG.get()) {
        let _ = std::fs::create_dir_all(root.join("runtime/logs"));
        let _ = std::fs::write(root.join("runtime/logs/native-service-error.log"), error);
    }
    status(SERVICE_STOPPED, result.is_err());
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
