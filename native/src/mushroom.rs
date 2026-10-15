// Compact logo 04 and its stepped mycelium button; only four bottom tips soften.
// One silhouette in every theme. R = primary, W = secondary, S = stem.
// Kept independent of Win32 so the EXE resource and runtime icons share pixels.
pub const PALETTE_LEN: usize = 14;

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
pub fn palette(
    bg: u32,
    primary: u32,
    secondary: u32,
    stem: u32,
    light: bool,
) -> [u32; PALETTE_LEN] {
    [
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
    ]
}

pub const LOGO_WIDTH: usize = 24;
pub const LOGO_HEIGHT: usize = 24;
pub const MINI: [&[u8; LOGO_WIDTH]; LOGO_HEIGHT] = [
    b"........................",
    b"........................",
    b"........................",
    b"........................",
    b".........RRRRRR.........",
    b".......RRRRRRRRRR.......",
    b".....RRWWWRRRRRRRRR.....",
    b"....RRRWWWRRRRRRRRRR....",
    b"...RRRRRRRRRWWWWWRRRR...",
    b"...RWWWRRRRRWWWWWRRRR...",
    b"...RWWWRRRRRWWWWRRRRR...",
    b"...RRRRRRRRRRRRRRRRRR...",
    b".........SSSSSS.........",
    b".........SSSSSS.........",
    b".........SSSSSS.........",
    b"........SSSSSSSS........",
    b".......SSSSSSSSSS.......",
    b".......SSSSSSSSSS.......",
    b"........................",
    b"........................",
    b"........................",
    b"........................",
    b"........................",
    b"........................",
];

pub const HERO_WIDTH: usize = 80;
pub const HERO_HEIGHT: usize = 72;
// Bounds (left, top, width, height) identify the three independently animated
// spots in HERO. All treads are square; only four bottom tips lose one pixel.
pub const HERO_SPOTS: [(usize, usize, usize, usize); 3] =
    [(28, 10, 8, 6), (20, 18, 8, 6), (41, 15, 12, 8)];
pub fn hero_spot(row: usize, col: usize) -> Option<usize> {
    HERO_SPOTS.iter().position(|&(left, top, width, height)| {
        (left..left + width).contains(&col) && (top..top + height).contains(&row)
    })
}
pub const HERO: [&[u8; HERO_WIDTH]; HERO_HEIGHT] = [
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
    b"...............................HHHHHHHHHHHHHHHHRR...............................",
    b"...............................HHHHHHHHHHHHHHHHRR...............................",
    b"...............................HHHHHHHHHHHHHHHHRR...............................",
    b".........................HHHHHHRRRRRRRRRRRRRRRRRRRRRRRR.........................",
    b".........................RHHHVVVVVVHRRRRRRRRRRRRRRRRRRR.........................",
    b".........................RRRWWWWWWWWRRRRRRRRRRRRRRRRRRR.........................",
    b"...................HHHHHHRRRWWWWWWWWRRRRRRRRRRRRRRRRRRRRRRRRR...................",
    b"...................RRRRRRRRRWWWWWWWWRRRRRRRRRRRRRRRRRRRRRRRRR...................",
    b"...................RRHHHRRRRWWWWWWWWRRRRRRRRRRRRRRRRRRRRRRRRR...................",
    b"................HHHRRHHHRRRRRQQQQQQRRRRRRRVVVVVVVVVVRRRRRRRRRREE................",
    b"................RRRRRRRRRRRRRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREE................",
    b"................RRRRRRRRRRRRRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREE................",
    b".............HHHRHHRRVVVVVVRRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREEEEE.............",
    b".............RRRRHHRWWWWWWWWRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREEEEE.............",
    b".............RRRRHHRWWWWWWWWRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREEEEE.............",
    b".............RRRRRRRWWWWWWWWRRRRRRRRRRRRRWWWWWWWWWWWWRRRRRRRRREEEEE.............",
    b".............RRRRRRRWWWWWWWWRRRRRRRRRRRRRRQQQQQQQQQQRRRRRRRRRREEEEE.............",
    b".............RRRRRRRRQQQQQQRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRREEEEE.............",
    b".............RRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRREEEEE.............",
    b".............RRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRREEEEE.............",
    b".............EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE.............",
    b".............EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE.............",
    b".............DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD.............",
    b"..............DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD..............",
    b"...............................AAAAAAAAAAAAAAAAAA...............................",
    b"...............................LSSSSSSSSSSSSSSSAA...............................",
    b"...............................LSSSSSSSSSSSSSSSAA...............................",
    b"...............................LSSSSSSSSSSSSSSSAA...............................",
    b"...............................LSSSSSSSSSSSSSSSAA...............................",
    b"...............................LSLLSSSSSSSSSSSSAA...............................",
    b"...............................LSLLSSSSSSSSSSSSAA...............................",
    b"...............................LSLLSSSSSSSSSSSSAA...............................",
    b"...............................LSLLSSSSSSSSSSSSAA...............................",
    b"............................LSSSSLLSSSSSSSSSSSSAAAAA............................",
    b"............................LSSSSLLSSSSSSSSSSSSAAAAA............................",
    b"............................LSSSSLLSSSSSSSSSSSSAAAAA............................",
    b".........................LSSSSSSSLLSSSSSSSSSSSSAAAAAAAA.........................",
    b".........................LSSSSSSSSSSSSSSSSSSSSSAAAAAAAA.........................",
    b".........................LSSSSSSSSSSSLLLLLLSSSSAAAAAAAA.........................",
    b".........................LSSSSSSSSSSSSSSSSSSSSSAAAAAAAA.........................",
    b".........................LBBBBBBBBBBBBBBBBBBBBBAAAAAAAA.........................",
    b"..........................BBBBBBBBBBBBBBBBBBBBBAAAAAAA..........................",
    b"............................JJJ..JJ.....JJ...M..JJJ.............................",
    b"..........................MMJJ...J......JJ...MM.JJJM...NNNNN....................",
    b"...............NNNNN....MMM.JJ...J......JJ....M..JJMNNNN...NNN..NNNNN...........",
    b".............NNN...NNNNNM.......MM......JJ....M......M.......NNNN...NNN.........",
    b"........NNNNNN.......MM.........M........M....M......MM...............N.........",
    b"....................MM.........MM........MM...MM......MM........................",
    b".................MMMM........NNN..........M....M.......NMMM.....................",
    b"..............MNMM.........NNN.M..........M....MM.....NN..MMMM..................",
    b".............MMN..........NN...M.........MM.....M.....N......MM.................",
    b"............MM.NN........NN....M.........M......M....NN.......MM................",
    b"......MMMMMMM...N........N.....M........NN.....MM....NN........NMMM.............",
    b"................N.......NN....MNN.....NNNMM....M......N........N..MMM...........",
    b"...............NN....NNNN...MMM.NNN...N...M...NN......NN......NN................",
    b".............NNN....NN.....MM.....NN..N...MM.NNMM......N......N.................",
    b"............NN............MM.......N.NN....MNN..M......N......NNN...............",
    b"............N.............M........N.N.....MN...MM....NN........NNN.............",
    b".........................MM........N.N....MM.....M....N...........N.............",
    b".........................M.........N......M.....MM....N.........................",
    b"...................................N......M.....M...............................",
    b"..........................................M.....M...............................",
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
    b"................................................................................",
];

