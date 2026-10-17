// Approved full and optical mini artwork; colors follow every application theme.
// Kept independent of Win32 so the EXE resource and runtime icons share pixels.
include!("mushroom_data.rs");

pub const fn palette_index(pixel: u8) -> Option<usize> {
    match pixel {
        b'R' => Some(0),
        b'H' => Some(1),
        b'E' => Some(2),
        b'D' => Some(3),
        b'W' => Some(4),
        b'V' => Some(5),
        b'Q' => Some(6),
        b'S' => Some(7),
        b'L' => Some(8),
        b'A' => Some(9),
        b'B' => Some(10),
        b'M' => Some(11),
        b'N' => Some(12),
        b'J' => Some(13),
        _ => None,
    }
}

// Channel-wise blending works with both RGB and Windows COLORREF ordering.
fn mix(first: u32, second: u32, percent: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |color, shift| {
        let a = (first >> shift) & 255;
        let b = (second >> shift) & 255;
        color | ((a * (100 - percent) + b * percent + 50) / 100) << shift
    })
}
/// Live theme colors use Windows COLORREF order (R in the low byte).
pub fn palette(
    bg: u32,
    primary: u32,
    secondary: u32,
    stem: u32,
    light: bool,
) -> [u32; PALETTE_LEN] {
    let mut colors = [0; PALETTE_LEN];
    colors[..14].copy_from_slice(&[
        primary,
        mix(primary, 0xffffff, 24),
        mix(primary, bg, 16),
        mix(primary, bg, 30),
        secondary,
        mix(secondary, 0xffffff, 16),
        mix(secondary, primary, 15),
        stem,
        mix(stem, 0xffffff, 60),
        mix(stem, bg, 21),
        mix(stem, bg, 12),
        mix(secondary, stem, 46),
        mix(secondary, bg, if light { 17 } else { 35 }),
        mix(secondary, stem, 70),
    ]);
    for (i, &(reference, family)) in APPROVED_COLORS.iter().enumerate() {
        colors[14 + i] = retint(reference, [0xb952ff, 0xd1f516, 0xf4fff3][family], [primary, secondary, stem][family]);
    }
    colors
}

// Preserve the approved Neon pixels exactly. Other themes replace each material
// color while keeping its light, shadow and small tonal variations.
fn retint(reference: u32, base: u32, themed: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |color, shift| {
        let r = (reference >> shift) & 255;
        let b = (base >> shift) & 255;
        let t = (themed >> shift) & 255;
        let value = if r <= b { (t * r + b / 2) / b.max(1) } else { t + ((255 - t) * (r - b) + (255 - b) / 2) / (255 - b).max(1) };
        color | value << shift
    })
}

/// Executable resources use RGB integers; the live UI uses COLORREF integers.
/// Convert at this boundary so the approved shades are identical in both.
pub fn rgb_palette(bg: u32, primary: u32, secondary: u32, stem: u32, light: bool) -> [u32; PALETTE_LEN] {
    let swap = |c: u32| (c & 255) << 16 | (c & 0xff00) | (c >> 16);
    palette(swap(bg), swap(primary), swap(secondary), swap(stem), light).map(swap)
}

pub fn hero_spot(row: usize, col: usize) -> Option<usize> {
    if !matches!(*HERO.get(row)?.get(col)?, b'W' | b'V' | b'Q') { return None; }
    HERO_SPOTS.iter().position(|&(left, top, width, height)| {
        (left..left + width).contains(&col) && (top..top + height).contains(&row)
    }).or_else(|| HERO_SPOTS.iter().enumerate().min_by_key(|&(_, &(left, top, width, height))| {
        (col * 2).abs_diff(left * 2 + width) + (row * 2).abs_diff(top * 2 + height)
    }).map(|(index, _)| index))
}

pub fn hero_palette_index(row: usize, col: usize) -> Option<usize> {
    tone_index(HERO_TONES[row][col])
}
pub const fn tone_index(tone: u8) -> Option<usize> {
    if tone == 255 { None } else { Some(tone as usize) }
}

