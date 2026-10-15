//! Pixel artwork, system icons and the baked animated field.
//! Colors here remain COLORREF until the final conversion to Slint RGBA.
use crate::{AppWindow, Palette, slint};
use slint::ComponentHandle;
#[allow(dead_code)]
#[path = "../../native/src/appearance.rs"]
pub mod appearance;
#[allow(dead_code)]
#[path = "../../native/src/mushroom.rs"]
mod mushroom;
#[allow(dead_code)]
mod baked {
    type Colors = [[u32; 256]; 4];
    // Same stationary four-tone tile and 2% blend as website/public/site.js.
    // Sample by field coordinates so every surface keeps the same pixel pitch.
    static GRAIN: std::sync::LazyLock<[u8; 144 * 96]> = std::sync::LazyLock::new(|| {
        let mut seed = 0x4d55_4b48u32;
        std::array::from_fn(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            ((seed >> 30) * 85) as u8
        })
    });
    std::thread_local! {
        static COLORS: std::cell::RefCell<Option<(super::appearance::Theme, Colors)>> = const { std::cell::RefCell::new(None) };
    }
    pub struct Palette(pub [u8; 1064]);
    include!(concat!(env!("OUT_DIR"), "/texture_data.rs"));
    pub fn dimensions() -> (u32, u32, i32) {
        (FIELD_WIDTH as u32, FIELD_HEIGHT as u32, CELL_GRAIN)
    }
    pub fn image(
        theme: super::appearance::Theme,
        surface: usize,
        millis: u32,
    ) -> super::slint::Image {
        let colors = COLORS.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.as_ref().is_none_or(|(saved, _)| *saved != theme) {
                *cache = Some((
                    theme,
                    std::array::from_fn(|surface| {
                        let (token, data) = &SURFACES[surface];
                        std::array::from_fn(|level| {
                            let p = 40 + level * 4;
                            let original = u32::from(data.0[p + 2])
                                | u32::from(data.0[p + 1]) << 8
                                | u32::from(data.0[p]) << 16;
                            theme.texture_color(*token, level, original)
                        })
                    }),
                ));
            }
            cache.as_ref().unwrap().1[surface]
        });
        let mut buffer = super::slint::SharedPixelBuffer::<super::slint::Rgba8Pixel>::new(
            FIELD_WIDTH as u32,
            FIELD_HEIGHT as u32,
        );
        let frame = (millis % 48_000) as usize / 2000;
        let next = (frame + 1) % FLOW_FRAMES;
        let weight = (millis % 2000) * 256 / 2000;
        for (offset, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
            let grain = u32::from(
                GRAIN[(offset / FIELD_WIDTH as usize % 96) * 144
                    + offset % FIELD_WIDTH as usize % 144],
            );
            let a = colors[if surface == 0 {
                FLOW[frame * FIELD.len() + offset]
            } else {
                FIELD[offset]
            } as usize];
            let b = colors[if surface == 0 {
                FLOW[next * FIELD.len() + offset]
            } else {
                FIELD[offset]
            } as usize];
            let c = [0, 8, 16].map(|shift| {
                let channel =
                    (((a >> shift) & 255) * (256 - weight) + ((b >> shift) & 255) * weight + 128)
                        / 256;
                ((channel * 98 + grain * 2 + 50) / 100) as u8
            });
            *pixel = super::slint::Rgba8Pixel::new(c[0], c[1], c[2], 255);
        }
        super::slint::Image::from_rgba8(buffer)
    }
}

fn color(value: u32) -> slint::Color {
    slint::Color::from_rgb_u8(value as u8, (value >> 8) as u8, (value >> 16) as u8)
}
fn artwork<const W: usize, const H: usize>(
    rows: &[&[u8; W]; H],
    colors: &[u32; mushroom::PALETTE_LEN],
    roots: bool,
) -> slint::Image {
    let mut image = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(W as u32, H as u32);
    for (y, row) in rows.iter().enumerate() {
        for (x, glyph) in row.iter().enumerate() {
            if H > 48 && y >= 48 && !roots {
                continue;
            }
            if let Some(i) = mushroom::palette_index(*glyph) {
                let c = color(colors[i]);
                image.make_mut_slice()[y * W + x] =
                    slint::Rgba8Pixel::new(c.red(), c.green(), c.blue(), 255);
            }
        }
    }
    slint::Image::from_rgba8(image)
}
fn icon(kind: usize) -> slint::Image {
    let mut image = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(12, 12);
    for (source, target) in mushroom::icon_grid(kind)
        .iter()
        .flat_map(|row| row.bytes())
        .zip(image.make_mut_slice())
    {
        if source != b'.' {
            *target = slint::Rgba8Pixel::new(255, 255, 255, 255);
        }
    }
    slint::Image::from_rgba8(image)
}

