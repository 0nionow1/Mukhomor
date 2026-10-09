use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*, Security::Authorization::*, Security::*, System::Threading::*,
    UI::WindowsAndMessaging::*,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn error_box(s: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(s).as_ptr(),
            wide("Mukhomor").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
pub fn new_event() -> isize {
    unsafe { CreateEventW(null_mut(), 1, 0, null_mut()) as isize }
}
pub fn signal(event: isize) {
    unsafe {
        SetEvent(event as HANDLE);
    }
}
pub fn signaled(event: isize) -> bool {
    unsafe { WaitForSingleObject(event as HANDLE, 0) == WAIT_OBJECT_0 }
}
pub struct OwnedHandle(pub HANDLE);
unsafe impl Send for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn user_sid() -> Result<String, String> {
    process_sid(unsafe { GetCurrentProcess() })
}
pub fn process_sid(process: HANDLE) -> Result<String, String> {
    unsafe {
        let mut handle = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut handle) == 0 {
            return Err("Cannot read user identity".into());
        }
        let handle = OwnedHandle(handle);
        let mut size = 0;
        GetTokenInformation(handle.0, TokenUser, null_mut(), 0, &mut size);
        let mut buffer = vec![0u8; size as usize];
        if GetTokenInformation(
            handle.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err("Cannot read user identity".into());
        }
        let token = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = null_mut();
        if ConvertSidToStringSidW(token.User.Sid, &mut text) == 0 {
            return Err("Cannot encode user identity".into());
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast::<c_void>());
        Ok(value)
    }
}
