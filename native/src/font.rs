use std::ptr::null;
use windows_sys::Win32::{
    Foundation::HANDLE,
    Graphics::Gdi::{AddFontMemResourceEx, RemoveFontMemResourceEx},
};

// Private to this process: no font installation, disk I/O or system-wide changes.
static FONT: &[u8] = include_bytes!("../../assets/fonts/Tiny5-Regular.ttf");

pub fn family() -> &'static str {
    "Tiny5"
}

pub struct EmbeddedFont(HANDLE);

impl EmbeddedFont {
    pub fn register() -> Result<Self, String> {
        let mut faces = 0;
        let handle = unsafe {
            AddFontMemResourceEx(
                FONT.as_ptr().cast(),
                FONT.len() as u32,
                null(),
                &raw mut faces,
            )
        };
        if handle.is_null() || faces == 0 {
            if !handle.is_null() {
                unsafe { RemoveFontMemResourceEx(handle) };
            }
            return Err("Не удалось загрузить встроенный шрифт Mukhomor.".into());
        }
        Ok(Self(handle))
    }
}

impl Drop for EmbeddedFont {
    fn drop(&mut self) {
        unsafe { RemoveFontMemResourceEx(self.0) };
    }
}