// UI artwork includes layout padding. Windows supplies its own spacing around
// taskbar, tray and shortcut icons, so only the visible mark belongs in the DIB.
// Fit its bounds to the full square canvas; nearest sampling keeps the pixel
// edges and the three color regions crisp even at 16px.
const ICON_BOUNDS: (usize, usize, usize, usize) = {
    let (mut left, mut top, mut right, mut bottom) = (LOGO_WIDTH, LOGO_HEIGHT, 0, 0);
    let mut y = 0;
    while y < LOGO_HEIGHT {
        let mut x = 0;
        while x < LOGO_WIDTH {
            if MINI[y][x] != b'.' {
                if x < left {
                    left = x;
                }
                if y < top {
                    top = y;
                }
                if x + 1 > right {
                    right = x + 1;
                }
                if y + 1 > bottom {
                    bottom = y + 1;
                }
            }
            x += 1;
        }
        y += 1;
    }
    (left, top, right - left, bottom - top)
};

/// Sample the visible mark without the layout padding used inside the window.
/// Shared by executable resources and the Slint taskbar/tray image.
pub fn icon_palette_index(x: usize, y: usize, size: usize) -> Option<usize> {
    assert!((1..=256).contains(&size) && x < size && y < size);
    let (left, top, width, height) = ICON_BOUNDS;
    palette_index(
        MINI[top + (2 * y + 1) * height / (2 * size)][left + (2 * x + 1) * width / (2 * size)],
    )
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
            0x29d2d62f8b93e62f
        );
        assert_eq!(
            fingerprint(HERO.iter().map(|row| &row[..]).collect()),
            0x82011c1516184bd4
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
    fn all_native_icon_sizes_fill_the_canvas_with_three_colors_and_a_matching_alpha_mask() {
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
                        let index = [colors[0], colors[4], colors[7]]
                            .iter()
                            .position(|&c| c == rgb)
                            .unwrap();
                        counts[index] += 1;
                    }
                }
            }
            assert!(counts.into_iter().all(|count| count >= 3), "size {size}");
            assert_eq!(
                (left, top, right, bottom),
                (0, 0, size, size),
                "size {size}"
            );
            // Transparent corners retain the mushroom silhouette, even though
            // its extremities now reach every edge of the system icon canvas.
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
    const GRIDS: [[&str; 12]; 9] = [
        [
            "............",
            ".....XX.....",
            "....XXXX....",
            "...XX..XX...",
            "..XX....XX..",
            ".XX......XX.",
            "..X......X..",
            "..X..XX..X..",
            "..X..XX..X..",
            "..X..XX..X..",
            "..XXXXXXXX..",
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
    ];
    &GRIDS[kind.min(GRIDS.len() - 1)]
}
