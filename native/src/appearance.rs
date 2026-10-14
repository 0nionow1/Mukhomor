//! Stable appearance identifiers and semantic colors. No process-global mutable theme.
#[allow(dead_code)]
mod base {
    include!(concat!(env!("OUT_DIR"), "/theme.rs"));
    pub const COLORS: [u32; 10] = [
        BG, PANEL, PANEL_ALT, INK, MUTED, TEAL, INDIGO, LINE, DANGER, STEM,
    ];
}

// A semantic token outside the COLORREF range: primary buttons need their own ink.
pub const PRIMARY_INK: u32 = 0x0100_0000;

pub const fn colorref(rgb: u32) -> u32 {
    (rgb & 255) << 16 | (rgb & 0xff00) | (rgb >> 16)
}

/// A user-selected scale must not put the fixed-width client off the monitor.
/// Vertical overflow uses the existing scrollable, compact layouts.
pub fn fit_dpi(requested: i32, work_width: i32) -> i32 {
    if work_width <= 0 {
        return requested;
    }
    requested.min(((work_width - 16).max(280) * 96 / 560).max(48))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Neon,
    Forest,
    Classic,
    Ember,
    Glacier,
    Plum,
    Paper,
    Voltage,
    Toxic,
    Mint,
    Ultraviolet,
    Laser,
    Sunset,
    Cherry,
    Ocean,
    Orchid,
    Arcade,
    Acid,
    Hologram,
    Dragon,
}
impl Theme {
    pub const ALL: [Self; 20] = [
        Self::Neon,
        Self::Classic,
        Self::Plum,
        Self::Glacier,
        Self::Forest,
        Self::Ember,
        Self::Voltage,
        Self::Toxic,
        Self::Mint,
        Self::Ultraviolet,
        Self::Laser,
        Self::Sunset,
        Self::Cherry,
        Self::Ocean,
        Self::Orchid,
        Self::Arcade,
        Self::Acid,
        Self::Hologram,
        Self::Dragon,
        Self::Paper,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Neon => "neon",
            Self::Forest => "forest",
            Self::Classic => "classic",
            Self::Ember => "ember",
            Self::Glacier => "glacier",
            Self::Plum => "plum",
            Self::Paper => "paper",
            Self::Voltage => "voltage",
            Self::Toxic => "toxic",
            Self::Mint => "mint",
            Self::Ultraviolet => "ultraviolet",
            Self::Laser => "laser",
            Self::Sunset => "sunset",
            Self::Cherry => "cherry",
            Self::Ocean => "ocean",
            Self::Orchid => "orchid",
            Self::Arcade => "arcade",
            Self::Acid => "acid",
            Self::Hologram => "hologram",
            Self::Dragon => "dragon",
        }
    }
    pub fn read(id: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| Some(t.id()) == id)
            .unwrap_or_default()
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Neon => "Неон",
            Self::Forest => "Биопанк",
            Self::Classic => "Классика",
            Self::Ember => "Плазма",
            Self::Glacier => "Киберпанк",
            Self::Plum => "Реактор",
            Self::Paper => "Поп-арт",
            Self::Voltage => "Высокий ток",
            Self::Toxic => "Токсик",
            Self::Mint => "Мятный огонь",
            Self::Ultraviolet => "Ультрафиолет",
            Self::Laser => "Лазер",
            Self::Sunset => "Рейв-закат",
            Self::Cherry => "Вишнёвый лёд",
            Self::Ocean => "Глубина",
            Self::Orchid => "Орхидея",
            Self::Arcade => "Аркада",
            Self::Acid => "Кислота",
            Self::Hologram => "Голограмма",
            Self::Dragon => "Дракон",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Neon => "Розовый + неон-teal",
            Self::Forest => "Электрозелёный + фуксия",
            Self::Classic => "Фиолетовый и бирюза",
            Self::Ember => "Оранжевый + электросиний",
            Self::Glacier => "Cyan + красный",
            Self::Plum => "Фиолетовый + лайм",
            Self::Paper => "Фуксия + кобальт на белом",
            Self::Voltage => "Кобальт + неон-жёлтый",
            Self::Toxic => "Лайм + ультрафиолет",
            Self::Mint => "Мята + мандарин",
            Self::Ultraviolet => "Ультрафиолет + cyan",
            Self::Laser => "Жёлтый + горячий розовый",
            Self::Sunset => "Мандарин + маджента",
            Self::Cherry => "Вишня + ледяная мята",
            Self::Ocean => "Лазурь + оранжевый",
            Self::Orchid => "Маджента + кислотный лайм",
            Self::Arcade => "Электросиний + розовый",
            Self::Acid => "Лимон + cyan",
            Self::Hologram => "Неон-мята + сиреневый",
            Self::Dragon => "Красный + электрожёлтый",
        }
    }
    /// Background, panel, raised panel, ink, muted ink, active, primary,
    /// border, danger, mushroom stem, primary-button ink; all COLORREF.
    pub fn colors(self) -> [u32; 11] {
        if self == Self::Neon {
            let mut colors = [0; 11];
            colors[..10].copy_from_slice(&base::COLORS);
            colors[10] = base::COLORS[0];
            return colors;
        }
        let rgb = match self {
            Self::Classic => [
                0x101014, 0x1B1C2B, 0x242736, 0xEEEEFA, 0xACAABD, 0x2ADECF, 0x604AC5, 0x3B3D50,
                0xF28FA0, 0xE9E6DA, 0xFFFFFF,
            ],
            Self::Ember => [
                0x140E18, 0x241826, 0x332236, 0xFFF2E8, 0xC5B2C9, 0x65A6FF, 0xFF8A24, 0x64416C,
                0xFF91A9, 0xFFF0DF, 0x180D14,
            ],
            Self::Glacier => [
                0x07141B, 0x0D222C, 0x15313C, 0xEAFCFF, 0xA4C3CF, 0xFF6575, 0x20E8FF, 0x28616D,
                0xFFA2B5, 0xE5FFFF, 0x04151B,
            ],
            Self::Plum => [
                0x160B21, 0x241334, 0x321D46, 0xF6ECFF, 0xC7AFDB, 0xD5FF26, 0xB878FF, 0x624080,
                0xFF94B5, 0xF3FFE0, 0x170923,
            ],
            Self::Paper => [
                0xFFF5FF, 0xF3E8FF, 0xE5D9F5, 0x220F38, 0x655079, 0x1743C9, 0xBF007F, 0xA884CB,
                0xAE1747, 0xFFFFFF, 0xFFFFFF,
            ],
            Self::Forest => [
                0x08180F, 0x10281C, 0x1B3829, 0xECFFF1, 0xA9CDB5, 0xFF62D8, 0x57FF85, 0x346C4C,
                0xFF9C9F, 0xE8FFE4, 0x06190E,
            ],
            Self::Voltage => [
                0x090F25, 0x131E38, 0x1D2B48, 0xF2F5FF, 0xAFBEDC, 0xF5FF29, 0x678AFF, 0x415B89,
                0xFF95B2, 0xFFFFDF, 0x060C20,
            ],
            Self::Toxic => [
                0x121708, 0x202811, 0x2D371C, 0xF7FFE8, 0xBFCDA3, 0xC98AFF, 0xC6FF18, 0x596C30,
                0xFF979F, 0xF6FFE0, 0x111806,
            ],
            Self::Mint => [
                0x071A19, 0x0D2B29, 0x183C36, 0xE8FFF7, 0xA7CEC4, 0xFF9648, 0x10F5CC, 0x2F7062,
                0xFF9FAE, 0xF0FFE4, 0x041A16,
            ],
            Self::Ultraviolet => [
                0x110B25, 0x1F153A, 0x2D204E, 0xF4EEFF, 0xBCB0DD, 0x35E8FF, 0xB978FF, 0x59418B,
                0xFF92B7, 0xEEE8FF, 0x120820,
            ],
            Self::Laser => [
                0x181409, 0x282112, 0x382E1B, 0xFFFCE6, 0xD0C1A1, 0xFF68D0, 0xF9FF32, 0x756234,
                0xFF999F, 0xFFFFDC, 0x191304,
            ],
            Self::Sunset => [
                0x210B1B, 0x341327, 0x461D36, 0xFFF0E9, 0xD6AEC4, 0xFF60E5, 0xFF9C32, 0x834064,
                0xFFA6B3, 0xFFF1D9, 0x210B12,
            ],
            Self::Cherry => [
                0x210C16, 0x331520, 0x461F2D, 0xFFF0F3, 0xD3AFBA, 0x49FFD5, 0xFF526E, 0x7D3D54,
                0xFFB29F, 0xEEFFF7, 0x200611,
            ],
            Self::Ocean => [
                0x071523, 0x0D2538, 0x17354A, 0xECF9FF, 0xA7C1D8, 0xFFA344, 0x35DCFF, 0x326380,
                0xFF9AB5, 0xE8FAFF, 0x041522,
            ],
            Self::Orchid => [
                0x1B0920, 0x2D1233, 0x3E1C45, 0xFDEEFF, 0xD1ADD5, 0x96FF35, 0xF564FF, 0x774181,
                0xFFA2B3, 0xF6FFE6, 0x1C0721,
            ],
            Self::Arcade => [
                0x0D1024, 0x191D38, 0x252A4A, 0xF1F1FF, 0xB4B8DC, 0xFF79CA, 0x7595FF, 0x494F88,
                0xFFA0AF, 0xF4ECFF, 0x080D24,
            ],
            Self::Acid => [
                0x101908, 0x1C2910, 0x29391B, 0xF8FFE7, 0xC2D0A6, 0x29F2FF, 0xE6FF24, 0x586D32,
                0xFF9CA8, 0xFFFFDC, 0x111905,
            ],
            Self::Hologram => [
                0x0C1821, 0x152835, 0x203947, 0xEDFFFC, 0xB0CAD6, 0xC6A1FF, 0x66FFCF, 0x42677B,
                0xFF9DBB, 0xEEFFF9, 0x071B1D,
            ],
            Self::Dragon => [
                0x210F0B, 0x321B13, 0x45271C, 0xFFF2E4, 0xD4B7A6, 0xF2FF38, 0xFF643C, 0x80523B,
                0xFF9FC1, 0xFFF7D8, 0x200C05,
            ],
            Self::Neon => unreachable!(),
        };
        rgb.map(colorref)
    }
    pub fn color(self, token: u32) -> u32 {
        if token == PRIMARY_INK {
            return self.colors()[10];
        }
        base::COLORS
            .iter()
            .position(|c| *c == token)
            .map_or(token, |index| self.colors()[index])
    }
    /// Retain the indexed texture field; only its tiny color table changes.
    pub fn texture_color(self, token: u32, level: usize, original: u32) -> u32 {
        if self == Self::Neon {
            return original;
        }
        let colors = self.colors();
        let index = base::COLORS.iter().position(|c| *c == token).unwrap_or(0);
        // A single low-contrast ramp erased the lava shapes in non-default
        // presets. Preserve the original three shading bands and accent tail.
        let (low, middle, high, accent) = if self == Self::Classic {
            (
                colorref(0x09090B),
                colorref(0x18181B),
                colorref(0x27272A),
                mix(colorref(0x0E0E11), colorref(0x6A55DF), 89),
            )
        } else if self == Self::Paper {
            (
                colors[0],
                colors[1],
                colors[2],
                mix(colors[2], colors[7], 70),
            )
        } else {
            (
                mix(colors[0], 0, 90),
                mix(colors[1], colors[6], 18),
                mix(colors[2], colors[5], 22),
                mix(colors[2], colors[6], 100),
            )
        };
        let level = level.min(255) as u32;
        let gradient = if level <= 93 {
            mix(low, middle, smooth_weight(level, 93))
        } else if level <= 187 {
            mix(middle, high, smooth_weight(level - 93, 94))
        } else {
            mix(high, accent, smooth_weight(level - 187, 68))
        };
        let strength = if index == 0 { 56 } else { 12 };
        mix(colors[index], gradient, strength * 255 / 100)
    }
}
fn smooth_weight(value: u32, span: u32) -> u32 {
    let t = value * 255 / span;
    t * t * (765 - 2 * t) / (255 * 255)
}
fn mix(a: u32, b: u32, strength: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |rgb, shift| {
        rgb | ((((a >> shift) & 255) * (255 - strength) + ((b >> shift) & 255) * strength) / 255)
            << shift
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InterfaceSize {
    Small,
    #[default]
    Medium,
    Large,
}
impl InterfaceSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];
    pub fn id(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
    pub fn read(id: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| Some(s.id()) == id)
            .unwrap_or_default()
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Small => "Маленький",
            Self::Medium => "Средний",
            Self::Large => "Большой",
        }
    }
    pub fn percent(self) -> i32 {
        match self {
            Self::Small => 85,
            Self::Medium => 100,
            Self::Large => 125,
        }
    }
    pub fn dpi(self, monitor_dpi: i32) -> i32 {
        (monitor_dpi.clamp(48, 768) * self.percent() + 50) / 100
    }
    /// Density choices stay visibly distinct on coarse pixel grids. Single-cell
    /// tiny artwork still rounds to nearest so short-window buttons cannot overflow.
    pub fn cell(self, logical: i32, dpi: i32) -> i32 {
        let rounding = match (self, logical) {
            (_, 1) | (Self::Medium, _) => 48,
            (Self::Small, _) => 0,
            (Self::Large, _) => 95,
        };
        ((logical * dpi + rounding) / 96).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(rgb: u32) -> f64 {
        [0.2126, 0.7152, 0.0722]
            .into_iter()
            .enumerate()
            .map(|(i, weight)| {
                let c = ((rgb >> (i * 8)) & 255) as f64 / 255.0;
                weight
                    * if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }
    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn all_presets_have_readable_text_and_unique_stable_ids() {
        for theme in Theme::ALL {
            assert_eq!(Theme::read(Some(theme.id())), theme);
            let c = theme.colors();
            for bg in &c[..3] {
                for fg in [c[3], c[4], c[5], c[8]] {
                    assert!(
                        contrast(fg, *bg) >= 4.5,
                        "{theme:?}: {fg:x} on {bg:x}: {}",
                        contrast(fg, *bg)
                    );
                }
            }
            assert!(contrast(c[10], c[6]) >= 4.5, "{theme:?} primary ink");
        }
        assert_eq!(Theme::read(Some("future-theme")), Theme::Neon);
    }
    #[test]
    fn size_composes_with_system_dpi() {
        for dpi in [96, 120, 144, 192, 240, 288] {
            assert!(InterfaceSize::Small.dpi(dpi) < dpi);
            assert_eq!(InterfaceSize::Medium.dpi(dpi), dpi);
            assert!(InterfaceSize::Large.dpi(dpi) > dpi);
            let cells = InterfaceSize::ALL.map(|size| size.cell(2, size.dpi(dpi)));
            assert!(
                cells[0] < cells[1] && cells[1] < cells[2],
                "icons at monitor DPI {dpi}: {cells:?}"
            );
        }
        assert_eq!(InterfaceSize::read(Some("bad")), InterfaceSize::Medium);
        assert_eq!(fit_dpi(240, 1920), 240);
        assert!(560 * fit_dpi(240, 1024) / 96 <= 1008);
    }
}
