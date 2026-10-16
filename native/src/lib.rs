//! Shared Slint host for Linux and Android. Windows retains its service adapter.
#[cfg(not(windows))]
use mukhomor_ui::visuals::appearance;
#[cfg(not(windows))]
mod design;
#[cfg(not(windows))]
mod i18n;
#[cfg(not(windows))]
mod presets;
#[cfg(not(windows))]
mod slint_host;
#[cfg(not(windows))]
mod app_update;
#[cfg(not(windows))]
mod portable;
#[cfg(not(windows))]
use portable::autostart;
#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "linux")]
pub fn run_desktop() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pair) = args.windows(2).find(|p| p[0] == "--after-update-pid") {
        let pid: u32 = pair[1].parse().map_err(|_| "Invalid update process")?;
        if pid == 0 || pid == std::process::id() { return Err("Invalid update process".into()); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            if std::time::Instant::now() >= deadline { return Err("Close the previous Mukhomor window and reopen the app".into()); }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    let root = portable::data_root()?;
    slint_host::run(root)
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: mukhomor_ui::slint::android::AndroidApp) {
    if let Err(error) = android::run(app) {
        eprintln!("Mukhomor: {error}");
    }
}
