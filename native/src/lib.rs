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
mod portable;
#[cfg(not(windows))]
use portable::autostart;
#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "linux")]
pub fn run_desktop() -> Result<(), String> {
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
