//! Windows instance activation and authenticated controller recovery.
#![allow(unsafe_op_in_unsafe_fn)]
use crate::platform::{OwnedHandle, wide};
use serde_json::{Value, json};
use std::{ptr::null, thread};
use windows_sys::Win32::{Foundation::*, System::Threading::*, UI::WindowsAndMessaging::*};
const ACTIVATE: u32 = WM_APP + 3;

pub(crate) fn controller_request(
    request: &Value,
    mut rpc: impl FnMut(&Value) -> Result<Value, String>,
    recover: impl FnOnce() -> Result<(), String>,
) -> Result<Value, String> {
    if matches!(request["action"].as_str(), Some("Disconnect" | "Exit")) {
        // A reachable authenticated controller also covers portable QA workers.
        // Probe before a mutation, never restart/retry an ambiguous cleanup RPC.
        let ready = rpc(&json!({"action":"Status"}))
            .is_ok_and(|reply| reply["ok"] == true && reply["data"]["service_ready"] == true);
        if !ready {
            recover()?;
        }
    }
    rpc(request)
}

pub(crate) fn window_identity(root: &std::path::Path) -> Result<String, String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let identity = format!(
        "{}|{}",
        root.to_string_lossy().to_lowercase(),
        crate::platform::user_sid()?
    );
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("Mukhomor.Native.{hash:016x}"))
}
// Held until the message loop exits; only the already installed, unprivileged
// controller uses this guard. Installer bootstrap remains independent.
pub(crate) unsafe fn single_instance(
    class: &str,
    activate: bool,
) -> Result<Option<OwnedHandle>, String> {
    SetLastError(ERROR_SUCCESS);
    let mutex = CreateMutexW(null(), 1, wide(&format!("Local\\{class}")).as_ptr());
    if mutex.is_null() {
        return Err("Не удалось открыть окно Mukhomor".into());
    }
    let existed = GetLastError() == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle(mutex);
    if !existed {
        return Ok(Some(mutex));
    }
    for _ in 0..20 {
        let wait = WaitForSingleObject(mutex.0, 0);
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Ok(Some(mutex));
        }
        if !activate {
            return Ok(None);
        }
        let window = FindWindowW(wide(class).as_ptr(), null());
        if !window.is_null() {
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            AllowSetForegroundWindow(pid);
            if PostMessageW(window, ACTIVATE, 0, 0) != 0 {
                return Ok(None);
            }
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("Окно Mukhomor уже открывается. Попробуй ещё раз через несколько секунд.".into())
}

#[cfg(test)]
mod cleanup_request_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn authenticated_ready_controller_cleanup_preserves_request_and_skips_restart() {
        for action in ["Disconnect", "Exit"] {
            let request =
                json!({"action":action,"request_id":"exact-terminal-receipt","generation":7});
            let mut seen = Vec::new();
            let reply = controller_request(
                &request,
                |value| {
                    seen.push(value.clone());
                    if value["action"] == "Status" {
                        Ok(json!({"ok":true,"data":{"service_ready":true}}))
                    } else {
                        Ok(json!({"ok":true,"request_id":"exact-terminal-receipt"}))
                    }
                },
                || panic!("A ready QA/portable controller must not use installed recovery"),
            )
            .unwrap();
            assert_eq!(seen, vec![json!({"action":"Status"}), request]);
            assert_eq!(reply["request_id"], "exact-terminal-receipt");
        }
    }

    #[test]
    fn unavailable_or_unready_status_requires_checked_recovery_before_single_cleanup() {
        for status in [
            Err("pipe unavailable".into()),
            Ok(json!({"ok":false,"data":{"service_ready":true}})),
            Ok(json!({"ok":true,"data":{"service_ready":false}})),
            Ok(json!({"ok":true,"data":{"service_ready":"true"}})),
        ] {
            let order = RefCell::new(Vec::new());
            controller_request(
                &json!({"action":"Disconnect"}),
                |value| {
                    let action = value["action"].as_str().unwrap();
                    order.borrow_mut().push(action.to_owned());
                    if action == "Status" {
                        status.clone()
                    } else {
                        Ok(json!({"ok":true}))
                    }
                },
                || {
                    order.borrow_mut().push("checked-recovery".into());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                *order.borrow(),
                ["Status", "checked-recovery", "Disconnect"]
            );
        }
    }

    #[test]
    fn failed_recovery_does_not_mutate_and_lost_cleanup_receipt_is_never_retried() {
        let mut calls = 0;
        assert_eq!(
            controller_request(
                &json!({"action":"Exit"}),
                |_| {
                    calls += 1;
                    Err("unavailable".into())
                },
                || Err("untrusted installation".into()),
            ),
            Err("untrusted installation".into())
        );
        assert_eq!(calls, 1);
        let mut actions = Vec::new();
        assert_eq!(
            controller_request(
                &json!({"action":"Exit","request_id":"lost-receipt"}),
                |value| {
                    actions.push(value["action"].as_str().unwrap().to_owned());
                    if value["action"] == "Status" {
                        Ok(json!({"ok":true,"data":{"service_ready":true}}))
                    } else {
                        Err("lost receipt".into())
                    }
                },
                || panic!("An ambiguous cleanup must not trigger restart"),
            ),
            Err("lost receipt".into())
        );
        assert_eq!(actions, ["Status", "Exit"]);
    }

    #[test]
    fn ordinary_requests_have_no_status_probe_or_recovery() {
        let mut calls = 0;
        let request = json!({"action":"Connect"});
        controller_request(
            &request,
            |value| {
                calls += 1;
                assert_eq!(value, &request);
                Ok(json!({"ok":true}))
            },
            || panic!("Ordinary actions cannot start recovery"),
        )
        .unwrap();
        assert_eq!(calls, 1);
    }
}
