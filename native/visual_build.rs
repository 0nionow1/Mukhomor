//! Shared, deterministic artwork baker for Win32 and Slint. No platform APIs.
use std::{fs, path::Path};

fn color(source: &str, key: &str) -> u32 {
    let needle = format!("\"{key}\"");
    let tail = source
        .split_once(&needle)
        .unwrap_or_else(|| panic!("Missing theme color: {key}"))
        .1;
    let value = tail
        .split_once(':')
        .expect("Theme requires a colon after every color name")
        .1
        .trim_start()
        .strip_prefix("\"#")
        .unwrap_or_else(|| panic!("Theme color {key} must be a quoted #RRGGBB value"));
    assert!(
        value.get(6..7) == Some("\""),
        "Theme color {key} must contain exactly six hexadecimal digits"
    );
    u32::from_str_radix(value.get(..6).expect("Theme color must be #RRGGBB"), 16)
        .expect("Invalid hexadecimal theme color")
}

fn number(source: &str, key: &str, default: u32, maximum: u32) -> u32 {
    let Some((_, tail)) = source.split_once(&format!("\"{key}\"")) else {
        return default;
    };
    let value = tail.split_once(':').unwrap().1.trim_start();
    let end = value.find([',', '}']).unwrap_or(value.len());
    let value: u32 = value[..end]
        .trim()
        .parse()
        .expect("Invalid texture setting");
    assert!(value <= maximum, "Texture setting {key} is too large");
    value
}

fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut value = x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ seed;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846ca68b);
    value ^ (value >> 16)
}

fn noise(x: f64, y: f64, seed: u32) -> f64 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let gradient = |dx: i32, dy: i32| {
        let px = fx - f64::from(dx);
        let py = fy - f64::from(dy);
        match hash((ix + dx) as u32, (iy + dy) as u32, seed) & 7 {
            0 => px,
            1 => -px,
            2 => py,
            3 => -py,
            4 => (px + py) * std::f64::consts::FRAC_1_SQRT_2,
            5 => (px - py) * std::f64::consts::FRAC_1_SQRT_2,
            6 => (-px + py) * std::f64::consts::FRAC_1_SQRT_2,
            _ => (-px - py) * std::f64::consts::FRAC_1_SQRT_2,
        }
    };
    let mix = |a: f64, b: f64, t: f64| a + (b - a) * t;
    mix(
        mix(gradient(0, 0), gradient(1, 0), fade(fx)),
        mix(gradient(0, 1), gradient(1, 1), fade(fx)),
        fade(fy),
    )
}

fn fbm(mut x: f64, mut y: f64, seed: u32, octaves: u32) -> f64 {
    let (mut sum, mut weight, mut total) = (0.0, 1.0, 0.0);
    for octave in 0..octaves {
        sum += noise(x, y, seed.wrapping_add(octave * 0x17ab21)) * weight;
        total += weight;
        // A small rotation keeps subsequent octaves from aligning in a grid.
        (x, y) = (x * 1.84 - y * 0.79 + 7.3, x * 0.79 + y * 1.84 + 3.9);
        weight *= 0.48;
    }
    sum / total
}

fn blend(a: u32, b: u32, amount: f64) -> u32 {
    let mut rgb = 0;
    for shift in [0, 8, 16] {
        let channel = f64::from((a >> shift) & 255)
            + (f64::from((b >> shift) & 255) - f64::from((a >> shift) & 255)) * amount;
        rgb |= (channel.round().clamp(0.0, 255.0) as u32) << shift;
    }
    rgb
}

