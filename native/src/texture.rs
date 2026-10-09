use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

#[repr(C, align(4))]
struct Palette([u8; 1064]);
include!(concat!(env!("OUT_DIR"), "/texture_data.rs"));

struct Surface {
    color: u32,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// Small immutable indexed bitmaps are cached once. Integer GDI projection
/// supplies coarse pixels; changing DPI adjusts only the projection scale.
pub struct Textures {
    surfaces: Vec<Surface>,
    solids: Vec<(u32, HBRUSH)>,
    grain: i32,
}
impl Textures {
    pub fn new() -> Result<Self, String> {
        let mut result = Self {
            surfaces: Vec::with_capacity(SURFACES.len()),
            solids: Vec::with_capacity(SOLIDS.len()),
            grain: CELL_GRAIN,
        };
        for (color, palette) in SURFACES {
            unsafe {
                let dc = CreateCompatibleDC(std::ptr::null_mut());
                if dc.is_null() {
                    return Err("Не удалось загрузить фактуру интерфейса Mukhomor.".into());
                }
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(
                    dc,
                    palette.0.as_ptr().cast(),
                    DIB_RGB_COLORS,
                    &mut bits,
                    std::ptr::null_mut(),
                    0,
                );
                if bitmap.is_null() || bits.is_null() {
                    if !bitmap.is_null() {
                        DeleteObject(bitmap);
                    }
                    DeleteDC(dc);
                    return Err("Не удалось загрузить фактуру интерфейса Mukhomor.".into());
                }
                // Copy baked indices only: noise and palette work happened in
                // build.rs. The runtime never generates texture pixels.
                std::ptr::copy_nonoverlapping(FIELD.as_ptr(), bits.cast::<u8>(), FIELD.len());
                let old = SelectObject(dc, bitmap);
                if old.is_null() || old as isize == -1 {
                    DeleteObject(bitmap);
                    DeleteDC(dc);
                    return Err("Не удалось загрузить фактуру интерфейса Mukhomor.".into());
                }
                result.surfaces.push(Surface {
                    color: *color,
                    dc,
                    bitmap,
                    old,
                });
            }
        }
        for color in SOLIDS {
            let brush = unsafe { CreateSolidBrush(*color) };
            if brush.is_null() {
                return Err("Не удалось загрузить фактуру интерфейса Mukhomor.".into());
            }
            result.solids.push((*color, brush));
        }
        Ok(result)
    }
    pub fn set_dpi(&mut self, dpi: i32) {
        self.grain = grain(dpi);
    }
    pub fn brush(&self, color: u32) -> HBRUSH {
        self.solids
            .iter()
            .find(|entry| entry.0 == color)
            .map(|entry| entry.1)
            .unwrap_or_else(|| unsafe { GetStockObject(NULL_BRUSH) })
    }