fn caption(close: bool) -> slint::Image {
    let mut image = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(8, 8);
    for (source, target) in mushroom::caption_grid(close)
        .iter()
        .flat_map(|row| row.bytes())
        .zip(image.make_mut_slice())
    {
        if source != b'.' {
            *target = slint::Rgba8Pixel::new(255, 255, 255, 255);
        }
    }
    slint::Image::from_rgba8(image)
}

fn system_icon_buffer(
    palette: &[u32; mushroom::PALETTE_LEN],
    size: u32,
) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(size, size);
    for (offset, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        if let Some(index) = mushroom::icon_palette_index(
            offset % size as usize,
            offset / size as usize,
            size as usize,
        ) {
            let c = color(palette[index]);
            *pixel = slint::Rgba8Pixel::new(c.red(), c.green(), c.blue(), 255);
        }
    }
    buffer
}

pub fn apply(ui: &AppWindow, theme: appearance::Theme, effects: bool) {
    let colors = theme.colors();
    let p = ui.global::<Palette>();
    p.set_background(color(colors[0]));
    p.set_panel(color(colors[1]));
    p.set_raised(color(colors[2]));
    p.set_ink(color(colors[3]));
    p.set_muted(color(colors[4]));
    p.set_teal(color(colors[5]));
    p.set_accent(color(colors[6]));
    p.set_line(color(colors[7]));
    p.set_danger(color(colors[8]));
    p.set_button_ink(color(colors[10]));
    ui.set_light_theme(theme == appearance::Theme::Paper);
    ui.set_effects(effects);
    let palette = mushroom::palette(
        colors[0],
        colors[6],
        colors[5],
        colors[9],
        theme == appearance::Theme::Paper,
    );
    ui.set_app_icon(artwork(&mushroom::MINI, &palette, false));
    ui.set_system_icon(slint::Image::from_rgba8(system_icon_buffer(&palette, 32)));
    ui.set_hero_image(artwork(&mushroom::HERO, &palette, false));
    ui.set_hero_connected_image(artwork(&mushroom::HERO, &palette, true));
    ui.set_navigation_icons(
        std::rc::Rc::new(slint::VecModel::from((0..4).map(icon).collect::<Vec<_>>())).into(),
    );
    ui.set_check_icon(icon(4));
    ui.set_close_icon(caption(true));
    ui.set_minimize_icon(caption(false));
    ui.set_arrow_icon(icon(8));
    let (width, height, grain) = baked::dimensions();
    ui.set_texture_width((width * grain as u32) as f32);
    ui.set_texture_height((height * grain as u32) as f32);
    ui.set_panel_texture(baked::image(theme, 1, 0));
    ui.set_raised_texture(baked::image(theme, 2, 0));
    ui.set_accent_texture(baked::image(theme, 3, 0));
    frame(ui, theme, 0);
}
pub fn frame(ui: &AppWindow, theme: appearance::Theme, millis: u32) {
    if ui.get_effects() {
        ui.set_background_image(baked::image(theme, 0, millis));
    }
}

#[allow(dead_code)]
mod motion {
    use super::*;
    include!("../../native/src/hero_motion.rs");

