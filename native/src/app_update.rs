//! User-initiated updates. Network and installation run outside the UI and VPN controller.
//! Trust anchor: the fixed GitHub repository over verified HTTPS, plus the asset's
//! SHA-256 from the GitHub API. This is not a Windows publisher signature.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const REPOSITORY: &str = "https://github.com/0nionow1/Mukhomor";
const LATEST: &str = "https://api.github.com/repos/0nionow1/Mukhomor/releases/latest";
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
const INVALID: &str = "Некорректный пакет обновления";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

pub enum Event {
    Checked(Result<Option<Release>, String>),
    Progress(u64, u64),
    Installing,
    Installed(Result<Outcome, String>),
}
pub enum Outcome {
    #[cfg(not(target_os = "android"))]
    Restarted,
    #[cfg(target_os = "android")]
    AndroidInstaller,
}

/// One update worker per window. The UI retains the offer after a failed attempt.
pub struct Updater {
    pub busy: bool,
    pub installing: bool,
    pub offer: Option<Release>,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
}
impl Default for Updater {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            busy: false,
            installing: false,
            offer: None,
            sender,
            receiver,
        }
    }
}
impl Updater {
    pub fn check(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.offer = None;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(Event::Checked(check()));
        });
    }
    pub fn install(&mut self, root: PathBuf) {
        if self.busy {
            return;
        }
        let Some(release) = self.offer.clone() else {
            return;
        };
        self.busy = true;
        self.installing = true;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = install(&release, &root, &sender);
            let _ = sender.send(Event::Installed(result));
        });
    }
    pub fn poll(&mut self) -> Option<Event> {
        let event = self.receiver.try_recv().ok()?;
        match &event {
            Event::Checked(result) => {
                self.busy = false;
                self.offer = result.as_ref().ok().cloned().flatten();
            }
            Event::Installed(_) => {
                self.busy = false;
                self.installing = false;
            }
            _ => {}
        }
        Some(event)
    }
}

fn version(value: &str) -> Result<[u32; 3], String> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 {
        return Err(INVALID.into());
    }
    let mut result = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(INVALID.into());
        }
        result[i] = part.parse().map_err(|_| INVALID)?;
    }
    Ok(result)
}

fn suffix() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x64.zip"),
        ("linux", "x86_64") => Ok("linux-x64.tar.gz"),
        ("linux", "aarch64") => Ok("linux-arm64.tar.gz"),
        ("android", "aarch64" | "x86_64") => Ok("android.apk"),
        _ => Err("Для этой платформы нет пакета обновления".into()),
    }
}

fn select_release(data: &Value, current: &str, platform: &str) -> Result<Option<Release>, String> {
    if data["draft"] != false || data["prerelease"] != false {
        return Err(INVALID.into());
    }
    let tag = data["tag_name"].as_str().ok_or(INVALID)?;
    let next = tag.strip_prefix('v').ok_or(INVALID)?;
    if version(next)? <= version(current)? {
        return Ok(None);
    }
    let name = format!("Mukhomor-{next}-{platform}");
    let assets = data["assets"].as_array().ok_or(INVALID)?;
    let matches: Vec<_> = assets.iter().filter(|a| a["name"] == name).collect();
    if matches.len() != 1 {
        return Err("Для этой платформы нет пакета обновления".into());
    }
    let asset = matches[0];
    let url = format!("{REPOSITORY}/releases/download/{tag}/{name}");
    let size = asset["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_PACKAGE)
        .ok_or(INVALID)?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .ok_or("В релизе отсутствует SHA-256 пакета")?;
    if asset["state"] != "uploaded"
        || asset["browser_download_url"] != url
        || digest.len() != 64
        || !digest.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(INVALID.into());
    }
    Ok(Some(Release {
        version: next.into(),
        url,
        size,
        sha256: digest.to_ascii_lowercase(),
    }))
}

