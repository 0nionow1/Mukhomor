use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Design {
    pub theme: crate::appearance::Theme,
    pub interface_size: crate::appearance::InterfaceSize,
    pub effects: bool,
    pub animation: bool,
    pub animation_speed: u32,
    pub rounding: bool,
}
impl Default for Design {
    fn default() -> Self {
        Self {
            theme: Default::default(),
            interface_size: Default::default(),
            effects: true,
            animation: true,
            animation_speed: 100,
            rounding: true,
        }
    }
}
impl Design {
    pub fn read(value: &Value) -> Self {
        Self {
            theme: crate::appearance::Theme::read(value["theme"].as_str()),
            interface_size: crate::appearance::InterfaceSize::read(
                value["interface_size"].as_str(),
            ),
            effects: value["effects"].as_bool().unwrap_or(true),
            animation: value["animation"].as_bool().unwrap_or(true),
            animation_speed: value["animation_speed"]
                .as_u64()
                .unwrap_or(100)
                .clamp(25, 200) as u32,
            rounding: value["rounding"].as_bool().unwrap_or(true),
        }
    }
    pub fn value(self) -> Value {
        json!({"theme":self.theme.id(),"interface_size":self.interface_size.id(),"effects":self.effects,"animation":self.animation,"animation_speed":self.animation_speed,"rounding":self.rounding})
    }
}

/// A visible four-pixel stair tread at 100%, shared by paint and HWND clipping.
pub fn corner_unit(dpi: i32) -> i32 {
    (4 * dpi.clamp(48, 960) + 48) / 96
}

/// Three right-angled steps, never an antialiased curve. The unit is an integer
/// device pixel, so fractional DPI cannot turn the steps into diagonals.
pub unsafe fn region(rect: RECT, unit: i32) -> HRGN {
    let minimum = (rect.right - rect.left).min(rect.bottom - rect.top);
    let u = unit.min(minimum / 12);
    if u < 1 {
        if minimum >= 3 {
            let c = unit.max(1).min(minimum / 3);
            let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
            let points = [
                (l + c, t),
                (r - c, t),
                (r - c, t + c),
                (r, t + c),
                (r, b - c),
                (r - c, b - c),
                (r - c, b),
                (l + c, b),
                (l + c, b - c),
                (l, b - c),
                (l, t + c),
                (l + c, t + c),
            ]
            .map(|(x, y)| POINT { x, y });
            return unsafe { CreatePolygonRgn(points.as_ptr(), points.len() as i32, WINDING) };
        }
        return unsafe { CreateRectRgn(rect.left, rect.top, rect.right, rect.bottom) };
    }
    let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
    let points = [
        (l + 3 * u, t),
        (r - 3 * u, t),
        (r - 3 * u, t + u),
        (r - 2 * u, t + u),
        (r - 2 * u, t + 2 * u),
        (r - u, t + 2 * u),
        (r - u, t + 3 * u),
        (r, t + 3 * u),
        (r, b - 3 * u),
        (r - u, b - 3 * u),
        (r - u, b - 2 * u),
        (r - 2 * u, b - 2 * u),
        (r - 2 * u, b - u),
        (r - 3 * u, b - u),
        (r - 3 * u, b),
        (l + 3 * u, b),
        (l + 3 * u, b - u),
        (l + 2 * u, b - u),
        (l + 2 * u, b - 2 * u),
        (l + u, b - 2 * u),
        (l + u, b - 3 * u),
        (l, b - 3 * u),
        (l, t + 3 * u),
        (l + u, t + 3 * u),
        (l + u, t + 2 * u),
        (l + 2 * u, t + 2 * u),
        (l + 2 * u, t + u),
        (l + 3 * u, t + u),
    ]
    .map(|(x, y)| POINT { x, y });
    unsafe { CreatePolygonRgn(points.as_ptr(), points.len() as i32, WINDING) }
}

pub unsafe fn window_shape(hwnd: HWND, width: i32, height: i32, dpi: i32, rounded: bool) {
    unsafe {
        clipped_window_shape(hwnd, width, height, dpi, rounded, 0, height);
    }
}