// Optically drawn 16/24/32px versions keep clear spot gaps and intentional
// padding; larger system icons use exact integer multiples when possible.
pub fn icon_palette_index(x: usize, y: usize, size: usize) -> Option<usize> {
    assert!((1..=256).contains(&size) && x < size && y < size);
    let tone = if size <= 20 {
        MICRO_TONES[(2 * y + 1) * 16 / (2 * size)][(2 * x + 1) * 16 / (2 * size)]
    } else if size == 24 || size == 48 || size == 96 {
        COMPACT_TONES[(2 * y + 1) * 24 / (2 * size)][(2 * x + 1) * 24 / (2 * size)]
    } else {
        MINI_TONES[(2 * y + 1) * 32 / (2 * size)][(2 * x + 1) * 32 / (2 * size)]
    };
    tone_index(tone)
}

// A 32-bit icon resource DIB, including its Windows transparency mask.
pub fn icon_dib(size: usize, palette_rgb: &[u32; PALETTE_LEN]) -> Vec<u8> {
    assert!((1..=256).contains(&size));
    let mut pixels = vec![0u8; size * size * 4];
    let mask_row = size.div_ceil(32) * 4;
    let mut mask = vec![0u8; mask_row * size];
    for y in 0..size {
        for x in 0..size {
            let i = ((size - y - 1) * size + x) * 4;
            if let Some(index) = icon_palette_index(x, y, size) {
                let rgb = palette_rgb[index];
                pixels[i..i + 4].copy_from_slice(&[
                    rgb as u8,
                    (rgb >> 8) as u8,
                    (rgb >> 16) as u8,
                    255,
                ]);
            } else {
                mask[(size - y - 1) * mask_row + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mut dib = Vec::with_capacity(40 + pixels.len() + mask.len());
    for value in [40u32, size as u32, (size * 2) as u32] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    for value in [0u32, (pixels.len() + mask.len()) as u32, 0, 0, 0, 0] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&pixels);
    dib.extend_from_slice(&mask);
    dib
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_artwork_has_valid_colors_and_mycelium_below_the_stem() {
        assert!(
            MINI.iter()
                .flat_map(|row| row.iter())
                .chain(HERO.iter().flat_map(|row| row.iter()))
                .all(|&p| p == b'.' || palette_index(p).is_some())
        );
        assert!(
            HERO[48..]
                .iter()
                .flat_map(|row| row.iter())
                .any(|&p| p == b'M')
        );
        assert!(
            HERO[48..]
                .iter()
                .flat_map(|row| row.iter())
                .any(|&p| p == b'N')
        );
        let mut spot_pixels = [0; HERO_SPOTS.len()];
        for (row, line) in HERO.iter().enumerate() {
            for (col, &pixel) in line.iter().enumerate() {
                if matches!(pixel, b'W' | b'V' | b'Q') {
                    spot_pixels[hero_spot(row, col).expect("every spot must animate")] += 1;
                }
            }
        }
        assert!(spot_pixels.into_iter().all(|count| count > 0));
        // A stable fingerprint protects the approved silhouette, spots and threads.
        let fingerprint = |rows: Vec<&[u8]>| {
            rows.into_iter()
                .flatten()
                .fold(0xcbf29ce484222325u64, |hash, &p| {
                    (hash ^ p as u64).wrapping_mul(0x100000001b3)
                })
        };
        assert_eq!(
            fingerprint(MINI.iter().map(|row| &row[..]).collect()),
            0x5302d2c1b083e085
        );
        assert_eq!(
            fingerprint(HERO.iter().map(|row| &row[..]).collect()),
            0x90827ecdbfa06661
        );
    }

    #[test]
    fn accent_roles_and_approved_shading_follow_the_palette() {
        let colors = palette(0x111c18, 0xef9273, 0xb5d58f, 0xf2e9d5, false);
        assert_eq!(colors[palette_index(b'R').unwrap()], 0xef9273);
        assert_eq!(colors[palette_index(b'W').unwrap()], 0xb5d58f);
        assert_eq!(colors[palette_index(b'S').unwrap()], 0xf2e9d5);
        assert_eq!(colors[palette_index(b'H').unwrap()], 0xf3ac95);
        let swapped = palette(0x111c18, 0xb5d58f, 0xef9273, 0xf2e9d5, false);
        assert_eq!(swapped[0], colors[4]);
        assert_eq!(swapped[4], colors[0]);
        assert_ne!(swapped[11], colors[11]);
    }

    #[test]
    fn approved_tonal_pixels_are_exact_in_neon_and_follow_other_themes() {
        let neon = palette(0x1b0d16, 0xb952ff, 0xd1f516, 0xf4fff3, false);
        let resource = rgb_palette(0x160d1b, 0xff52b9, 0x16f5d1, 0xf3fff4, false);
        let themed = palette(0x111c18, 0xef9273, 0xb5d58f, 0xf2e9d5, false);
        for (i, &(reference, _)) in APPROVED_COLORS.iter().enumerate() {
            assert_eq!(neon[14 + i], reference);
            assert_eq!(resource[14 + i], (reference & 255) << 16 | (reference & 0xff00) | (reference >> 16));
            assert_ne!(themed[14 + i], reference);
        }
        for y in 0..HERO_HEIGHT {
            for x in 0..HERO_WIDTH {
                assert_eq!(hero_palette_index(y,x).is_none(), HERO[y][x] == b'.');
                if let Some(index) = hero_palette_index(y,x) { assert!(index < PALETTE_LEN); }
            }
        }
    }

    #[test]
    fn optical_icon_sizes_preserve_materials_and_a_matching_alpha_mask() {
        let colors = palette(0x111c18, 0xef9273, 0xb5d58f, 0xf2e9d5, false);
        for size in [16usize, 20, 24, 32, 40, 48, 64, 96, 128, 256] {
            let dib = icon_dib(size, &colors);
            let mask_row = size.div_ceil(32) * 4;
            assert_eq!(dib.len(), 40 + size * size * 4 + mask_row * size);
            let mut counts = [0usize; 3];
            let (mut left, mut top, mut right, mut bottom) = (size, size, 0, 0);
            for y in 0..size {
                for x in 0..size {
                    let i = 40 + (y * size + x) * 4;
                    let opaque = dib[i + 3] == 255;
                    let masked =
                        dib[40 + size * size * 4 + y * mask_row + x / 8] & (0x80 >> (x % 8)) != 0;
                    assert_eq!(opaque, !masked);
                    if opaque {
                        left = left.min(x);
                        top = top.min(y);
                        right = right.max(x + 1);
                        bottom = bottom.max(y + 1);
                        let rgb =
                            dib[i] as u32 | (dib[i + 1] as u32) << 8 | (dib[i + 2] as u32) << 16;
                        assert!(colors.contains(&rgb));
                        let index = if rgb & 255 > (rgb >> 8) & 255 { 0 } else if (rgb & 255) < 150 { 1 } else { 2 };
                        counts[index] += 1;
                    }
                }
            }
            assert!(counts.into_iter().sum::<usize>() > size * size / 4, "size {size}");
            assert!(right - left >= size * 3 / 4 && bottom - top >= size * 3 / 4, "size {size}");
            // Intentional padding and transparent corners protect the silhouette.
            for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
                assert_eq!(dib[40 + (y * size + x) * 4 + 3], 0, "size {size}");
            }
        }
    }
}

// Shared by both UI renderers; retain the original 12-pixel icon grids.
pub fn caption_grid(close: bool) -> &'static [&'static str; 8] {
    if close {
        &[
            "XX....XX", "XXX..XXX", ".XXXXXX.", "..XXXX..", "..XXXX..", ".XXXXXX.", "XXX..XXX",
            "XX....XX",
        ]
    } else {
        &[
            "........", "........", "........", "XXXXXXXX", "XXXXXXXX", "........", "........",
            "........",
        ]
    }
}

pub fn icon_grid(kind: usize) -> &'static [&'static str; 12] {
    if kind == 0 { return &NAV_LOGO; }
    const GRIDS: [[&str; 12]; 11] = [
        // The logo's broad stepped cap, three spots and flared stem, reduced
        // to the same monochrome 12px grid as the other navigation glyphs.
        [
            "............",
            "....XXXX....",
            "..XXXXXXXX..",
            ".XX..XXXXXX.",
            "XXX..XX..XXX",
            "X..XXXX..XXX",
            "XXXXXXXXXXXX",
            "....X..X....",
            "....X..X....",
            "...XX..XX...",
            "...XXXXXX...",
            "............",
        ],
        [
            "............",
            "...XX.......",
            "XXXXXXXXXXXX",
            "...XX.......",
            "............",
            "........XX..",
            "XXXXXXXXXXXX",
            "........XX..",
            "............",
            "..XX........",
            "XXXXXXXXXXXX",
            "..XX........",
        ],
        [
            "....XXXX....",
            "....X..X....",
            "....XXXX....",
            ".....XX.....",
            ".....XX.....",
            "..XXXXXXXX..",
            "..X......X..",
            "..X......X..",
            "XXXX....XXXX",
            "X..X....X..X",
            "XXXX....XXXX",
            "............",
        ],
        [
            ".......XX...",
            "......XXX...",
            ".....XXX....",
            ".....XXX...X",
            ".....XXX..XX",
            ".....XXXXXXX",
            "....XXXXXXX.",
            "...XXXXX....",
            "..XXXX......",
            ".XXXX.......",
            ".XXX........",
            "..X.........",
        ],
        [
            "............",
            "..........XX",
            ".........XXX",
            "........XXX.",
            ".......XXX..",
            "XX....XXX...",
            "XXX..XXX....",
            ".XXXXXX.....",
            "..XXXX......",
            "...XX.......",
            "............",
            "............",
        ],
        [
            ".....XX.....",
            ".....XX.....",
            ".....XX.....",
            ".XX..XX..XX.",
            ".XX..XX..XX.",
            ".XX..XX..XX.",
            ".XX......XX.",
            ".XX......XX.",
            ".XX......XX.",
            ".XX......XX.",
            ".XXXXXXXXXX.",
            ".XXXXXXXXXX.",
        ],
        [
            "............",
            ".XX......XX.",
            ".XXX....XXX.",
            "..XXX..XXX..",
            "...XXXXXX...",
            "....XXXX....",
            "....XXXX....",
            "...XXXXXX...",
            "..XXX..XXX..",
            ".XXX....XXX.",
            ".XX......XX.",
            "............",
        ],
        [
            "............",
            "............",
            "............",
            "............",
            "............",
            "............",
            ".XXXXXXXXXX.",
            ".XXXXXXXXXX.",
            "............",
            "............",
            "............",
            "............",
        ],
        [
            "............",
            "......XX....",
            ".......XX...",
            "........XX..",
            ".........XX.",
            ".XXXXXXXXXXX",
            ".XXXXXXXXXXX",
            ".........XX.",
            "........XX..",
            ".......XX...",
            "......XX....",
            "............",
        ],
        // Own VPS: two rack units with indicator lights.
        [
            "............",
            ".XXXXXXXXXX.",
            ".X........X.",
            ".X.XX..XX.X.",
            ".X........X.",
            ".XXXXXXXXXX.",
            "............",
            ".XXXXXXXXXX.",
            ".X........X.",
            ".X.XX..XX.X.",
            ".X........X.",
            ".XXXXXXXXXX.",
        ],
        // P2P: equal peers joined directly, without a central server.
        [
            "............",
            "XXXX........",
            "X..X........",
            "X..X........",
            "XXXX........",
            "..XX........",
            "..XXXXXXXX..",
            "........XX..",
            "........XXXX",
            "........X..X",
            "........X..X",
            "........XXXX",
        ],
    ];
    &GRIDS[kind.min(GRIDS.len() - 1)]
}