fn allowed_url(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    uri.scheme_str() == Some("https")
        && uri.port_u16().is_none_or(|port| port == 443)
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
        && match uri.host() {
            Some("api.github.com") => uri.path() == "/repos/0nionow1/Mukhomor/releases/latest",
            Some("github.com") => uri
                .path()
                .starts_with("/0nionow1/Mukhomor/releases/download/v"),
            Some("release-assets.githubusercontent.com") => {
                uri.path().starts_with("/github-production-release-asset/")
            }
            _ => false,
        }
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut next = url.to_owned();
    for _ in 0..4 {
        if !allowed_url(&next) {
            return Err("Недопустимый адрес обновления".into());
        }
        let response = agent
            .get(&next)
            .header(
                "User-Agent",
                concat!("Mukhomor/", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(
                |_| "Не удалось загрузить обновление. Проверьте подключение и повторите попытку.",
            )?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            next = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or(INVALID)?
                .into();
            continue;
        }
        return match response.status().as_u16() {
            200 => Ok(response),
            403 | 429 => Err("GitHub временно ограничил запросы. Повторите проверку позже.".into()),
            _ => Err("Релиз недоступен. Повторите проверку позже.".into()),
        };
    }
    Err("Слишком много перенаправлений при обновлении".into())
}

fn check() -> Result<Option<Release>, String> {
    let mut response = get(LATEST)?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(INVALID.into());
    }
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
    select_release(&data, env!("CARGO_PKG_VERSION"), suffix()?)
}

fn copy_verified(
    mut input: impl Read,
    mut output: impl Write,
    release: &Release,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut total = 0;
    let mut last_percent = 101;
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|_| "Загрузка прервана. Повторите попытку.")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > release.size || total > MAX_PACKAGE {
            return Err(INVALID.into());
        }
        output
            .write_all(&buffer[..n])
            .map_err(|_| "Не удалось сохранить обновление. Проверьте свободное место.")?;
        hash.update(&buffer[..n]);
        let percent = total * 100 / release.size;
        if percent != last_percent {
            progress(total, release.size);
            last_percent = percent;
        }
    }
    if total != release.size || format!("{:x}", hash.finalize()) != release.sha256 {
        return Err("SHA-256 не совпадает. Обновление не установлено.".into());
    }
    output
        .flush()
        .map_err(|_| "Не удалось сохранить обновление".into())
}

fn safe_entry(name: &str) -> Result<PathBuf, String> {
    // Reject Windows separators/ADS and traversal even when running on Unix.
    if name.is_empty() || name.contains(['\\', ':', '\0']) {
        return Err(INVALID.into());
    }
    let path = Path::new(name);
    if !path.components().all(|c| matches!(c, Component::Normal(_)))
        || name.split('/').any(|s| s == "." || s == "..")
    {
        return Err(INVALID.into());
    }
    Ok(path.into())
}

#[cfg(windows)]
fn unpack(archive: &Path, destination: &Path, _: &Release) -> Result<PathBuf, String> {
    let mut zip =
        zip::ZipArchive::new(File::open(archive).map_err(|_| INVALID)?).map_err(|_| INVALID)?;
    if zip.len() > 4096 {
        return Err(INVALID.into());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|_| INVALID)?;
        let path = safe_entry(entry.name().trim_end_matches('/'))?;
        if path
            .components()
            .next()
            .is_none_or(|c| c.as_os_str() != "Mukhomor")
            || entry.is_symlink()
        {
            return Err(INVALID.into());
        }
        total = total
            .checked_add(entry.size())
            .filter(|n| *n <= MAX_EXPANDED)
            .ok_or(INVALID)?;
        let out = destination.join(path);
        if entry.is_dir() {
            fs::create_dir_all(out).map_err(|_| INVALID)?;
        } else {
            fs::create_dir_all(out.parent().ok_or(INVALID)?).map_err(|_| INVALID)?;
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(out)
                .map_err(|_| INVALID)?;
            let size = entry.size();
            if std::io::copy(&mut (&mut entry).take(size + 1), &mut file).map_err(|_| INVALID)?
                != size
            {
                return Err(INVALID.into());
            }
        }
    }
    let package = destination.join("Mukhomor");
    for file in ["mukhomor.exe", "Install.ps1", "release-manifest.json"] {
        if !package.join(file).is_file() {
            return Err(INVALID.into());
        }
    }
    Ok(package)
}