/// Clip scrollable controls without resizing their text/caret coordinate space.
pub unsafe fn clipped_window_shape(
    hwnd: HWND,
    width: i32,
    height: i32,
    dpi: i32,
    rounded: bool,
    top: i32,
    bottom: i32,
) {
    let shape = if rounded {
        unsafe {
            region(
                RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                },
                corner_unit(dpi),
            )
        }
    } else {
        std::ptr::null_mut()
    };
    let top = top.clamp(0, height);
    let bottom = bottom.clamp(top, height);
    let shape = if top > 0 || bottom < height {
        let clip = unsafe { CreateRectRgn(0, top, width, bottom) };
        if !shape.is_null() {
            unsafe {
                CombineRgn(shape, shape, clip, RGN_AND);
                DeleteObject(clip);
            }
            shape
        } else {
            clip
        }
    } else {
        shape
    };
    // Windows owns a successfully assigned region; failure leaves it to us.
    if unsafe { SetWindowRgn(hwnd, shape, 0) } == 0 && !shape.is_null() {
        unsafe {
            DeleteObject(shape);
        }
    }
}

#[derive(Default)]
pub struct Regions(RefCell<HashMap<(i32, i32, i32), HRGN>>);
impl Regions {
    pub fn clear(&self) {
        for (_, shape) in self.0.borrow_mut().drain() {
            unsafe {
                DeleteObject(shape);
            }
        }
    }
    pub unsafe fn get(&self, width: i32, height: i32, unit: i32) -> HRGN {
        let key = (width, height, unit);
        let reset = {
            let cache = self.0.borrow();
            cache.len() >= 96 && !cache.contains_key(&key)
        };
        if reset {
            self.clear();
        }
        *self.0.borrow_mut().entry(key).or_insert_with(|| unsafe {
            region(
                RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                },
                unit,
            )
        })
    }
}
impl Drop for Regions {
    fn drop(&mut self) {
        for &shape in self.0.get_mut().values() {
            unsafe {
                DeleteObject(shape);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_migrates_old_preferences_and_roundtrips_independently() {
        let old = Design::read(
            &json!({"effects":false,"animation":false,"animation_speed":75,"rounding":false}),
        );
        assert_eq!(old.theme, crate::appearance::Theme::Neon);
        assert_eq!(old.interface_size, crate::appearance::InterfaceSize::Medium);
        assert!(!old.effects);
        for theme in crate::appearance::Theme::ALL {
            for interface_size in crate::appearance::InterfaceSize::ALL {
                let design = Design {
                    theme,
                    interface_size,
                    ..old
                };
                assert_eq!(Design::read(&design.value()), design);
            }
        }
        let malformed = Design::read(&json!({"theme":[],"interface_size":999}));
        assert_eq!(malformed, Design::default());
    }
    #[test]
    fn animation_speed_migrates_and_bounds_saved_input() {
        assert_eq!(Design::read(&json!({})).animation_speed, 100);
        for (input, expected) in [
            (json!(25), 25),
            (json!(175), 175),
            (json!(0), 25),
            (json!(u64::MAX), 200),
            (json!(-1), 100),
            (json!("fast"), 100),
        ] {
            let design = Design::read(&json!({"animation_speed":input}));
            assert_eq!(design.animation_speed, expected);
            assert_eq!(Design::read(&design.value()), design);
        }
    }
    #[test]
    fn region_cache_is_bounded_during_resizing_and_small_caps_stay_pixel_shaped() {
        unsafe {
            let cache = Regions::default();
            for width in 100..500 {
                let shape = cache.get(width, 42, 2);
                assert!(!shape.is_null());
                assert_eq!(shape, cache.get(width, 42, 2));
                assert!(cache.0.borrow().len() <= 96);
            }
            let cap = cache.get(3, 40, 2);
            assert_eq!(PtInRegion(cap, 0, 0), 0);
            assert_ne!(PtInRegion(cap, 1, 0), 0);
            assert_ne!(PtInRegion(cap, 0, 1), 0);
            cache.clear();
            assert!(cache.0.borrow().is_empty());
        }
    }
    #[test]
    fn corners_are_symmetric_integer_steps_at_every_dpi() {
        unsafe {
            for dpi in [96, 120, 144, 192] {
                let u = corner_unit(dpi);
                let (w, h) = (120 * dpi / 96, 42 * dpi / 96);
                let shape = region(
                    RECT {
                        left: 0,
                        top: 0,
                        right: w,
                        bottom: h,
                    },
                    u,
                );
                assert!(!shape.is_null());
                for y in 0..h {
                    for x in 0..w {
                        let inside = PtInRegion(shape, x, y);
                        assert_eq!(inside, PtInRegion(shape, w - 1 - x, y));
                        assert_eq!(inside, PtInRegion(shape, x, h - 1 - y));
                    }
                }
                assert_eq!(PtInRegion(shape, 0, 0), 0);
                assert_ne!(PtInRegion(shape, 3 * u, 0), 0);
                assert_ne!(PtInRegion(shape, 0, 3 * u), 0);
                DeleteObject(shape);
            }
        }
    }
}
