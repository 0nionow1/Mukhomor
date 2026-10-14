//! Derive the owned TUN identity through the pinned Wintun library, never from
//! a caller-selected interface number. Opening an existing adapter does not
//! create one, change its addresses, install a driver, or start a packet session.
use std::{ffi::c_void, io::Read, path::Path, ptr::null};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    NetworkManagement::{
        IpHelper::{GetIfEntry2, MIB_IF_ROW2},
        Ndis::{IfOperStatusUp, NET_IF_ADMIN_STATUS_UP, NET_LUID_LH},
    },
    Security::Cryptography::*,
    System::LibraryLoader::*,
};

const WINTUN_SHA256: &str = "e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce";
const DLL_LIMIT: u64 = 4 * 1024 * 1024;
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
struct Algorithm(BCRYPT_ALG_HANDLE);
impl Drop for Algorithm {
    fn drop(&mut self) {
        unsafe {
            BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}
fn verify_bytes(bytes: &[u8]) -> Result<(), String> {
    let mut handle = std::ptr::null_mut();
    unsafe {
        if BCryptOpenAlgorithmProvider(&mut handle, BCRYPT_SHA256_ALGORITHM, null(), 0) < 0 {
            return Err("WFP_WINTUN_HASH_PROVIDER".into());
        }
        let handle = Algorithm(handle);
        let mut digest = [0u8; 32];
        if BCryptHash(
            handle.0,
            null(),
            0,
            bytes.as_ptr(),
            bytes.len() as u32,
            digest.as_mut_ptr(),
            32,
        ) < 0
        {
            return Err("WFP_WINTUN_HASH".into());
        }
        let encoded: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if encoded != WINTUN_SHA256 {
            return Err("WFP_WINTUN_INTEGRITY".into());
        }
    }
    Ok(())
}
pub fn resolve(base: &Path) -> Result<u64, String> {
    if !base.is_absolute() {
        return Err("WFP_BUNDLE_PATH".into());
    }
    let path = base.join("bin").join("wintun.dll");
    // Hold a non-write/non-delete-sharing handle through LoadLibrary. This also
    // prevents replacing the pinned image between verification and execution.
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|_| "WFP_WINTUN_MISSING")?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(DLL_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "WFP_WINTUN_READ")?;
    if bytes.len() as u64 > DLL_LIMIT {
        return Err("WFP_WINTUN_SIZE".into());
    }
    verify_bytes(&bytes)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let h = LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if h.is_null() {
            return Err("WFP_WINTUN_LOAD".into());
        }
        let library = Library(h);
        let open = GetProcAddress(library.0, c"WintunOpenAdapter".as_ptr().cast())
            .ok_or("WFP_WINTUN_API")?;
        let get_luid = GetProcAddress(library.0, c"WintunGetAdapterLUID".as_ptr().cast())
            .ok_or("WFP_WINTUN_API")?;
        let close = GetProcAddress(library.0, c"WintunCloseAdapter".as_ptr().cast())
            .ok_or("WFP_WINTUN_API")?;
        let open: unsafe extern "system" fn(*const u16) -> *mut c_void = std::mem::transmute(open);
        let get_luid: unsafe extern "system" fn(*mut c_void, *mut u64) =
            std::mem::transmute(get_luid);
        let close: unsafe extern "system" fn(*mut c_void) = std::mem::transmute(close);
        let name: Vec<u16> = "Mukhomor".encode_utf16().chain(Some(0)).collect();
        let adapter = open(name.as_ptr());
        if adapter.is_null() {
            return Err("WFP_OWNED_TUN_MISSING".into());
        }
        let mut luid = 0;
        get_luid(adapter, &mut luid);
        close(adapter);
        if luid == 0 {
            return Err("WFP_OWNED_TUN_LUID".into());
        }
        Ok(luid)
    }
}
fn operational_identity(row: &MIB_IF_ROW2, expected: u64) -> bool {
    expected != 0
        && unsafe { row.InterfaceLuid.Value } == expected
        && row.AdminStatus == NET_IF_ADMIN_STATUS_UP
        && row.OperStatus == IfOperStatusUp
}
/// An existing adapter is not sufficient: its exact kernel identity must also
/// be administratively enabled and operational. Errors never confirm activity.
pub fn active(base: &Path, expected: u64) -> Result<bool, String> {
    if expected == 0 || resolve(base)? != expected {
        return Ok(false);
    }
    let mut row = MIB_IF_ROW2 {
        InterfaceLuid: NET_LUID_LH { Value: expected },
        ..Default::default()
    };
    if unsafe { GetIfEntry2(&mut row) } != 0 {
        return Err("WFP_OWNED_TUN_STATE".into());
    }
    Ok(operational_identity(&row, expected))
}
#[cfg(test)]
mod tests {
    #[test]
    fn forged_library_is_rejected_without_loading_it() {
        assert_eq!(
            super::verify_bytes(b"not a Wintun image").unwrap_err(),
            "WFP_WINTUN_INTEGRITY"
        );
    }
    #[test]
    fn only_the_exact_enabled_operational_interface_is_active() {
        use super::*;
        use windows_sys::Win32::NetworkManagement::Ndis::*;
        let live = MIB_IF_ROW2 {
            InterfaceLuid: NET_LUID_LH { Value: 0x1234 },
            AdminStatus: NET_IF_ADMIN_STATUS_UP,
            OperStatus: IfOperStatusUp,
            ..Default::default()
        };
        assert!(operational_identity(&live, 0x1234));
        assert!(!operational_identity(&live, 0));
        assert!(!operational_identity(&live, 0x5678));
        for state in [NET_IF_ADMIN_STATUS_DOWN, NET_IF_ADMIN_STATUS_TESTING, 0] {
            assert!(!operational_identity(
                &MIB_IF_ROW2 {
                    AdminStatus: state,
                    ..live
                },
                0x1234
            ));
        }
        for state in [
            IfOperStatusDown,
            IfOperStatusDormant,
            IfOperStatusLowerLayerDown,
            IfOperStatusNotPresent,
            IfOperStatusTesting,
            IfOperStatusUnknown,
            0,
        ] {
            assert!(!operational_identity(
                &MIB_IF_ROW2 {
                    OperStatus: state,
                    ..live
                },
                0x1234
            ));
        }
    }
}