#[cfg(target_os = "linux")]
fn unpack(archive: &Path, destination: &Path, release: &Release) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let decoder = flate2::read::GzDecoder::new(File::open(archive).map_err(|_| INVALID)?);
    let mut tar = tar::Archive::new(decoder);
    let mut total = 0u64;
    let mut count = 0;
    let prefix = format!("Mukhomor-{}", release.version);
    for entry in tar.entries().map_err(|_| INVALID)? {
        let mut entry = entry.map_err(|_| INVALID)?;
        count += 1;
        let name = entry
            .path()
            .map_err(|_| INVALID)?
            .to_str()
            .ok_or(INVALID)?
            .to_owned();
        let path = safe_entry(name.trim_end_matches('/'))?;
        let kind = entry.header().entry_type();
        if count > 4096
            || path
                .components()
                .next()
                .is_none_or(|c| c.as_os_str() != prefix.as_str())
            || !(kind.is_file() || kind.is_dir())
        {
            return Err(INVALID.into());
        }
        total = total
            .checked_add(entry.size())
            .filter(|n| *n <= MAX_EXPANDED)
            .ok_or(INVALID)?;
        let out = destination.join(path);
        if kind.is_dir() {
            fs::create_dir_all(out).map_err(|_| INVALID)?;
        } else {
            fs::create_dir_all(out.parent().ok_or(INVALID)?).map_err(|_| INVALID)?;
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(&out)
                .map_err(|_| INVALID)?;
            let size = entry.size();
            if std::io::copy(&mut (&mut entry).take(size + 1), &mut file).map_err(|_| INVALID)?
                != size
            {
                return Err(INVALID.into());
            }
            fs::set_permissions(out, fs::Permissions::from_mode(0o700)).map_err(|_| INVALID)?;
        }
    }
    let package = destination.join(prefix);
    for file in ["mukhomor", "mukhomor-controller", "install.sh"] {
        if !package.join(file).is_file() {
            return Err(INVALID.into());
        }
    }
    Ok(package)
}

