use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

#[repr(C, align(4))]
struct Palette([u8; 1064]);
include!(concat!(env!("OUT_DIR"), "/texture_data.rs"));

pub const FLOW_PERIOD_MS: u32 = 48_000;
const FLOW_FRAME_MS: u32 = FLOW_PERIOD_MS / FLOW_FRAMES as u32;

struct Surface {
    color: u32,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}
struct Background {
    surface: Surface,
    bits: *mut u32,
    size: (i32, i32),
    key: Option<(i32, u32)>,
}
impl Background {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = 40;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height;
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
            if bitmap.is_null() || bits.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            Some(Self {
                surface: Surface {
                    color: SURFACES[0].0,
                    dc,
                    bitmap,
                    old,
                },
                bits: bits.cast(),
                size: (width, height),
                key: None,
            })
        }
    }
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
    pub theme: crate::appearance::Theme,
    flow_palette: [u32; 256],
    surfaces: Vec<Surface>,
    solids: Vec<(u32, HBRUSH)>,
    grain: i32,
    dpi: i32,
    effects: bool,
    background: Option<Background>,
    pub background_frames: u64,
}
impl Textures {
    #[cfg(test)]
    pub fn new() -> Result<Self, String> {
        Self::with_theme(crate::appearance::Theme::default())
    }
    pub fn with_theme(theme: crate::appearance::Theme) -> Result<Self, String> {
        let mut result = Self {
            theme,
            flow_palette: [0; 256],
            surfaces: Vec::with_capacity(SURFACES.len()),
            solids: Vec::with_capacity(SOLIDS.len()),
            grain: CELL_GRAIN,
            dpi: 96,
            effects: true,
            background: None,
            background_frames: 0,
        };
        for (color, palette) in SURFACES {
            let mut palette = palette.0;
            for level in 0..256 {
                let p = 40 + level * 4;
                let original = u32::from(palette[p + 2])
                    | u32::from(palette[p + 1]) << 8
                    | u32::from(palette[p]) << 16;
                let mapped = theme.texture_color(*color, level, original);
                palette[p] = (mapped >> 16) as u8;
                palette[p + 1] = (mapped >> 8) as u8;
                palette[p + 2] = mapped as u8;
                if *color == SURFACES[0].0 {
                    result.flow_palette[level] = mapped;
                }
            }
            // BITMAPINFO must retain four-byte alignment.
            let palette = Palette(palette);
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
            let brush = unsafe { CreateSolidBrush(theme.color(*color)) };
            if brush.is_null() {
                return Err("Не удалось загрузить фактуру интерфейса Mukhomor.".into());
            }
            result.solids.push((*color, brush));
        }
        Ok(result)
    }
    pub fn color(&self, token: u32) -> u32 {
        self.theme.color(token)
    }
    pub fn set_dpi(&mut self, dpi: i32) {
        if self.dpi != dpi
            && let Some(background) = &mut self.background
        {
            background.key = None;
        }
        self.grain = grain(dpi);
        self.dpi = dpi;
    }
    pub fn set_design(&mut self, design: crate::design::Design) {
        self.effects = design.effects;
        if !self.effects {
            self.background = None;
        }
        if let Some(background) = &mut self.background {
            background.key = None;
        }
    }
    /// Render only the visible viewport. One palette lookup per coarse cell,
    /// integer blending, and row copies supply a reusable, gapless GDI bitmap.
    /// All controls blit the same frame, including the cutout corners.
    pub unsafe fn prepare_background(&mut self, width: i32, height: i32, millis: u32) {
        if !self.effects || width <= 0 || height <= 0 {
            return;
        }
        let key = (self.dpi, millis % FLOW_PERIOD_MS);
        if self
            .background
            .as_ref()
            .is_none_or(|b| b.size != (width, height))
        {
            self.background = unsafe { Background::new(width, height) };
        }
        let Some(background) = &mut self.background else {
            return;
        };
        if background.key == Some(key) {
            return;
        }
        // GDI may batch an earlier blit from this DIB. Finish it before writing.
        unsafe {
            GdiFlush();
        }
        let g = self.grain;
        let row_size = width as usize;
        for y in (0..height).step_by(g as usize) {
            let row = unsafe {
                std::slice::from_raw_parts_mut(background.bits.add(y as usize * row_size), row_size)
            };
            for x in (0..width).step_by(g as usize) {
                let end = (x + g).min(width) as usize;
                let color = dib_color(flow_color_from(&self.flow_palette, x / g, y / g, millis));
                row[x as usize..end].fill(color);
            }
            for yy in y + 1..(y + g).min(height) {
                let dest = unsafe { background.bits.add(yy as usize * row_size) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        background.bits.add(y as usize * row_size),
                        dest,
                        row_size,
                    );
                }
            }
        }
        background.key = Some(key);
        self.background_frames += 1;
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
            if !self.effects {
                FillRect(dc, rect, self.brush(color));
                return;
            }
            let Some(surface) = self.surfaces.iter().find(|entry| entry.color == color) else {
                FillRect(dc, rect, self.brush(color));
                return;
            };
            let (mut viewport, mut window, mut origin) =
                (POINT::default(), POINT::default(), POINT::default());
            GetViewportOrgEx(dc, &mut viewport);
            GetWindowOrgEx(dc, &mut window);
            GetBrushOrgEx(dc, &mut origin);
            let offset_x = viewport.x - window.x - origin.x;
            let offset_y = viewport.y - window.y - origin.y;
            if color == SURFACES[0].0
                && let Some(background) = &self.background
                && background.key.is_some()
            {
                // A scrolled child can straddle the canvas. Keep its visible
                // part on the published frame instead of falling back to a
                // different, static phase for the entire child.
                let left = rect.left.max(-offset_x);
                let top = rect.top.max(-offset_y);
                let right = rect.right.min(background.size.0 - offset_x);
                let bottom = rect.bottom.min(background.size.1 - offset_y);
                if right <= left || bottom <= top {
                    return;
                }
                BitBlt(
                    dc,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    background.surface.dc,
                    left + offset_x,
                    top + offset_y,
                    SRCCOPY,
                );
                return;
            }
            FillRect(dc, rect, self.brush(color));
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
    (CELL_GRAIN * dpi.clamp(48, 960) + 48) / 96
}
fn dib_color(color: u32) -> u32 {
    ((color & 255) << 16) | (color & 0xff00) | ((color >> 16) & 255)
}
#[cfg(test)]
fn flow_color(x: i32, y: i32, millis: u32) -> u32 {
    let colors = std::array::from_fn(|i| {
        let p = 40 + i * 4;
        let palette = &SURFACES[0].1.0;
        u32::from(palette[p + 2]) | u32::from(palette[p + 1]) << 8 | u32::from(palette[p]) << 16
    });
    flow_color_from(&colors, x, y, millis)
}
fn flow_color_from(palette: &[u32; 256], x: i32, y: i32, millis: u32) -> u32 {
    let frame_size = FIELD.len();
    let frame = (millis % FLOW_PERIOD_MS) as usize / FLOW_FRAME_MS as usize;
    let next = (frame + 1) % FLOW_FRAMES;
    let weight = (millis % FLOW_FRAME_MS) * 256 / FLOW_FRAME_MS;
    let offset = (y.rem_euclid(FIELD_HEIGHT) * FIELD_WIDTH + x.rem_euclid(FIELD_WIDTH)) as usize;
    let a = palette[FLOW[frame * frame_size + offset] as usize];
    let b = palette[FLOW[next * frame_size + offset] as usize];
    [0, 8, 16].into_iter().fold(0, |rgb, shift| {
        let first = (a >> shift) & 255;
        let second = (b >> shift) & 255;
        rgb | ((first * (256 - weight) + second * weight + 128) / 256) << shift
    })
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
    fn lava_remains_visible_and_moves_in_every_theme() {
        let distance = |a: u32, b: u32| {
            [0, 8, 16]
                .into_iter()
                .map(|shift| (((a >> shift) & 255) as i32 - ((b >> shift) & 255) as i32).abs())
                .max()
                .unwrap()
        };
        for theme in crate::appearance::Theme::ALL {
            let textures = Textures::with_theme(theme).unwrap();
            let colors = &textures.flow_palette;
            let range = colors
                .iter()
                .flat_map(|a| colors.iter().map(move |b| distance(*a, *b)))
                .max()
                .unwrap();
            let mut moving = 0;
            let mut largest_change = 0;
            for y in 0..57 {
                for x in 0..43 {
                    let delta = distance(
                        flow_color_from(colors, x, y, 0),
                        flow_color_from(colors, x, y, 12_000),
                    );
                    moving += usize::from(delta >= 5);
                    largest_change = largest_change.max(delta);
                }
            }
            eprintln!(
                "{theme:?}: palette range {range}, moving cells {moving}/2451, maximum change {largest_change}"
            );
            assert!(range >= 16, "{theme:?}: lava palette has collapsed");
            assert!(moving >= 245, "{theme:?}: lava movement is too faint");
        }
    }
    #[test]
    fn partially_clipped_children_keep_the_published_animation_frame() {
        unsafe {
            let mut textures = Textures::new().unwrap();
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = 40;
            info.bmiHeader.biWidth = 160;
            info.bmiHeader.biHeight = -160;
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
            let old = SelectObject(dc, bitmap);
            let rect = RECT {
                left: 0,
                top: 0,
                right: 160,
                bottom: 160,
            };
            for dpi in [96, 120, 144, 192] {
                textures.set_dpi(dpi);
                for phase in [0, 2000, 13_750] {
                    textures.prepare_background(560, 740, phase);
                    for (ox, oy) in [(32, -80), (480, 32), (32, 650), (-80, 32)] {
                        SetBrushOrgEx(dc, -ox, -oy, std::ptr::null_mut());
                        textures.fill(dc, &rect, SURFACES[0].0);
                        for y in 0..160 {
                            for x in 0..160 {
                                if (0..560).contains(&(x + ox)) && (0..740).contains(&(y + oy)) {
                                    assert_eq!(
                                        GetPixel(dc, x, y),
                                        flow_color(
                                            (x + ox) / grain(dpi),
                                            (y + oy) / grain(dpi),
                                            phase
                                        ),
                                        "clipped child at {ox},{oy}, phase {phase}, DPI {dpi}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
        }
    }
    #[test]
    fn animated_viewport_is_seamless_gapless_and_reuses_every_gdi_object() {
        unsafe {
            drop(Textures::new().unwrap());
            let before = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            let mut textures = Textures::new().unwrap();
            textures.prepare_background(560, 740, 0);
            let background = textures.background.as_ref().unwrap();
            let handles = (
                background.surface.dc,
                background.surface.bitmap,
                background.bits,
            );
            let resources = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            let initial = std::slice::from_raw_parts(handles.2, 560 * 740).to_vec();
            textures.prepare_background(560, 740, 2_000);
            let changed = std::slice::from_raw_parts(handles.2, 560 * 740)
                .iter()
                .zip(&initial)
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                changed > initial.len() / 10,
                "lava must visibly move within two seconds"
            );
            // Every cell is solid through its last row/column, without a grid.
            for y in 0..56 {
                for x in 0..43 {
                    assert_eq!(
                        GetPixel(handles.0, x * 13, y * 13),
                        GetPixel(handles.0, x * 13 + 12, y * 13 + 12)
                    );
                    for t in (0..FLOW_PERIOD_MS).step_by(50) {
                        let a = flow_color(x, y, t);
                        let b = flow_color(x, y, t + 50);
                        for shift in [0, 8, 16] {
                            assert!(
                                (((a >> shift) & 255) as i32 - ((b >> shift) & 255) as i32).abs()
                                    <= 6,
                                "animation must not flash between frames"
                            );
                        }
                    }
                }
            }
            let start = std::time::Instant::now();
            for millis in (0..FLOW_PERIOD_MS).step_by(50) {
                textures.prepare_background(560, 740, millis);
                let b = textures.background.as_ref().unwrap();
                assert_eq!(handles, (b.surface.dc, b.surface.bitmap, b.bits));
                assert_eq!(
                    GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS),
                    resources
                );
                for (x, y) in [(7, 9), (63, 63), (110, 139), (550, 700)] {
                    let expected = flow_color(x / 13, y / 13, millis);
                    assert_eq!(GetPixel(b.surface.dc, x, y), expected);
                }
            }
            eprintln!("960 cached 560x740 animation frames: {:?}", start.elapsed());
            textures.prepare_background(560, 740, FLOW_PERIOD_MS);
            assert_eq!(std::slice::from_raw_parts(handles.2, 560 * 740), initial);
            for y in 0..57 {
                for x in 0..43 {
                    let a = flow_color(x, y, FLOW_PERIOD_MS - 1);
                    let b = flow_color(x, y, 0);
                    for shift in [0, 8, 16] {
                        assert!(
                            (((a >> shift) & 255) as i32 - ((b >> shift) & 255) as i32).abs() <= 1
                        );
                    }
                }
            }
            let frames = textures.background_frames;
            textures.prepare_background(560, 740, 0);
            assert_eq!(textures.background_frames, frames, "same phase is free");
            assert_eq!(GetPixel(handles.0, 12, 12), flow_color(0, 0, 0));
            textures.set_design(crate::design::Design {
                effects: false,
                ..Default::default()
            });
            assert!(
                textures.background.is_none(),
                "low power mode releases the animation bitmap"
            );
            let frames = textures.background_frames;
            textures.prepare_background(560, 740, 10_000);
            assert_eq!(textures.background_frames, frames);
            drop(textures);
            assert_eq!(GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), before);
        }
    }
    #[test]
    fn baked_field_has_readable_neon_flow_and_crisp_blocks() {
        const { assert!(FIELD_WIDTH * CELL_GRAIN >= 1152 && FIELD_HEIGHT * CELL_GRAIN >= 4608) };
        assert_eq!(FIELD.len(), (FIELD_WIDTH * FIELD_HEIGHT) as usize);
        assert_eq!(
            [grain(96), grain(120), grain(144), grain(192)],
            [13, 16, 20, 26]
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
                        sample(
                            bg,
                            dpi,
                            x * g + g - (dpi + 48) / 96 - 1,
                            y * g + g - (dpi + 48) / 96 - 1
                        )
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
            let channels = [red, green, blue];
            let brightest = *channels.iter().max().unwrap();
            assert!(
                brightest <= 96,
                "neon texture must retain a dark background"
            );
            let luminance = |channels: [u8; 3]| -> f64 {
                channels
                    .into_iter()
                    .zip([0.2126, 0.7152, 0.0722])
                    .map(|(c, weight)| {
                        let c = f64::from(c) / 255.0;
                        weight
                            * if c <= 0.04045 {
                                c / 12.92
                            } else {
                                ((c + 0.055) / 1.055).powf(2.4)
                            }
                    })
                    .sum()
            };
            let background = luminance(channels);
            let colors = crate::appearance::Theme::default().colors();
            for ink in [colors[3], colors[4]] {
                let foreground = luminance([ink as u8, (ink >> 8) as u8, (ink >> 16) as u8]);
                assert!(
                    (foreground + 0.05) / (background + 0.05) >= 4.5,
                    "text must stay readable at every neon animation level"
                );
            }
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
