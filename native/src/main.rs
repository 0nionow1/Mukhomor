#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod backend;
mod font;
mod i18n;
mod platform;
mod service;
mod setup;
mod texture;
mod ui;
use std::path::PathBuf;

fn argument(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|a| a[0] == key).map(|a| a[1].clone())
}
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
    let result = if args.iter().any(|a| a == "--setup" || a == "--start") {
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
    } else {
        let smoke = args.iter().any(|a| a == "--smoke");
        if smoke {
            ui::run(base, root, true)
        } else {
            setup::ensure_installed(&base, &root).and_then(|ready| {
                if ready {
                    ui::run(base, root, false)
                } else {
                    Ok(())
                }
            })
        }
    };
    if let Err(error) = result {
        if args
            .iter()
            .any(|a| a == "--service" || a == "--worker" || a == "--rpc" || a == "--smoke")
        {
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
