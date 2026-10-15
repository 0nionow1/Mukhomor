#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use mukhomor_ui::visuals::appearance;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod design;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod i18n;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod presets;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod slint_host;
#[cfg(windows)]
mod traffic_guard;
#[cfg(windows)]
mod traffic_guard_cli;
#[cfg(windows)]
mod traffic_guard_tun;
use std::path::PathBuf;

pub const APP_NAME: &str = "Mukhomor";

fn argument(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|a| a[0] == key).map(|a| a[1].clone())
}
#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let base = argument(&args, "--base").map(PathBuf::from).unwrap_or(base);
    let root = argument(&args, "--root")
        .map(PathBuf::from)
        .unwrap_or_else(setup::data_root);
    let error_root = root.clone();
    let result = if args.iter().any(|a| a == "--traffic-guard-action") {
        traffic_guard_cli::run(&args, &base)
    } else if args.iter().any(|a| a == "--setup" || a == "--start") {
        setup::install(
            &base,
            &argument(&args, "--owner").unwrap_or_default(),
            args.iter().any(|a| a == "--start"),
        )
    } else if args.iter().any(|a| a == "--uninstall" || a == "--remove") {
        setup::uninstall(&base, args.iter().any(|a| a == "--remove"))
    } else if args.iter().any(|a| a == "--service") {
        service::dispatch(base, root)
    } else if args.iter().any(|a| a == "--worker") {
        backend::run(base, root, platform::new_event())
    } else if args.iter().any(|a| a == "--rpc") {
        let request = if let Some(path) = argument(&args, "--request") {
            std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
        } else {
            Ok(serde_json::json!({"action": "Status"}))
        };
        request.and_then(|r| backend::rpc(&root, &r)).and_then(|r| {
            if let Some(path) = argument(&args, "--output") {
                std::fs::write(path, serde_json::to_vec_pretty(&r).unwrap())
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    } else if args.iter().any(|a| a == "--qa-startup") {
        if root.join("qa.marker").is_file() {
            slint_host::run(root)
        } else {
            Err("Startup probe requires an isolated QA root".into())
        }
    } else {
        if args.iter().any(|a| a == "--startup") {
            setup::ensure_startup_ready(&root)
                .and_then(|ready| if ready { slint_host::run(root) } else { Ok(()) })
        } else {
            setup::ensure_installed(&base, &root)
                .and_then(|ready| if ready { slint_host::run(root) } else { Ok(()) })
        }
    };
    if let Err(error) = result {
        if args.iter().any(|a| a == "--startup") {
            if let Ok(folder) = setup::folder(windows_sys::Win32::UI::Shell::CSIDL_LOCAL_APPDATA) {
                let logs = folder.join("Mukhomor");
                let _ = std::fs::create_dir_all(&logs);
                let _ = std::fs::write(logs.join("startup-error.log"), &error);
            }
            std::process::exit(1);
        }
        if args.iter().any(|a| {
            a == "--service"
                || a == "--worker"
                || a == "--rpc"
                || a == "--qa-startup"
                || a == "--traffic-guard-action"
        }) {
            if let Some(path) = argument(&args, "--output") {
                let _ = std::fs::write(
                    path,
                    serde_json::json!({"ok":false,"error":error}).to_string(),
                );
            }
        } else {
            let localized = i18n::Locale::new(&error_root, false)
                .map(|locale| locale.error(&error).into_owned())
                .unwrap_or(error);
            platform::error_box(&localized);
        }
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    if let Err(error) = mukhomor_app::run_desktop() {
        eprintln!("Mukhomor: {error}");
        std::process::exit(1);
    }
}
