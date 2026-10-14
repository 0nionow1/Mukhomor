//! Per-user Windows logon registration. Never register the downloaded EXE.
use crate::platform::wide;
use std::{
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
const NAME: &str = "Mukhomor";
const ERROR: &str = "Не удалось изменить запуск с Windows. Проверь доступ к настройкам Windows.";
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
struct Store<'a> {
    key: &'a str,
}
impl Store<'_> {
    // Advisory read only: Windows owns this value. Never create, reset or delete
    // its approval state, including on upgrade or removal.
    fn disabled_by_windows(&self) -> Result<bool, String> {
        let Some(key) = self.open(false)? else {
            return Ok(false);
        };
        let mut kind = 0;
        let mut size = 12;
        let mut buffer = [0u8; 12];
        let result = unsafe {
            RegQueryValueExW(
                key.0,
                wide(NAME).as_ptr(),
                null(),
                &mut kind,
                buffer.as_mut_ptr(),
                &mut size,
            )
        };
        if result == ERROR_FILE_NOT_FOUND || result == ERROR_MORE_DATA {
            return Ok(false);
        }
        if result != ERROR_SUCCESS {
            return Err(ERROR.into());
        }
        Ok(kind == REG_BINARY
            && size == 12
            && buffer[1..4] == [0, 0, 0]
            && matches!(buffer[0], 3 | 7))
    }
    fn open(&self, write: bool) -> Result<Option<Key>, String> {
        let mut key = null_mut();
        let result = unsafe {
            if write {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(self.key).as_ptr(),
                    0,
                    null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    null(),
                    &mut key,
                    null_mut(),
                )
            } else {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    wide(self.key).as_ptr(),
                    0,
                    KEY_QUERY_VALUE,
                    &mut key,
                )
            }
        };
        match result {
            ERROR_SUCCESS => Ok(Some(Key(key))),
            ERROR_FILE_NOT_FOUND if !write => Ok(None),
            _ => Err(ERROR.into()),
        }
    }
    fn read(&self) -> Result<Option<String>, String> {
        let Some(key) = self.open(false)? else {
            return Ok(None);
        };
        let mut kind = 0;
        let mut size = 0;
        let result = unsafe {
            RegQueryValueExW(
                key.0,
                wide(NAME).as_ptr(),
                null(),
                &mut kind,
                null_mut(),
                &mut size,
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if result != ERROR_SUCCESS || kind != REG_SZ || size > 4096 || size % 2 != 0 {
            return Err(ERROR.into());
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        if unsafe {
            RegQueryValueExW(
                key.0,
                wide(NAME).as_ptr(),
                null(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        } != ERROR_SUCCESS
            || kind != REG_SZ
        {
            return Err(ERROR.into());
        }
        let length = buffer.iter().position(|x| *x == 0).unwrap_or(buffer.len());
        String::from_utf16(&buffer[..length])
            .map(Some)
            .map_err(|_| ERROR.into())
    }
    fn set(&self, executable: &Path, enabled: bool) -> Result<(), String> {
        let command = command(executable)?;
        if self
            .read()?
            .is_some_and(|old| !owned_command(&old, executable))
        {
            return Err(
                "Запись автозапуска Mukhomor занята другой командой. Изменение отменено.".into(),
            );
        }
        let Some(key) = self.open(true)? else {
            return Err(ERROR.into());
        };
        let result = unsafe {
            if enabled {
                let value = wide(&command);
                RegSetValueExW(
                    key.0,
                    wide(NAME).as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            } else {
                RegDeleteValueW(key.0, wide(NAME).as_ptr())
            }
        };
        if result != ERROR_SUCCESS && !(result == ERROR_FILE_NOT_FOUND && !enabled) {
            return Err(ERROR.into());
        }
        if self.read()? != enabled.then_some(command) {
            return Err(ERROR.into());
        }
        Ok(())
    }
}
pub fn command(executable: &Path) -> Result<String, String> {
    let path = executable.to_str().ok_or(ERROR)?;
    if !executable.is_absolute()
        || path.contains(['"', '\0', '\r', '\n'])
        || executable.file_name().and_then(|p| p.to_str()) != Some("mukhomor.exe")
    {
        return Err(ERROR.into());
    }
    let command = format!("\"{path}\" --startup");
    if command.encode_utf16().count() > 260 {
        return Err("Путь Mukhomor слишком длинный для запуска с Windows.".into());
    }
    Ok(command)
}
fn version_directory(name: &str) -> bool {
    let Some((version, id)) = name.strip_prefix("app-").and_then(|s| s.rsplit_once('-')) else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && version.split('.').count() == 3
        && version
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
fn owned_command(value: &str, executable: &Path) -> bool {
    let Some(path) = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix("\" --startup"))
    else {
        return false;
    };
    let candidate = Path::new(path);
    candidate.is_absolute()
        && !path.contains(['"', '\0', '\r', '\n'])
        && candidate.file_name().and_then(|s| s.to_str()) == Some("mukhomor.exe")
        && candidate
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(version_directory)
        && candidate
            .parent()
            .and_then(Path::parent)
            .zip(executable.parent().and_then(Path::parent))
            .is_some_and(|(a, b)| {
                a.to_string_lossy()
                    .eq_ignore_ascii_case(&b.to_string_lossy())
            })
}
pub fn enabled(executable: &Path) -> Result<bool, String> {
    Ok(Store { key: RUN }
        .read()?
        .is_some_and(|value| owned_command(&value, executable))
        && !Store { key: APPROVED }.disabled_by_windows()?)
}
pub fn set(executable: &Path, enabled: bool) -> Result<(), String> {
    Store { key: RUN }.set(executable, enabled)?;
    if enabled && (Store { key: APPROVED }).disabled_by_windows()? {
        return Err("Windows отключила автозапуск Mukhomor. Включи Mukhomor в «Параметры Windows → Приложения → Автозагрузка».".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn executable(version: &str) -> std::path::PathBuf {
        Path::new("C:\\Program Files\\Mukhomor")
            .join(format!("app-{version}-{}", "a".repeat(32)))
            .join("mukhomor.exe")
    }
    #[test]
    fn commands_are_quoted_bounded_and_only_our_installed_versions_are_owned() {
        let exe = executable("0.6.0");
        let value = command(&exe).unwrap();
        assert_eq!(value, format!("\"{}\" --startup", exe.display()));
        assert!(owned_command(&value, &exe));
        assert!(owned_command(&command(&executable("0.5.0")).unwrap(), &exe));
        assert!(!owned_command(
            "\"C:\\Downloads\\mukhomor.exe\" --startup",
            &exe
        ));
        assert!(!owned_command(&(value.clone() + " --smoke"), &exe));
        assert!(command(Path::new("mukhomor.exe")).is_err());
        assert!(command(&Path::new("C:\\").join("x".repeat(260)).join("mukhomor.exe")).is_err());
    }
    #[test]
    fn isolated_registry_registration_roundtrips_upgrade_disable_and_collision() {
        let name = format!(
            "Software\\Mukhomor\\Tests\\autostart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let store = Store { key: &name };
        let exe = executable("0.6.0");
        let result = std::panic::catch_unwind(|| {
            assert_eq!(store.read().unwrap(), None);
            store.set(&exe, true).unwrap();
            assert_eq!(store.read().unwrap(), Some(command(&exe).unwrap()));
            let next = executable("0.6.1");
            store.set(&next, true).unwrap();
            assert_eq!(store.read().unwrap(), Some(command(&next).unwrap()));
            store.set(&next, false).unwrap();
            store.set(&next, false).unwrap();
            assert_eq!(store.read().unwrap(), None);
            let key = store.open(true).unwrap().unwrap();
            let foreign = wide("foreign.exe");
            unsafe {
                assert_eq!(
                    RegSetValueExW(
                        key.0,
                        wide(NAME).as_ptr(),
                        0,
                        REG_SZ,
                        foreign.as_ptr().cast(),
                        (foreign.len() * 2) as u32
                    ),
                    ERROR_SUCCESS
                );
            }
            assert!(store.set(&next, true).is_err());
            assert!(store.set(&next, false).is_err());
            assert_eq!(store.read().unwrap(), Some("foreign.exe".into()));
            // Exercise advisory approval parsing only inside this isolated key.
            for (state, disabled) in [(2, false), (3, true), (6, false), (7, true), (42, false)] {
                let mut value = [0u8; 12];
                value[0] = state;
                unsafe {
                    assert_eq!(
                        RegSetValueExW(
                            key.0,
                            wide(NAME).as_ptr(),
                            0,
                            REG_BINARY,
                            value.as_ptr(),
                            12
                        ),
                        ERROR_SUCCESS
                    );
                }
                assert_eq!(store.disabled_by_windows().unwrap(), disabled);
                let mut after = [0u8; 12];
                let mut size = 12;
                let mut kind = 0;
                unsafe {
                    assert_eq!(
                        RegQueryValueExW(
                            key.0,
                            wide(NAME).as_ptr(),
                            null(),
                            &mut kind,
                            after.as_mut_ptr(),
                            &mut size
                        ),
                        ERROR_SUCCESS
                    );
                }
                assert_eq!(after, value);
                assert_eq!(kind, REG_BINARY);
            }
            let opaque = [3u8; 20];
            unsafe {
                assert_eq!(
                    RegSetValueExW(
                        key.0,
                        wide(NAME).as_ptr(),
                        0,
                        REG_BINARY,
                        opaque.as_ptr(),
                        20
                    ),
                    ERROR_SUCCESS
                );
            }
            assert!(!store.disabled_by_windows().unwrap());
        });
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&name).as_ptr());
        }
        result.unwrap();
    }
}