/// Bake one large indexed field and five palettes. Native GDI stores the tiny
/// coarse source; integer nearest-neighbor projection keeps UI pixels crisp.
fn textures(out: &Path, source: &str, colors: &[u32]) {
    let background_strength = number(source, "texture_background_strength", 100, 100);
    let surface_strength = number(source, "texture_surface_strength", 28, 100);
    let seed = number(source, "texture_seed", 13371337, u32::MAX);
    let grain = number(source, "texture_grain", 13, 16);
    let scale = number(source, "texture_flow_scale", 480, 1024);
    let warp = number(source, "texture_warp_strength", 200, 512);
    let octaves = number(source, "texture_octaves", 2, 6);
    let levels = number(source, "texture_levels", 16, 64);
    assert!(
        (4..=16).contains(&grain) && scale >= 96 && octaves >= 1 && levels >= 8,
        "Invalid texture flow settings"
    );
    let zinc_low = color(source, "texture_zinc_low");
    let zinc_mid = color(source, "texture_zinc_mid");
    let zinc_high = color(source, "texture_zinc_high");
    let brand_base = color(source, "texture_brand_base");
    let zinc_ceiling = number(source, "texture_zinc_ceiling", 187, 254);
    assert!(zinc_ceiling > 0);
    let indigo = blend(
        brand_base,
        colors[6],
        f64::from(number(source, "texture_indigo_tint", 35, 100)) / 100.0,
    );
    let width = (1152u32.div_ceil(grain)).div_ceil(4) * 4;
    let height = 4608u32.div_ceil(grain);
    let mut field = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let px = f64::from(x * grain) / f64::from(scale) + 3.17;
            let py = f64::from(y * grain) / f64::from(scale) + 7.91;
            let wx = fbm(px * 0.71, py * 0.71, seed ^ 0x6395dc21, octaves);
            let wy = fbm(
                px * 0.71 + 13.7,
                py * 0.71 + 4.2,
                seed ^ 0x791dc553,
                octaves,
            );
            let value = fbm(
                px + wx * f64::from(warp) / f64::from(scale),
                py + wy * f64::from(warp) / f64::from(scale),
                seed,
                octaves,
            );
            let level = ((0.5 + value * 1.65).clamp(0.0, 1.0) * f64::from(levels - 1)).round();
            field.push((level * 255.0 / f64::from(levels - 1)).round() as u8);
        }
    }
    fs::write(out.join("texture-field.bin"), &field).unwrap();
    // Bake a seamless morph. Runtime controls speed and interpolates two tiny fields;
    // there are no noise, trigonometry, or per-pixel floating point calculations.
    let frames = 24;
    let mut flow = Vec::with_capacity(field.len() * frames);
    for frame in 0..frames {
        let angle = frame as f64 * std::f64::consts::TAU / frames as f64;
        let (dx, dy) = (angle.sin() * 0.32, (angle.cos() - 1.0) * 0.38);
        for y in 0..height {
            for x in 0..width {
                let px = f64::from(x * grain) / f64::from(scale) + 3.17;
                let py = f64::from(y * grain) / f64::from(scale) + 7.91;
                let wx = fbm(px * 0.71 + dx, py * 0.71 + dy, seed ^ 0x6395dc21, octaves);
                let wy = fbm(
                    px * 0.71 + 13.7 - dy,
                    py * 0.71 + 4.2 + dx,
                    seed ^ 0x791dc553,
                    octaves,
                );
                let value = fbm(
                    px + dx + wx * f64::from(warp) / f64::from(scale),
                    py + dy + wy * f64::from(warp) / f64::from(scale),
                    seed,
                    octaves,
                );
                let level = ((0.5 + value * 1.65).clamp(0.0, 1.0) * f64::from(levels - 1)).round();
                flow.push((level * 255.0 / f64::from(levels - 1)).round() as u8);
            }
        }
    }
    assert_eq!(&flow[..field.len()], field.as_slice());
    fs::write(out.join("texture-flow.bin"), flow).unwrap();
    let mut generated = format!(
        "const CELL_GRAIN: i32 = {grain};\nconst FIELD_WIDTH: i32 = {width};\nconst FIELD_HEIGHT: i32 = {height};\nconst FLOW_FRAMES: usize = {frames};\nstatic FLOW: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/texture-flow.bin\"));\n#[cfg(test)] const ZINC_CEILING: u8 = {zinc_ceiling};\nstatic FIELD: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/texture-field.bin\"));\nconst SURFACES: &[(u32, Palette)] = &[\n"
    );
    for index in [0usize, 1, 2, 6, 7] {
        let rgb = colors[index];
        let colorref = (rgb & 255) << 16 | (rgb & 0xff00) | (rgb >> 16);
        let mut info = Vec::with_capacity(1064);
        info.extend_from_slice(&40u32.to_le_bytes());
        info.extend_from_slice(&(width as i32).to_le_bytes());
        info.extend_from_slice(&(-(height as i32)).to_le_bytes());
        info.extend_from_slice(&1u16.to_le_bytes());
        info.extend_from_slice(&8u16.to_le_bytes());
        for value in [0u32, width * height, 0, 0, 256, 256] {
            info.extend_from_slice(&value.to_le_bytes());
        }
        for level in 0..256 {
            let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
            let gradient = if level <= zinc_ceiling {
                let value = f64::from(level) / f64::from(zinc_ceiling);
                if value < 0.5 {
                    blend(zinc_low, zinc_mid, smooth(value * 2.0))
                } else {
                    blend(zinc_mid, zinc_high, smooth((value - 0.5) * 2.0))
                }
            } else {
                let value = f64::from(level - zinc_ceiling) / f64::from(255 - zinc_ceiling);
                blend(zinc_high, indigo, smooth(value))
            };
            let strength = if index == 0 {
                background_strength
            } else {
                surface_strength
            };
            let pixel = blend(rgb, gradient, f64::from(strength) / 100.0);
            for shift in [0, 8, 16] {
                info.push((pixel >> shift) as u8);
            }
            info.push(0);
        }
        assert_eq!(info.len(), 1064);
        fs::write(out.join(format!("texture-palette-{index}.bin")), &info).unwrap();
        generated.push_str(&format!("(0x{colorref:06x}, Palette({info:?})),\n"));
    }
    generated.push_str("];\nconst SOLIDS: &[u32] = &[\n");
    for rgb in colors {
        let colorref = (rgb & 255) << 16 | (rgb & 0xff00) | (rgb >> 16);
        generated.push_str(&format!("0x{colorref:06x},\n"));
    }
    generated.push_str("];\n");
    fs::write(out.join("texture_data.rs"), generated).unwrap();
}

pub fn generate(out: &Path, source: &str) -> Vec<u32> {
    let pairs = [
        ("BG", "background"),
        ("PANEL", "panel"),
        ("PANEL_ALT", "panel_alt"),
        ("INK", "text"),
        ("MUTED", "muted"),
        ("TEAL", "teal"),
        ("INDIGO", "indigo"),
        ("LINE", "line"),
        ("DANGER", "danger"),
        ("STEM", "logo_stem"),
    ];
    let colors: Vec<_> = pairs.iter().map(|(_, key)| color(&source, key)).collect();
    let generated: String = pairs
        .iter()
        .zip(&colors)
        .map(|((name, _), rgb)| {
            let bgr = (rgb & 255) << 16 | (rgb & 0xff00) | (rgb >> 16);
            format!("const {name}: u32 = 0x{bgr:06x};\n")
        })
        .collect();
    fs::write(out.join("theme.rs"), generated).unwrap();
    textures(out, source, &colors);
    colors
}