fn install(
    release: &Release,
    root: &Path,
    sender: &mpsc::Sender<Event>,
) -> Result<Outcome, String> {
    // An offer cannot be changed by another check while busy. Never accept an
    // arbitrary URL or path from the UI or the VPN controller.
    if version(&release.version)? <= version(env!("CARGO_PKG_VERSION"))? {
        return Err(INVALID.into());
    }
    #[cfg(target_os = "android")]
    {
        // Ask before downloading so returning from Android settings does not
        // require downloading the same APK a second time.
        let permission = crate::android::call("prepareUpdate", "")?;
        if let Some(error) = permission.strip_prefix("ERROR:") {
            return Err(error.into());
        }
    }
    #[cfg(target_os = "android")]
    let stage = tempfile::Builder::new()
        .prefix("mukhomor-update-")
        .tempdir_in(crate::android::call("updateCache", "")?)
        .map_err(|_| "Не удалось создать папку обновления")?;
    #[cfg(not(target_os = "android"))]
    let stage = tempfile::Builder::new()
        .prefix("mukhomor-update-")
        .tempdir()
        .map_err(|_| "Не удалось создать папку обновления")?;
    let archive = stage.path().join("package");
    let mut response = get(&release.url)?;
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&archive)
        .map_err(|_| INVALID)?;
    copy_verified(
        response.body_mut().as_reader(),
        &mut file,
        release,
        |done, size| {
            let _ = sender.send(Event::Progress(done, size));
        },
    )?;
    file.sync_all().map_err(|_| INVALID)?;
    drop(file);
    let _ = sender.send(Event::Installing);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = root;
        let package = unpack(&archive, stage.path(), release)?;
        let status = std::process::Command::new(package.join("mukhomor.exe"))
            .current_dir(&package)
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .status()
            .map_err(|_| "Не удалось запустить установщик обновления")?;
        if !status.success() {
            return Err("Установка отменена или завершилась ошибкой. Повторите попытку.".into());
        }
        Ok(Outcome::Restarted)
    }
    #[cfg(target_os = "linux")]
    {
        let package = unpack(&archive, stage.path(), release)?;
        let status = std::process::Command::new("pkexec")
            .arg("/bin/bash")
            .arg(package.join("install.sh"))
            .output()
            .map_err(|_| "Для обновления требуется polkit (pkexec)")?;
        if !status.status.success() {
            return Err(
                "Установка отменена или завершилась ошибкой. Текущая версия сохранена.".into(),
            );
        }
        // The new process waits for this window's single-instance lock and its
        // controller to close; it never starts a second VPN session.
        std::process::Command::new("/opt/mukhomor/mukhomor")
            .arg("--after-update-pid")
            .arg(std::process::id().to_string())
            .arg("--root")
            .arg(root)
            .spawn()
            .map_err(|_| "Обновление установлено. Откройте Mukhomor заново.")?;
        Ok(Outcome::Restarted)
    }
    #[cfg(target_os = "android")]
    {
        let _ = root;
        // Java copies the verified bytes into private cache before returning;
        // Android's package installer owns consent and signature continuity.
        let result = crate::android::call("installUpdate", archive.to_str().ok_or(INVALID)?)?;
        if let Some(error) = result.strip_prefix("ERROR:") {
            return Err(error.into());
        }
        Ok(Outcome::AndroidInstaller)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture(tag: &str, platform: &str) -> Value {
        let name = format!("Mukhomor-{}-{platform}", tag.trim_start_matches('v'));
        json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":[{"name":name,"size":3,"state":"uploaded","digest":format!("sha256:{:x}",Sha256::digest(b"abc")),"browser_download_url":format!("{REPOSITORY}/releases/download/{tag}/{name}")}]})
    }
    #[test]
    fn offers_only_new_stable_matching_packages() {
        for platform in [
            "windows-x64.zip",
            "linux-x64.tar.gz",
            "linux-arm64.tar.gz",
            "android.apk",
        ] {
            let data = fixture("v0.8.10", platform);
            assert_eq!(
                select_release(&data, "0.8.2", platform)
                    .unwrap()
                    .unwrap()
                    .version,
                "0.8.10"
            );
            assert!(select_release(&data, "0.8.10", platform).unwrap().is_none());
            assert!(select_release(&data, "1.0.0", platform).unwrap().is_none());
            assert!(select_release(&data, "0.8.2", "other").is_err());
        }
        for tag in [
            "v0.8.3-rc.1",
            "v00.8.3",
            "v0.8",
            "v0.8.3/evil",
            "v4294967296.0.0",
        ] {
            assert!(select_release(&fixture(tag, "android.apk"), "0.8.2", "android.apk").is_err());
        }
        for field in ["draft", "prerelease"] {
            let mut data = fixture("v0.8.3", "android.apk");
            data[field] = json!(true);
            assert!(select_release(&data, "0.8.2", "android.apk").is_err());
        }
    }
    #[test]
    fn refuses_untrusted_and_incomplete_assets() {
        for (field, value) in [
            ("digest", json!(null)),
            ("digest", json!("sha256:bad")),
            ("size", json!(0)),
            ("size", json!(MAX_PACKAGE + 1)),
            ("state", json!("new")),
            ("browser_download_url", json!("https://example.com/app.apk")),
        ] {
            let mut data = fixture("v0.8.3", "android.apk");
            data["assets"][0][field] = value;
            assert!(
                select_release(&data, "0.8.2", "android.apk").is_err(),
                "{field}"
            );
        }
        let mut data = fixture("v0.8.3", "android.apk");
        let duplicate = data["assets"][0].clone();
        data["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(select_release(&data, "0.8.2", "android.apk").is_err());
    }
    #[test]
    fn bounds_and_verifies_download_before_installation() {
        let release = select_release(&fixture("v0.8.3", "android.apk"), "0.8.2", "android.apk")
            .unwrap()
            .unwrap();
        let mut out = Vec::new();
        let mut progress = Vec::new();
        copy_verified(&b"abc"[..], &mut out, &release, |a, b| {
            progress.push((a, b))
        })
        .unwrap();
        assert_eq!(out, b"abc");
        assert_eq!(progress, vec![(3, 3)]);
        for bad in [&b"ab"[..], &b"abcd"[..], &b"xyz"[..]] {
            assert!(copy_verified(bad, Vec::new(), &release, |_, _| {}).is_err());
        }
    }
    #[test]
    fn rejects_redirects_and_archive_path_escapes() {
        assert!(allowed_url(LATEST));
        assert!(allowed_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/file?token=test"
        ));
        for url in [
            "http://github.com/0nionow1/Mukhomor/releases/download/v1/a",
            "https://github.com.evil/0nionow1/Mukhomor/releases/download/v1/a",
            "https://github.com:444/0nionow1/Mukhomor/releases/download/v1/a",
            "https://user@github.com/0nionow1/Mukhomor/releases/download/v1/a",
            "https://github.com/another/project/releases/download/v1/a",
            "https://127.0.0.1/a",
        ] {
            assert!(!allowed_url(url), "{url}");
        }
        for name in [
            "../escape",
            "/absolute",
            "C:/path",
            "dir\\escape",
            "dir/../escape",
            "dir/./escape",
            "file:ads",
            "",
        ] {
            assert!(safe_entry(name).is_err(), "{name}");
        }
        assert!(safe_entry("Mukhomor/bin/app.exe").is_ok());
    }
    #[test]
    fn update_state_retains_offer_after_failure_and_blocks_duplicates() {
        let mut updater = Updater::default();
        let release = select_release(&fixture("v0.8.3", "android.apk"), "0.8.2", "android.apk")
            .unwrap()
            .unwrap();
        updater.busy = true;
        updater
            .sender
            .send(Event::Checked(Ok(Some(release.clone()))))
            .unwrap();
        assert!(matches!(updater.poll(), Some(Event::Checked(Ok(Some(_))))));
        assert!(!updater.busy);
        updater.busy = true;
        updater.installing = true;
        updater.check();
        updater.install(PathBuf::new());
        assert_eq!(updater.offer, Some(release.clone()));
        updater
            .sender
            .send(Event::Installed(Err("cancelled".into())))
            .unwrap();
        assert!(matches!(updater.poll(), Some(Event::Installed(Err(_)))));
        assert!(!updater.busy && !updater.installing);
        assert_eq!(updater.offer, Some(release));
    }
    #[cfg(windows)]
    #[test]
    fn extracts_complete_windows_package_and_rejects_escapes_links_and_duplicates() {
        use zip::write::SimpleFileOptions;
        for bad in ["", "../escape", "Mukhomor/link", "Mukhomor/mukhomor.exe"] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("test.zip");
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            for name in ["mukhomor.exe", "Install.ps1", "release-manifest.json"] {
                zip.start_file(format!("Mukhomor/{name}"), SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"fixture").unwrap();
            }
            if bad.ends_with("link") {
                zip.add_symlink(bad, "../../outside", SimpleFileOptions::default())
                    .unwrap();
            } else if !bad.is_empty() {
                // ZipWriter rejects duplicate names; use case-insensitive alias
                // to exercise the Windows create_new collision instead.
                let name = if bad.ends_with(".exe") {
                    "Mukhomor/MUKHOMOR.EXE"
                } else {
                    bad
                };
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"bad").unwrap();
            }
            zip.finish().unwrap();
            let release = select_release(
                &fixture("v0.8.3", "windows-x64.zip"),
                "0.8.2",
                "windows-x64.zip",
            )
            .unwrap()
            .unwrap();
            let result = unpack(&archive, &dir.path().join("out"), &release);
            assert_eq!(result.is_ok(), bad.is_empty(), "{bad}");
            assert!(!dir.path().join("escape").exists());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn extracts_complete_linux_package_and_rejects_links() {
        for link in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("test.tar.gz");
            let encoder = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(encoder);
            for name in ["mukhomor", "mukhomor-controller", "install.sh"] {
                let mut header = tar::Header::new_gnu();
                header.set_size(7);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("Mukhomor-0.8.3/{name}"),
                    &b"fixture"[..],
                )
                .unwrap();
            }
            if link {
                let mut header = tar::Header::new_gnu();
                header.set_size(0);
                header.set_mode(0o777);
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("../../outside").unwrap();
                header.set_cksum();
                tar.append_data(&mut header, "Mukhomor-0.8.3/link", std::io::empty())
                    .unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
            let release = select_release(
                &fixture("v0.8.3", "linux-x64.tar.gz"),
                "0.8.2",
                "linux-x64.tar.gz",
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                unpack(&archive, &dir.path().join("out"), &release).is_ok(),
                !link
            );
        }
    }
    #[test]
    #[ignore = "explicit network check against the public GitHub release"]
    fn public_release_contract() {
        let mut response = get(LATEST).unwrap();
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(2 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .unwrap();
        let data: Value = serde_json::from_slice(&bytes).unwrap();
        for platform in ["windows-x64.zip", "linux-x64.tar.gz", "android.apk"] {
            assert!(select_release(&data, "0.0.0", platform).unwrap().is_some());
        }
        // Exercise the real GitHub -> release-assets redirect and streaming
        // digest verification with a tiny checksum asset, never an installer.
        let tag = data["tag_name"].as_str().unwrap();
        let name = format!(
            "Mukhomor-{}-windows-x64.zip.sha256",
            tag.trim_start_matches('v')
        );
        let asset = data["assets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap();
        let release = Release {
            version: tag[1..].into(),
            url: format!("{REPOSITORY}/releases/download/{tag}/{name}"),
            size: asset["size"].as_u64().unwrap(),
            sha256: asset["digest"]
                .as_str()
                .unwrap()
                .strip_prefix("sha256:")
                .unwrap()
                .into(),
        };
        assert!(release.size < 1024);
        let mut response = get(&release.url).unwrap();
        let mut bytes = Vec::new();
        copy_verified(
            response.body_mut().as_reader(),
            &mut bytes,
            &release,
            |_, _| {},
        )
        .unwrap();
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("windows-x64.zip")
        );
    }
}