    #[derive(Default)]
    pub(super) struct Animation {
        hero: HeroMotion,
    }
    impl Animation {
        pub(super) fn render(&mut self, ui: &AppWindow, theme: appearance::Theme, delta: u32) {
            let phase = ui.get_phase();
            self.hero.sync(HeroPhase::read(if ui.get_boot_active() { "loading" } else { phase.as_str() }, ui.get_notice_error()));
            let animate = ui.get_effects() && ui.get_animation();
            if animate {
                self.hero.advance(delta);
            }
            let c = theme.colors();
            let colors =
                mushroom::palette(c[0], c[6], c[5], c[9], theme == appearance::Theme::Paper);
            let amount = self.hero.roots(animate);
            let growth = mycelium_growth();
            let front = u32::from(amount) * (u32::from(growth.longest) + ROOT_FADE) / 1000;
            let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(
                mushroom::HERO_WIDTH as u32,
                mushroom::HERO_HEIGHT as u32,
            );
            for (y, row) in mushroom::HERO.iter().enumerate() {
                for (x, &glyph) in row.iter().enumerate() {
                    let Some(index) = mushroom::palette_index(glyph) else {
                        continue;
                    };
                    let coverage = growth.coverage(y, x, amount);
                    if coverage == 0 {
                        continue;
                    }
                    let mut shade = colors[index];
                    if animate && y < MYCELIUM_ROW && matches!(glyph, b'W' | b'V' | b'Q') {
                        let group = mushroom::hero_spot(y, x).unwrap();
                        shade = hero_mix(
                            hero_mix(shade, colors[0], 270),
                            hero_mix(shade, colors[7], 350),
                            self.hero.spot_glow(group),
                        );
                    }
                    if animate && y >= MYCELIUM_ROW {
                        let distance = u32::from(growth.distance[y][x]);
                        let age = front.saturating_sub(distance);
                        let tip = if age < 700 && amount < 1000 {
                            1000 - age * 1000 / 700
                        } else {
                            0
                        };
                        let waiting = if matches!(self.hero.phase, HeroPhase::Loading | HeroPhase::Connecting) {
                            let t = (self.hero.elapsed_ms % 2100 + 2100 - distance % 2100) % 2100;
                            if t < 450 {
                                smooth_progress(450 - t, 450) * 280 / 1000
                            } else {
                                0
                            }
                        } else {
                            0
                        };
                        shade = hero_mix(shade, colors[7], (tip * 650 / 1000).max(waiting));
                    }
                    let c = color(shade);
                    pixels.make_mut_slice()[y * mushroom::HERO_WIDTH + x] = slint::Rgba8Pixel::new(
                        c.red(),
                        c.green(),
                        c.blue(),
                        (coverage * 255 / 1000) as u8,
                    );
                }
            }
            ui.set_hero_image(slint::Image::from_rgba8(pixels));
        }
    }
}

/// The caller owns the timer. Hidden/minimized windows do no artwork rendering.
/// Only confirmed controller phases drive the shared mushroom state machine.
pub fn animate(ui: &AppWindow) -> slint::Timer {
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    let mut hero = motion::Animation::default();
    let mut elapsed = 0u32;
    let mut previous = std::time::Instant::now();
    let mut static_key = None;
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(33),
        move || {
            let now = std::time::Instant::now();
            let delta = now.duration_since(previous).as_millis().min(100) as u32;
            previous = now;
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.window().is_visible() || ui.window().is_minimized() {
                return;
            }
            let theme = appearance::Theme::ALL
                .get(ui.get_theme_index() as usize)
                .copied()
                .unwrap_or_default();
            let moving = ui.get_animation() && ui.get_effects();
            let key = (
                theme,
                ui.get_phase(),
                ui.get_effects(),
                ui.get_animation(),
                ui.get_page(),
                ui.get_boot_active(),
            );
            if moving {
                elapsed = elapsed
                    .wrapping_add(delta * ui.get_animation_speed().clamp(25, 200) as u32 / 100)
                    % 48_000;
                frame(&ui, theme, elapsed);
            } else if static_key.as_ref() == Some(&key) {
                return;
            }
            if ui.get_boot_active() || ui.get_page() == 0 {
                hero.render(&ui, theme, delta * ui.get_animation_speed().clamp(25, 200) as u32 / 100);
            }
            static_key = Some(key);
        },
    );
    timer
}

/// Deterministic samples of the production artwork for visual verification.
#[cfg(feature = "preview")]
pub fn preview_frame(ui: &AppWindow, theme: appearance::Theme, millis: u32) {
    motion::Animation::default().render(ui, theme, millis);
    frame(ui, theme, millis);
}

#[cfg(test)]
mod system_icon_tests {
    use super::*;

    #[test]
    fn system_icon_fills_its_canvas_at_tray_and_taskbar_sizes() {
        for theme in appearance::Theme::ALL {
            let c = theme.colors();
            let palette =
                mushroom::palette(c[0], c[6], c[5], c[9], theme == appearance::Theme::Paper);
            for size in [16, 20, 24, 32, 40, 48, 64] {
                let buffer = system_icon_buffer(&palette, size as u32);
                let pixels = buffer.as_slice();
                assert!(pixels[..size].iter().any(|p| p.a == 255));
                assert!(pixels[(size - 1) * size..].iter().any(|p| p.a == 255));
                assert!((0..size).any(|y| pixels[y * size].a == 255));
                assert!((0..size).any(|y| pixels[y * size + size - 1].a == 255));
                assert!(pixels.iter().all(|p| p.a == 0 || p.a == 255));
                // The header still uses its original padded layout artwork.
                assert_eq!(mushroom::MINI[0][0], b'.');
            }
        }
    }
}
