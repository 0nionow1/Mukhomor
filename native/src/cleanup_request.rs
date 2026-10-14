//! Durable cancellation before an explicit restart of the installed service.
//! The installation owner can write only this fixed request directory; runtime
//! intent and credentials retain their existing privileged permissions.
use super::recovery::{handle_security, pin};
use crate::platform::wide;
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
};
const DIRECTORY: &str = "recovery-request";
const MARKER: &str = "disconnect.request";
const CANCEL: &[u8] = b"disconnect";
const DIRECTORY_OWNER_MASK: u32 = FILE_GENERIC_READ | FILE_ADD_FILE | FILE_DELETE_CHILD;
const FILE_OWNER_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE;
const WRITE_MASK: u32 = 0x000d_0156 | GENERIC_ALL | GENERIC_WRITE;

struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Directory {
    _root: File,
    _owner_file: File,
    handle: File,
    path: PathBuf,
    owner: Sid,
}
pub(crate) struct Pending {
    _directory: Directory,
    file: File,
}
fn trusted(sid: PSID, owner: PSID) -> bool {
    !sid.is_null()
        && unsafe {
            IsValidSid(sid) != 0
                && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                    || EqualSid(sid, owner) != 0)
        }
}
fn request_security(handle: HANDLE, owner: PSID, directory: bool) -> Result<(), String> {
    let mut sd = null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    } != 0
    {
        return Err("CLEANUP_DIRECTORY_SECURITY".into());
    }
    let result = (|| unsafe {
        let mut sid = null_mut();
        let mut acl = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut control = 0;
        let mut revision = 0;
        if IsValidSecurityDescriptor(sd) == 0
            || GetSecurityDescriptorOwner(sd, &mut sid, &mut defaulted) == 0
            || !trusted(sid, owner)
            || GetSecurityDescriptorControl(sd, &mut control, &mut revision) == 0
            || (directory && control & SE_DACL_PROTECTED == 0)
            || GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0
            || present == 0
            || acl.is_null()
        {
            return Err("CLEANUP_DIRECTORY_SECURITY".into());
        }
        let mut info = ACL_SIZE_INFORMATION::default();
        if GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
            AclSizeInformation,
        ) == 0
            || info.AceCount != if directory { 4 } else { 3 }
        {
            return Err("CLEANUP_DIRECTORY_SECURITY".into());
        }
        let mut seen = [false; 4];
        for index in 0..info.AceCount {
            let mut raw = null_mut();
            if GetAce(acl, index, &mut raw) == 0 || raw.is_null() {
                return Err("CLEANUP_DIRECTORY_SECURITY".into());
            }
            let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
            if ace.Header.AceType != 0
                || usize::from(ace.Header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            {
                return Err("CLEANUP_DIRECTORY_SECURITY".into());
            }
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            let flags = u32::from(ace.Header.AceFlags);
            let trusted_flags = if directory {
                flags == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                flags == 0 || flags == INHERITED_ACE
            };
            let slot = if IsWellKnownSid(sid, WinLocalSystemSid) != 0
                && ace.Mask == FILE_ALL_ACCESS
                && trusted_flags
            {
                0
            } else if IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                && ace.Mask == FILE_ALL_ACCESS
                && trusted_flags
            {
                1
            } else if directory
                && EqualSid(sid, owner) != 0
                && ace.Mask == DIRECTORY_OWNER_MASK
                && flags == 0
            {
                2
            } else if directory
                && EqualSid(sid, owner) != 0
                && ace.Mask == FILE_OWNER_MASK
                && flags == OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE
            {
                3
            } else if !directory
                && EqualSid(sid, owner) != 0
                && ace.Mask == FILE_OWNER_MASK
                && (flags == 0 || flags == INHERITED_ACE)
            {
                2
            } else {
                return Err("CLEANUP_DIRECTORY_SECURITY".into());
            };
            if seen[slot] {
                return Err("CLEANUP_DIRECTORY_SECURITY".into());
            }
            seen[slot] = true;
        }
        Ok(())
    })();
    unsafe {
        LocalFree(sd);
    }
    result
}
fn directory(root: &Path, required: bool) -> Result<Option<Directory>, String> {
    let path = root.join(DIRECTORY);
    // Absence preserves old ordinary installations; an explicit restart must
    // have the installer-created directory and cannot create/adopt one itself.
    let handle = match OpenOptions::new()
        .read(true)
        // LIST_DIRECTORY counts as data read for sharing checks, excluding a
        // WRITE_DATA handle that could turn this directory into a reparse point.
        .access_mode(FILE_GENERIC_READ)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => return Ok(None),
        Err(_) => return Err("CLEANUP_DIRECTORY_OPEN".into()),
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err("CLEANUP_DIRECTORY_IDENTITY".into());
    }
    let mut bytes = Vec::new();
    let mut owner_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(root.join("owner.sid"))
        .map_err(|_| "CLEANUP_OWNER")?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(owner_file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || info.nNumberOfLinks != 1
    {
        return Err("CLEANUP_OWNER".into());
    }
    (&mut owner_file)
        .take(513)
        .read_to_end(&mut bytes)
        .map_err(|_| "CLEANUP_OWNER")?;
    if bytes.len() > 512 {
        return Err("CLEANUP_OWNER".into());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "CLEANUP_OWNER")?
        .trim();
    if !text.starts_with("S-1-")
        || text
            .bytes()
            .any(|b| !b.is_ascii_digit() && b != b'S' && b != b'-')
    {
        return Err("CLEANUP_OWNER".into());
    }
    let mut sid = null_mut();
    if unsafe { ConvertStringSidToSidW(wide(text).as_ptr(), &mut sid) } == 0 {
        return Err("CLEANUP_OWNER".into());
    }
    let owner = Sid(sid);
    handle_security(
        owner_file.as_raw_handle(),
        SE_FILE_OBJECT,
        Some(owner.0),
        WRITE_MASK,
    )?;
    let _root = pin(root, true, Some(owner.0))?;
    request_security(handle.as_raw_handle(), owner.0, true)?;
    Ok(Some(Directory {
        _root,
        _owner_file: owner_file,
        handle,
        path,
        owner,
    }))
}
fn marker_file(directory: &Directory, delete: bool) -> Result<Option<File>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .access_mode(FILE_GENERIC_READ | if delete { DELETE } else { 0 })
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(directory.path.join(MARKER))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("CLEANUP_MARKER_OPEN".into()),
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || info.nNumberOfLinks != 1
    {
        return Err("CLEANUP_MARKER_IDENTITY".into());
    }
    request_security(file.as_raw_handle(), directory.owner.0, false)?;
    let mut bytes = Vec::new();
    (&file)
        .take(CANCEL.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "CLEANUP_MARKER_READ")?;
    if bytes != CANCEL {
        return Err("CLEANUP_MARKER_CONTENT".into());
    }
    Ok(Some(file))
}
pub(crate) fn pending(root: &Path) -> Result<Option<Pending>, String> {
    let Some(directory) = directory(root, false)? else {
        return Ok(None);
    };
    let Some(file) = marker_file(&directory, true)? else {
        return Ok(None);
    };
    Ok(Some(Pending {
        _directory: directory,
        file,
    }))
}
impl Pending {
    pub(crate) fn consume(self) -> Result<(), String> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&disposition) as u32,
            )
        } == 0
        {
            return Err("CLEANUP_MARKER_DELETE".into());
        }
        // Exact opened inode, not another file swapped at a path. A surviving
        // marker after a power loss only repeats the already-durable cancel.
        Ok(())
    }
}
fn validate_intent(root: &Path, owner: PSID) -> Result<(), String> {
    let runtime = pin(&root.join("runtime"), true, Some(owner))?;
    let path = root.join("runtime/connection-intent.json");
    let file = match pin(&path, false, Some(owner)) {
        Ok(file) => file,
        Err(_) if !path.try_exists().map_err(|_| "CLEANUP_INTENT_READ")? => return Ok(()),
        Err(_) => return Err("CLEANUP_INTENT_READ".into()),
    };
    let mut bytes = Vec::new();
    (&file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "CLEANUP_INTENT_READ")?;
    if bytes.len() > 4096 {
        return Err("CLEANUP_INTENT_SIZE".into());
    }
    let data: Value = crate::traffic_guard_cli::parse(&bytes).map_err(|_| "CLEANUP_INTENT_JSON")?;
    if data.as_object().is_none_or(|o| o.len() != 3)
        || data["schema"] != 1
        || data["generation"].as_u64().is_none()
        || data["desired"].as_bool().is_none()
    {
        return Err("CLEANUP_INTENT_JSON".into());
    }
    drop(runtime);
    Ok(())
}
pub(crate) fn publish(root: &Path) -> Result<(), String> {
    let directory = directory(root, true)?.ok_or("CLEANUP_DIRECTORY_OPEN")?;
    validate_intent(root, directory.owner.0)?;
    let path = directory.path.join(MARKER);
    let options = || {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
        options
    };
    let mut file = match options().create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists =>
        // No truncate/create-on-existing before identity and exact ACL.
        {
            options().open(&path).map_err(|_| "CLEANUP_MARKER_OPEN")?
        }
        Err(_) => return Err("CLEANUP_MARKER_CREATE".into()),
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || info.nNumberOfLinks != 1
    {
        return Err("CLEANUP_MARKER_IDENTITY".into());
    }
    request_security(file.as_raw_handle(), directory.owner.0, false)?;
    let mut bytes = Vec::new();
    (&file)
        .take(CANCEL.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "CLEANUP_MARKER_READ")?;
    if bytes != CANCEL {
        // Only explicit checked cleanup repairs this fixed ordinary owned
        // object. Startup and Connect still reject partial/malformed bytes.
        file.rewind().map_err(|_| "CLEANUP_MARKER_WRITE")?;
        file.write_all(CANCEL).map_err(|_| "CLEANUP_MARKER_WRITE")?;
        file.set_len(CANCEL.len() as u64)
            .map_err(|_| "CLEANUP_MARKER_WRITE")?;
    }
    // No rename while the strong directory pin excludes reparse writers.
    // A crash during a partial write leaves a request that fails closed.
    file.sync_all().map_err(|_| "CLEANUP_MARKER_SYNC")?;
    request_security(directory.handle.as_raw_handle(), directory.owner.0, true)?;
    request_security(file.as_raw_handle(), directory.owner.0, false)?;
    file.rewind().map_err(|_| "CLEANUP_MARKER_READ")?;
    let mut verified = Vec::new();
    (&file)
        .take(CANCEL.len() as u64 + 1)
        .read_to_end(&mut verified)
        .map_err(|_| "CLEANUP_MARKER_READ")?;
    if verified != CANCEL {
        return Err("CLEANUP_MARKER_CONTENT".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(crate) struct Tree {
        pub(crate) root: PathBuf,
        pub(crate) owner: String,
    }
    pub(crate) fn set_acl(path: &Path, sddl: &str) {
        let mut sd = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide(sddl).as_ptr(),
                    1,
                    &mut sd,
                    null_mut(),
                )
            },
            0
        );
        let mut acl = null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) },
            0
        );
        let code = unsafe {
            SetNamedSecurityInfoW(
                wide(&path.to_string_lossy()).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null_mut(),
            )
        };
        unsafe {
            LocalFree(sd);
        }
        assert_eq!(code, 0, "Cannot set the temporary fixture DACL");
    }
    impl Tree {
        pub(crate) fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("tests")
                .join(format!(
                    "recovery-request-{}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir(&root).unwrap();
            let owner = crate::platform::user_sid().unwrap();
            set_acl(
                &root,
                &format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{owner})"),
            );
            std::fs::write(root.join("owner.sid"), &owner).unwrap();
            std::fs::create_dir(root.join("runtime")).unwrap();
            std::fs::write(
                root.join("runtime/connection-intent.json"),
                br#"{"schema":1,"generation":7,"desired":true}"#,
            )
            .unwrap();
            std::fs::create_dir(root.join(DIRECTORY)).unwrap();
            let tree = Self { root, owner };
            tree.restore();
            tree
        }
        pub(crate) fn restore(&self) {
            set_acl(
                &self.root.join(DIRECTORY),
                &format!(
                    "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200cb;;;{})(A;OIIO;0x13019f;;;{})",
                    self.owner, self.owner
                ),
            );
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            // Only fixed files and ordinary directories created by this fixture;
            // no recursive walk or deletion of computed/outside paths.
            for path in [
                "owner.sid",
                "runtime/connection-intent.json",
                "outside.txt",
                "recovery-request/disconnect.request",
            ] {
                let _ = std::fs::remove_file(self.root.join(path));
            }
            let _ = std::fs::remove_dir(self.root.join("runtime/connection-intent.json.pending"));
            let _ = std::fs::remove_dir(self.root.join("recovery-request/disconnect.request"));
            let _ = std::fs::remove_dir(self.root.join("recovery-request"));
            let _ = std::fs::remove_dir(self.root.join("runtime"));
            let _ = std::fs::remove_dir(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};
    #[test]
    fn durable_cancellation_publishes_before_service_and_consumes_exact_file() {
        let tree = Tree::new();
        assert!(pending(&tree.root).unwrap().is_none());
        publish(&tree.root).unwrap();
        assert_eq!(
            std::fs::read(tree.root.join(DIRECTORY).join(MARKER)).unwrap(),
            CANCEL
        );
        assert!(
            crate::traffic_guard_cli::parse(
                &std::fs::read(tree.root.join("runtime/connection-intent.json")).unwrap()
            )
            .unwrap()["desired"]
                == true
        );
        // Idempotent new publication preserves cancellation across GUI crashes.
        publish(&tree.root).unwrap();
        let request = pending(&tree.root).unwrap().unwrap();
        assert!(
            OpenOptions::new()
                .write(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(tree.root.join(DIRECTORY).join(MARKER))
                .is_err()
        );
        // Production consumes only after persisting false; the marker primitive
        // itself deliberately cannot mutate SYSTEM's separate runtime state.
        request.consume().unwrap();
        assert!(pending(&tree.root).unwrap().is_none());
    }
    #[test]
    fn only_explicit_recovery_repairs_partial_marker_and_rejects_hardlinks_or_writers() {
        let tree = Tree::new();
        let marker = tree.root.join(DIRECTORY).join(MARKER);
        for bytes in [
            &b""[..],
            &b"stop"[..],
            &b"disconnect\n"[..],
            &b"disconnectX"[..],
            &[b'x'; 128][..],
        ] {
            std::fs::write(&marker, bytes).unwrap();
            assert!(pending(&tree.root).is_err());
            assert_eq!(std::fs::read(&marker).unwrap(), bytes);
            publish(&tree.root).unwrap();
            assert_eq!(std::fs::read(&marker).unwrap(), CANCEL);
        }
        std::fs::remove_file(&marker).unwrap();
        let outside = tree.root.join("outside.txt");
        std::fs::write(&outside, CANCEL).unwrap();
        std::fs::hard_link(&outside, &marker).unwrap();
        assert!(pending(&tree.root).is_err());
        assert!(publish(&tree.root).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), CANCEL);
        std::fs::remove_file(&marker).unwrap();
        publish(&tree.root).unwrap();
        let writer = OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&marker)
            .unwrap();
        assert!(pending(&tree.root).is_err());
        assert!(publish(&tree.root).is_err());
        drop(writer);
        assert!(pending(&tree.root).unwrap().is_some());
    }
    #[test]
    fn directory_pin_excludes_reparse_writers_and_preserves_fixed_acl_before_repair() {
        let tree = Tree::new();
        let path = tree.root.join(DIRECTORY);
        let writer = OpenOptions::new()
            .write(true)
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)
            .unwrap();
        assert!(pending(&tree.root).is_err());
        assert!(publish(&tree.root).is_err());
        drop(writer);
        let pin = directory(&tree.root, false).unwrap().unwrap();
        assert!(
            OpenOptions::new()
                .write(true)
                .access_mode(FILE_WRITE_DATA)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .open(&path)
                .is_err()
        );
        // Child create/write works while the ordinary directory cannot acquire
        // the WRITE_DATA handle needed to alter its reparse identity.
        publish(&tree.root).unwrap();
        drop(pin);
        let marker = path.join(MARKER);
        std::fs::write(&marker, b"partial").unwrap();
        set_acl(&marker, "D:P(A;;FA;;;WD)");
        assert!(publish(&tree.root).is_err());
        assert_eq!(std::fs::read(&marker).unwrap(), b"partial");
        set_acl(
            &marker,
            &format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x13019f;;;{})", tree.owner),
        );
        let sibling = path.join("unknown.txt");
        std::fs::write(&sibling, b"unknown").unwrap();
        publish(&tree.root).unwrap();
        assert_eq!(std::fs::read(&marker).unwrap(), CANCEL);
        assert_eq!(std::fs::read(&sibling).unwrap(), b"unknown");
        std::fs::remove_file(sibling).unwrap();
    }
    #[test]
    fn actual_directory_reparse_is_rejected_before_outside_marker_read_or_consume() {
        let tree = Tree::new();
        let source = tree.root.join(DIRECTORY);
        let outside = tree.root.join("outside-directory");
        std::fs::create_dir(&outside).unwrap();
        set_acl(
            &outside,
            &format!(
                "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200cb;;;{})(A;OIIO;0x13019f;;;{})",
                tree.owner, tree.owner
            ),
        );
        let outside_marker = outside.join(MARKER);
        std::fs::write(&outside_marker, CANCEL).unwrap();
        let substitute = wide(&format!("\\??\\{}", outside.display()));
        let print = wide(&outside.to_string_lossy());
        let mut buffer = vec![0u8; 16 + 2 * (substitute.len() + print.len())];
        buffer[0..4].copy_from_slice(&0xa0000003u32.to_le_bytes()); // mount point
        let data_length = (buffer.len() - 8) as u16;
        buffer[4..6].copy_from_slice(&data_length.to_le_bytes());
        buffer[10..12].copy_from_slice(&((substitute.len() as u16 - 1) * 2).to_le_bytes());
        buffer[12..14].copy_from_slice(&(substitute.len() as u16 * 2).to_le_bytes());
        buffer[14..16].copy_from_slice(&((print.len() as u16 - 1) * 2).to_le_bytes());
        for (index, value) in substitute.iter().chain(print.iter()).enumerate() {
            buffer[16 + index * 2..18 + index * 2].copy_from_slice(&value.to_le_bytes());
        }
        let writer = OpenOptions::new()
            .write(true)
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&source)
            .unwrap();
        let mut returned = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::IO::DeviceIoControl(
                    writer.as_raw_handle(),
                    0x000900a4,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            },
            0,
            "Temporary junction fixture creation failed: {}",
            std::io::Error::last_os_error()
        );
        drop(writer);
        assert_eq!(
            pending(&tree.root).err().unwrap(),
            "CLEANUP_DIRECTORY_IDENTITY"
        );
        assert_eq!(
            publish(&tree.root).unwrap_err(),
            "CLEANUP_DIRECTORY_IDENTITY"
        );
        assert_eq!(std::fs::read(&outside_marker).unwrap(), CANCEL);
        // Remove only the junction object, never recurse into its target.
        std::fs::remove_dir(&source).unwrap();
        assert_eq!(std::fs::read(&outside_marker).unwrap(), CANCEL);
        std::fs::remove_file(outside_marker).unwrap();
        std::fs::remove_dir(outside).unwrap();
    }
    #[test]
    fn broad_directory_acl_or_corrupt_intent_blocks_restart_cancellation_publication() {
        let tree = Tree::new();
        let marker = tree.root.join(DIRECTORY).join(MARKER);
        set_acl(&tree.root.join(DIRECTORY), "D:P(A;OICI;FA;;;WD)");
        assert!(publish(&tree.root).is_err());
        assert!(!marker.exists());
        assert!(pending(&tree.root).is_err());
        tree.restore();
        for bytes in [
            br#"{"schema":1,"generation":7,"desired":true,"desired":false}"#.as_slice(),
            br#"{"schema":1,"generation":7,"desired":"true"}"#.as_slice(),
            br#"{"schema":1,"generation":-1,"desired":true}"#.as_slice(),
            &[b'x'; 4097][..],
        ] {
            std::fs::write(tree.root.join("runtime/connection-intent.json"), bytes).unwrap();
            assert!(publish(&tree.root).is_err());
            assert!(!marker.exists());
            assert_eq!(
                std::fs::read(tree.root.join("runtime/connection-intent.json")).unwrap(),
                bytes
            );
        }
    }
    #[test]
    fn absent_directory_is_legacy_compatible_and_explicit_restart_cannot_adopt_it() {
        let tree = Tree::new();
        std::fs::remove_dir(tree.root.join(DIRECTORY)).unwrap();
        assert!(pending(&tree.root).unwrap().is_none());
        assert!(publish(&tree.root).is_err());
        assert!(!tree.root.join(DIRECTORY).exists());
    }
}