    /// Honor the caller's device-space brush origin: global field coordinate
    /// equals device pixel minus brush origin. Child controls use a negative
    /// parent-relative position, so they continue the parent's same field.
    pub unsafe fn fill(&self, dc: HDC, rect: &RECT, color: u32) {
        unsafe {
            FillRect(dc, rect, self.brush(color));
            let Some(surface) = self.surfaces.iter().find(|entry| entry.color == color) else {
                return;
            };
            let (mut viewport, mut window, mut origin) =
                (POINT::default(), POINT::default(), POINT::default());
            GetViewportOrgEx(dc, &mut viewport);
            GetWindowOrgEx(dc, &mut window);
            GetBrushOrgEx(dc, &mut origin);
            let offset_x = viewport.x - window.x - origin.x;
            let offset_y = viewport.y - window.y - origin.y;
            let x0 = ((rect.left + offset_x).div_euclid(self.grain)).clamp(0, FIELD_WIDTH);
            let y0 = ((rect.top + offset_y).div_euclid(self.grain)).clamp(0, FIELD_HEIGHT);
            let x1 = ((rect.right + offset_x + self.grain - 1).div_euclid(self.grain))
                .clamp(0, FIELD_WIDTH);
            let y1 = ((rect.bottom + offset_y + self.grain - 1).div_euclid(self.grain))
                .clamp(0, FIELD_HEIGHT);
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let saved = SaveDC(dc);
            if saved == 0 {
                return;
            }
            IntersectClipRect(dc, rect.left, rect.top, rect.right, rect.bottom);
            SetStretchBltMode(dc, COLORONCOLOR);
            StretchBlt(
                dc,
                x0 * self.grain - offset_x,
                y0 * self.grain - offset_y,
                (x1 - x0) * self.grain,
                (y1 - y0) * self.grain,
                surface.dc,
                x0,
                y0,
                x1 - x0,
                y1 - y0,
                SRCCOPY,
            );
            RestoreDC(dc, saved);
        }
    }
}
impl Drop for Textures {
    fn drop(&mut self) {
        for (_, brush) in &self.solids {
            unsafe {
                DeleteObject(*brush);
            }
        }
    }
}
fn grain(dpi: i32) -> i32 {
    (CELL_GRAIN * dpi.clamp(96, 384) + 48) / 96
}

/// QA reads the exact baked palette and globally projected indexed field.
pub fn sample(color: u32, dpi: i32, x: i32, y: i32) -> u32 {
    let Some((_, palette)) = SURFACES.iter().find(|entry| entry.0 == color) else {
        return color;
    };
    let (x, y) = (x.div_euclid(grain(dpi)), y.div_euclid(grain(dpi)));
    if !(0..FIELD_WIDTH).contains(&x) || !(0..FIELD_HEIGHT).contains(&y) {
        return color;
    }
    let offset = 40 + usize::from(FIELD[(y * FIELD_WIDTH + x) as usize]) * 4;
    u32::from(palette.0[offset + 2])
        | u32::from(palette.0[offset + 1]) << 8
        | u32::from(palette.0[offset]) << 16
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::{
        GR_GDIOBJECTS, GetCurrentProcess, GetGuiResources,
    };
    #[test]
    fn baked_field_has_large_zinc_flow_and_crisp_blocks() {
        const { assert!(FIELD_WIDTH * CELL_GRAIN >= 1152 && FIELD_HEIGHT * CELL_GRAIN >= 4608) };
        assert_eq!(FIELD.len(), (FIELD_WIDTH * FIELD_HEIGHT) as usize);
        assert_eq!(
            [grain(96), grain(120), grain(144), grain(192)],
            [10, 13, 15, 20]
        );
        let mut tones = FIELD.to_vec();
        tones.sort_unstable();
        tones.dedup();
        let quantization_step = tones
            .windows(2)
            .map(|pair| i32::from(pair[1]) - i32::from(pair[0]))
            .max()
            .unwrap();
        let bg = SURFACES[0].0;
        let (mut zinc, mut indigo, mut smooth, mut edges) = (0, 0, 0, 0);
        for y in 0..(740 / CELL_GRAIN) {
            for x in 0..(560 / CELL_GRAIN) {
                let value = FIELD[(y * FIELD_WIDTH + x) as usize];
                zinc += usize::from(value <= ZINC_CEILING);
                indigo += usize::from(value > ZINC_CEILING);
                if x > 0 {
                    edges += 1;
                    smooth += usize::from(
                        (i32::from(value) - i32::from(FIELD[(y * FIELD_WIDTH + x - 1) as usize]))
                            .abs()
                            <= quantization_step,
                    );
                }
                for dpi in [96, 120, 144, 192] {
                    let g = grain(dpi);
                    assert_eq!(
                        sample(bg, dpi, x * g, y * g),
                        sample(bg, dpi, x * g + g - 1, y * g + g - 1)
                    );
                }
            }
        }
        let total = zinc + indigo;
        assert!(
            (50..=60).contains(&(zinc * 100 / total))
                && (40..=50).contains(&(indigo * 100 / total)),
            "home crop needs mostly zinc with indigo regions: {zinc} {indigo}"
        );
        for level in 0..256 {
            let offset = 40 + level * 4;
            let [blue, green, red] = SURFACES[0].1.0[offset..offset + 3] else {
                unreachable!()
            };
            assert!(
                green <= red && red <= blue,
                "background palette must contain zinc and indigo without teal"
            );
        }
        assert!(
            smooth * 100 / edges > 92,
            "flow must be large continuous forms rather than fine random grain"
        );
        for (color, _) in SURFACES {
            let first: Vec<_> = (0..64)
                .map(|x| sample(*color, 96, x * CELL_GRAIN, 0))
                .collect();
            let distant: Vec<_> = (0..64)
                .map(|x| sample(*color, 96, x * CELL_GRAIN, 384))
                .collect();
            assert_ne!(first, distant, "no old small repeating texture");
        }
    }
    #[test]
    fn cached_gdi_field_preserves_origin_clipping_dpi_and_resources() {
        unsafe {
            drop(Textures::new().unwrap());
            let before = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            let mut textures = Textures::new().unwrap();
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = 40;
            info.bmiHeader.biWidth = 256;
            info.bmiHeader.biHeight = -256;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            assert!(!bitmap.is_null());
            let old = SelectObject(dc, bitmap);
            let rect = RECT {
                left: 3,
                top: 5,
                right: 230,
                bottom: 241,
            };
            let handles: Vec<_> = textures.surfaces.iter().map(|s| (s.dc, s.bitmap)).collect();
            for _ in 0..12 {
                for dpi in [96, 120, 144, 192] {
                    textures.set_dpi(dpi);
                    for (color, _) in SURFACES {
                        SetViewportOrgEx(dc, 0, -13, std::ptr::null_mut());
                        SetBrushOrgEx(dc, -61, -37, std::ptr::null_mut());
                        textures.fill(dc, &rect, *color);
                        let mut viewport = POINT::default();
                        GetViewportOrgEx(dc, &mut viewport);
                        assert_eq!((viewport.x, viewport.y), (0, -13));
                        SetViewportOrgEx(dc, 0, 0, std::ptr::null_mut());
                        for (x, y) in [(7, 9), (63, 63), (110, 139), (221, 220)] {
                            assert_eq!(
                                GetPixel(dc, x, y),
                                sample(*color, dpi, x + 61, y + 37),
                                "GDI projection must match baked palette and parent-relative origin"
                            );
                        }
                    }
                }
            }
            assert_eq!(
                handles,
                textures
                    .surfaces
                    .iter()
                    .map(|s| (s.dc, s.bitmap))
                    .collect::<Vec<_>>()
            );
            SetViewportOrgEx(dc, 0, 0, std::ptr::null_mut());
            SetBrushOrgEx(dc, 0, 0, std::ptr::null_mut());
            let bg = SURFACES[0].0;
            let all = RECT {
                left: 0,
                top: 0,
                right: 256,
                bottom: 256,
            };
            FillRect(dc, &all, textures.brush(bg));
            textures.fill(
                dc,
                &RECT {
                    left: 37,
                    top: 43,
                    right: 55,
                    bottom: 58,
                },
                bg,
            );
            assert_eq!(GetPixel(dc, 36, 43), bg);
            assert_eq!(GetPixel(dc, 55, 55), bg);
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
            drop(textures);
            assert_eq!(GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), before);
        }
    }
}
