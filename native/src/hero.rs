// Connection feedback stays local to the owner-drawn button. No network state
// is inferred from animation time; only the service can confirm a connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HeroPhase {
    #[default]
    Idle,
    Connecting,
    Connected,
    Disconnecting,
    Recovery,
    Error,
}
impl HeroPhase {
    fn read(phase: &str, error: bool) -> Self {
        match phase {
            "connecting" => Self::Connecting,
            "connected" => Self::Connected,
            "disconnecting" => Self::Disconnecting,
            "recovery" => Self::Recovery,
            _ if error => Self::Error,
            _ => Self::Idle,
        }
    }
    fn pending(self) -> bool {
        matches!(self, Self::Connecting | Self::Disconnecting)
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct HeroMotion {
    phase: HeroPhase,
    elapsed_ms: u32,
    roots_at_transition: u16,
}
impl HeroMotion {
    fn sync(&mut self, phase: HeroPhase) -> bool {
        if self.phase == phase {
            return false;
        }
        self.roots_at_transition = self.roots(true);
        self.phase = phase;
        self.elapsed_ms = 0;
        true
    }
    fn advance(&mut self, millis: u32) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(millis);
    }
    fn animating(self) -> bool {
        matches!(
            self.phase,
            HeroPhase::Idle
                | HeroPhase::Connecting
                | HeroPhase::Connected
                | HeroPhase::Disconnecting
        )
    }
    fn roots(self, animate: bool) -> u16 {
        match self.phase {
            // Confirmation starts a short completion wave, never a jump to full roots.
            HeroPhase::Connected if animate => {
                let t = smooth_progress(self.elapsed_ms.min(800), 800);
                self.roots_at_transition
                    + ((1000 - u32::from(self.roots_at_transition)) * t / 1000) as u16
            }
            HeroPhase::Connected => 1000,
            HeroPhase::Connecting if animate => {
                // A readable opening sequence, then continued slow growth while
                // the service is working. Time alone can never complete it.
                let elapsed = u64::from(self.elapsed_ms);
                if elapsed <= 3600 {
                    (800 * elapsed / 3600) as u16
                } else {
                    (800 + 140 * (elapsed - 3600) / (elapsed - 3600 + 9000)) as u16
                }
            }
            HeroPhase::Connecting => 450,
            HeroPhase::Disconnecting if animate => {
                (u32::from(self.roots_at_transition)
                    * (1000 - smooth_progress(self.elapsed_ms.min(700), 700))
                    / 1000) as u16
            }
            _ => 0,
        }
    }
    fn dot_lift(self, index: u32) -> i32 {
        // Staggered, smooth half-sine hops followed by a short resting beat.
        let t = (self.elapsed_ms % 1200 + 1200 - index * 160) % 1200;
        if t < 600 {
            (6.0 * (t as f32 / 600.0 * std::f32::consts::PI).sin()).round() as i32
        } else {
            0
        }
    }
    fn spot_glow(self, spot: usize) -> u32 {
        let period = if self.phase.pending() { 1500 } else { 3000 };
        let offset =
            (spot % mushroom::HERO_SPOTS.len()) as u32 * period / mushroom::HERO_SPOTS.len() as u32;
        let t = (self.elapsed_ms % period + period - offset) % period;
        let triangle = (t.min(period - t) * 2000 / period).min(1000);
        smooth_progress(triangle, 1000)
    }
}

fn smooth_progress(value: u32, span: u32) -> u32 {
    let t = value.min(span) * 1000 / span;
    (u64::from(t) * u64::from(t) * u64::from(3000 - 2 * t) / 1_000_000) as u32
}

fn hero_mix(a: u32, b: u32, amount: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |color, shift| {
        color
            | ((((a >> shift) & 255) * (1000 - amount) + ((b >> shift) & 255) * amount) / 1000)
                << shift
    })
}

fn hero_motion_enabled(s: &State) -> bool {
    s.pulse_enabled && s.locale.design.effects && s.locale.design.animation
}

// DC_BRUSH is a stock object: no per-frame brush/pen allocation or GDI leaks.
unsafe fn hero_pixel(s: &State, dc: HDC, x: i32, y: i32, size: i32, color: u32) {
    SetDCBrushColor(dc, color);
    FillRect(
        dc,
        &RECT {
            left: s.d(x),
            top: s.d(y),
            right: s.d(x + size),
            bottom: s.d(y + size),
        },
        GetStockObject(DC_BRUSH) as HBRUSH,
    );
}

// Weighted travel along the actual branches, computed once. Different lengths
// and seeded local delays keep forks from switching on as synchronized bands.
const MYCELIUM_ROW: usize = 48;
const ROOT_FADE: u32 = 320;
struct MyceliumGrowth {
    distance: [[u16; mushroom::HERO_WIDTH]; mushroom::HERO_HEIGHT],
    longest: u16,
}
impl MyceliumGrowth {
    fn new() -> Self {
        let mut result = Self {
            distance: [[u16::MAX; mushroom::HERO_WIDTH]; mushroom::HERO_HEIGHT],
            longest: 0,
        };
        let mut pending = std::collections::BinaryHeap::new();
        for col in 0..mushroom::HERO_WIDTH {
            if mushroom::HERO[MYCELIUM_ROW][col] != b'.' {
                let delay = (col.abs_diff(39) * 12) as u16;
                result.distance[MYCELIUM_ROW][col] = delay;
                pending.push(std::cmp::Reverse((delay, MYCELIUM_ROW, col)));
            }
        }
        while let Some(std::cmp::Reverse((distance, row, col))) = pending.pop() {
            if distance != result.distance[row][col] {
                continue;
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let y = row as i32 + dy;
                    let x = col as i32 + dx;
                    if y < MYCELIUM_ROW as i32
                        || y >= mushroom::HERO_HEIGHT as i32
                        || x < 0
                        || x >= mushroom::HERO_WIDTH as i32
                    {
                        continue;
                    }
                    let (y, x) = (y as usize, x as usize);
                    let step = if dx != 0 && dy != 0 { 141 } else { 100 };
                    let next_distance = distance + step + ((y * 37 + x * 19) % 61) as u16;
                    if mushroom::HERO[y][x] != b'.' && next_distance < result.distance[y][x] {
                        result.distance[y][x] = next_distance;
                        pending.push(std::cmp::Reverse((next_distance, y, x)));
                    }
                }
            }
        }
        result.longest = result
            .distance
            .iter()
            .flatten()
            .copied()
            .filter(|&d| d != u16::MAX)
            .max()
            .unwrap_or(0);
        result
    }
    fn coverage(&self, row: usize, col: usize, amount: u16) -> u32 {
        if row < MYCELIUM_ROW {
            return 1000;
        }
        let distance = self.distance[row][col];
        if distance == u16::MAX {
            return 0;
        }
        let front = u32::from(amount) * (u32::from(self.longest) + ROOT_FADE) / 1000;
        smooth_progress(
            front.saturating_sub(u32::from(distance)).min(ROOT_FADE),
            ROOT_FADE,
        )
    }
    fn visible(&self, row: usize, col: usize, amount: u16) -> bool {
        self.coverage(row, col, amount) == 1000
    }
}
fn mycelium_growth() -> &'static MyceliumGrowth {
    static GROWTH: std::sync::OnceLock<MyceliumGrowth> = std::sync::OnceLock::new();
    GROWTH.get_or_init(MyceliumGrowth::new)
}
unsafe fn draw_hero_mushroom(s: &State, dc: HDC, width: i32, height: i32) {
    let animate = hero_motion_enabled(s) && s.hero_motion.animating();
    let amount = s.hero_motion.roots(animate);
    let growth = mycelium_growth();
    let bounds = hero_mushroom_bounds(s, width, height);
    let spot = |glyph| matches!(glyph, b'W' | b'V' | b'Q');
    draw_mushroom_visible(s, dc, &mushroom::HERO, bounds, 0, |row, col| {
        if animate {
            row < MYCELIUM_ROW && !spot(mushroom::HERO[row][col])
        } else {
            growth.visible(row, col, amount)
        }
    });
    if !animate {
        return;
    }
    let colors = mushroom_colors(s.locale.design.theme);
    // Each rounded cap spot breathes out of phase. The cap, stem and silhouette
    // stay fixed, and the stock brush avoids allocating resources per frame.
    let spots: [[u32; 3]; mushroom::HERO_SPOTS.len()] = std::array::from_fn(|group| {
        let glow = s.hero_motion.spot_glow(group);
        [4, 5, 6].map(|index| {
            let dim = hero_mix(colors[index], colors[0], 270);
            let bright = hero_mix(colors[index], colors[7], 350);
            hero_mix(dim, bright, glow)
        })
    });
    let cell = |row: usize, left: usize, right: usize| RECT {
        left: bounds.left
            + left as i32 * (bounds.right - bounds.left) / mushroom::HERO_WIDTH as i32,
        top: bounds.top + row as i32 * (bounds.bottom - bounds.top) / mushroom::HERO_HEIGHT as i32,
        right: bounds.left
            + right as i32 * (bounds.right - bounds.left) / mushroom::HERO_WIDTH as i32,
        bottom: bounds.top
            + (row + 1) as i32 * (bounds.bottom - bounds.top) / mushroom::HERO_HEIGHT as i32,
    };
    let brush = GetStockObject(DC_BRUSH) as HBRUSH;
    for (row, line) in mushroom::HERO.iter().enumerate().take(MYCELIUM_ROW) {
        let mut col = 0;
        while col < mushroom::HERO_WIDTH {
            let glyph = line[col];
            if !spot(glyph) {
                col += 1;
                continue;
            }
            let start = col;
            let group = mushroom::hero_spot(row, col).expect("cap spot animation bounds");
            while col < mushroom::HERO_WIDTH && line[col] == glyph {
                col += 1;
            }
            let shade = mushroom::palette_index(glyph).unwrap() - 4;
            SetDCBrushColor(dc, spots[group][shade]);
            FillRect(dc, &cell(row, start, col), brush);
        }
    }
    let front = u32::from(amount) * (u32::from(growth.longest) + ROOT_FADE) / 1000;
    for (row, line) in mushroom::HERO.iter().enumerate().skip(MYCELIUM_ROW) {
        for (col, &glyph) in line.iter().enumerate() {
            let Some(index) = mushroom::palette_index(glyph) else {
                continue;
            };
            let coverage = growth.coverage(row, col, amount);
            if coverage == 0 {
                continue;
            }
            let rect = cell(row, col, col + 1);
            if rect.right <= rect.left || rect.bottom <= rect.top {
                continue;
            }
            let distance = u32::from(growth.distance[row][col]);
            // A short bright leading edge makes the direction visible. Older
            // branches settle to the theme's exact original root colors.
            let age = front.saturating_sub(distance);
            let tip = if age < 700 && amount < 1000 {
                1000 - age * 1000 / 700
            } else {
                0
            };
            let waiting = if s.hero_motion.phase == HeroPhase::Connecting {
                let t = (s.hero_motion.elapsed_ms % 2100 + 2100 - distance % 2100) % 2100;
                if t < 450 {
                    smooth_progress(450 - t, 450) * 280 / 1000
                } else {
                    0
                }
            } else {
                0
            };
            let mut color = hero_mix(colors[index], colors[7], (tip * 650 / 1000).max(waiting));
            if coverage < 1000 {
                let background = GetPixel(
                    dc,
                    (rect.left + rect.right) / 2,
                    (rect.top + rect.bottom) / 2,
                );
                color = hero_mix(background, color, coverage);
            }
            SetDCBrushColor(dc, color);
            FillRect(dc, &rect, brush);
        }
    }
}

unsafe fn draw_hero_caption(s: &State, dc: HDC, width: i32, y: i32, color: u32) {
    let motion = s.hero_motion;
    let phase = motion.phase;
    let action = s.text(1007);
    let caption = if phase.pending() {
        action.trim_end_matches(['…', '.'])
    } else {
        &action
    };
    let encoded = wide(&caption);
    let old = SelectObject(dc, s.fonts[2]);
    let mut size = SIZE::default();
    GetTextExtentPoint32W(dc, encoded.as_ptr(), encoded.len() as i32 - 1, &mut size);
    SelectObject(dc, old);
    let text_width = (size.cx * 96 + s.dpi - 1) / s.dpi;
    // Reserve a constant slot for dots: neither text nor the action jumps.
    let dots_width = if phase.pending() { 32 } else { 0 };
    let group_width = (text_width + dots_width).min(width - 12);
    let x = (width - group_width) / 2;
    centered(
        s,
        dc,
        &caption,
        x,
        y,
        group_width - dots_width,
        31,
        2,
        color,
    );
    if phase.pending() {
        for index in 0..3 {
            let lift = if hero_motion_enabled(s) {
                motion.dot_lift(index)
            } else {
                0
            };
            hero_pixel(
                s,
                dc,
                x + group_width - 25 + index as i32 * 8,
                y + 19 - lift,
                4,
                hero_mix(
                    s.textures.color(color),
                    s.textures.color(TEAL),
                    350 + lift as u32 * 650 / 6,
                ),
            );
        }
    }
}

unsafe fn smoke_hero_feedback(s: &mut State) -> Result<[bool; 6], String> {
    let original = (
        s.model.clone(),
        s.busy,
        s.pending_action.clone(),
        s.locale.design,
        s.pulse_enabled,
        s.hero_motion,
        s.page,
        s.notice.clone(),
    );
    let mut checks = [true; 6];
    s.page = 0;
    s.notice.clear();
    s.pulse_enabled = true;
    s.locale.design.effects = true;
    s.locale.design.animation = true;
    s.busy = false;
    s.pending_action.clear();
    s.model["last_error"] = json!("");
    s.model["phase"] = json!("connecting");
    s.update_buttons();
    s.layout();
    let mut rect = RECT::default();
    GetClientRect(s.controls[&1007], &mut rect);
    let mut draw = DRAWITEMSTRUCT::default();
    draw.CtlID = 1007;
    draw.CtlType = ODT_BUTTON;
    draw.rcItem = rect;
    draw.hDC = s.buffer_dc;
    paint_draw_item(s, &draw);
    let caption_top = rect.bottom - s.d(40);
    let caption_bottom = caption_top + s.d(31);
    let art_bounds = hero_mushroom_bounds(s, 240, s.home_layout().hero_art_height);
    let cap_bottom =
        art_bounds.top + 30 * (art_bounds.bottom - art_bounds.top) / mushroom::HERO_HEIGHT as i32;
    let sample = |dc: HDC, top: i32, bottom: i32| -> Vec<u32> {
        (top..bottom)
            .flat_map(|y| (0..rect.right).map(move |x| GetPixel(dc, x, y)))
            .collect()
    };
    let text_sample = |dc: HDC| -> Vec<u32> {
        (caption_top..caption_bottom)
            .flat_map(|y| (0..rect.right / 2).map(move |x| GetPixel(dc, x, y)))
            .collect()
    };
    let action_before = sample(s.hero_buffer.dc, caption_top, caption_top + s.d(31));
    let text_before = text_sample(s.hero_buffer.dc);
    let parent = s.parent_paints.get();
    let layouts = s.layout_passes.get();
    let allocations = s.hero_buffer.allocations;
    GdiFlush();
    let resources = GetGuiResources(GetCurrentProcess(), 0);
    for _ in 0..5 {
        SendMessageW(s.hwnd, WM_TIMER, PULSE_TIMER, 0);
    }
    paint_draw_item(s, &draw);
    checks[0] &= action_before != sample(s.hero_buffer.dc, caption_top, caption_top + s.d(31));
    checks[0] &= text_before == text_sample(s.hero_buffer.dc);
    for _ in 0..64 {
        SendMessageW(s.hwnd, WM_TIMER, PULSE_TIMER, 0);
        paint_draw_item(s, &draw);
    }
    GdiFlush();
    checks[1] &= s.parent_paints.get() == parent
        && s.layout_passes.get() == layouts
        && s.hero_buffer.allocations == allocations
        && GetGuiResources(GetCurrentProcess(), 0) == resources;
    // Reduced motion retains the original action, static dots and partial roots.
    for switch in 0..3 {
        match switch {
            0 => s.pulse_enabled = false,
            1 => s.locale.design.animation = false,
            _ => s.locale.design.effects = false,
        }
        s.sync_pulse();
        let ticks = s.pulse_ticks.get();
        SendMessageW(s.hwnd, WM_TIMER, PULSE_TIMER, 0);
        checks[2] &= !s.pulse_running && s.pulse_ticks.get() == ticks;
        paint_draw_item(s, &draw);
        let before = sample(s.hero_buffer.dc, caption_top, caption_top + s.d(31));
        s.hero_motion.advance(250);
        paint_draw_item(s, &draw);
        checks[2] &= before == sample(s.hero_buffer.dc, caption_top, caption_top + s.d(31));
        s.pulse_enabled = true;
        s.locale.design.animation = true;
        s.locale.design.effects = true;
        s.sync_pulse();
    }
    s.model["phase"] = json!("connected");
    s.update_buttons();
    checks[3] &= s.pulse_running;
    paint_draw_item(s, &draw);
    let connected_spots = sample(s.hero_buffer.dc, art_bounds.top, cap_bottom);
    for _ in 0..20 {
        SendMessageW(s.hwnd, WM_TIMER, PULSE_TIMER, 0);
    }
    checks[3] &= s.pulse_running && s.hero_motion.phase == HeroPhase::Connected;
    paint_draw_item(s, &draw);
    checks[3] &= connected_spots != sample(s.hero_buffer.dc, art_bounds.top, cap_bottom);
    s.model["phase"] = json!("disconnecting");
    s.update_buttons();
    checks[3] &= s.pulse_running && IsWindowEnabled(s.controls[&1007]) == 0;
    s.model["phase"] = json!("idle");
    s.update_buttons();
    checks[3] &= s.pulse_running;
    paint_draw_item(s, &draw);
    let idle_spots = sample(s.hero_buffer.dc, art_bounds.top, cap_bottom);
    for _ in 0..15 {
        SendMessageW(s.hwnd, WM_TIMER, PULSE_TIMER, 0);
    }
    paint_draw_item(s, &draw);
    checks[3] &= idle_spots != sample(s.hero_buffer.dc, art_bounds.top, cap_bottom);
    SendMessageW(s.controls[&1007], WM_MOUSEMOVE, 0, 0);
    checks[4] &= s.hero_hover;
    SendMessageW(s.controls[&1007], WM_MOUSELEAVE, 0, 0);
    checks[4] &= !s.hero_hover;
    // Measure the painted roots at source-pixel centers. This catches roots
    // leaking into idle, a missing branch, and growth that jumps or reverses.
    let bounds = hero_mushroom_bounds(s, 240, s.home_layout().hero_art_height);
    paint_draw_item(s, &draw);
    let mut root_background = Vec::new();
    for (row, line) in mushroom::HERO.iter().enumerate().skip(MYCELIUM_ROW) {
        for (col, &glyph) in line.iter().enumerate() {
            if mushroom::palette_index(glyph).is_none() {
                continue;
            }
            let x = bounds.left + (2 * col as i32 + 1) * (bounds.right - bounds.left) / 160;
            let y = bounds.top + (2 * row as i32 + 1) * (bounds.bottom - bounds.top) / 144;
            root_background.push((x, y, GetPixel(s.hero_buffer.dc, x, y)));
        }
    }
    let root_count = |dc: HDC| -> usize {
        root_background
            .iter()
            .filter(|&&(x, y, bg)| GetPixel(dc, x, y) != bg)
            .count()
    };
    let mut counts = Vec::new();
    for (phase, time) in [
        ("idle", 0),
        ("connecting", 0),
        ("connecting", 400),
        ("connecting", 2400),
        ("connected", 1000),
        ("connected", 60000),
        ("disconnecting", 250),
        ("idle", 0),
    ] {
        s.model["phase"] = json!(phase);
        s.update_buttons();
        s.hero_motion.elapsed_ms = time;
        paint_draw_item(s, &draw);
        counts.push(root_count(s.hero_buffer.dc));
    }
    checks[5] &= counts[0] == 0
        && counts[1] == 0
        && counts[2] > 0
        && counts[2] < counts[3]
        && counts[3] < counts[4]
        && counts[4] == 282
        && counts[4] == counts[5]
        && counts[6] > 0
        && counts[6] < counts[5]
        && counts[7] == 0;
    std::fs::write(
        s.root.join("native-root-growth.json"),
        serde_json::to_vec_pretty(&counts).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    for (phase, elapsed, tag) in [
        ("idle", 0, "idle"),
        ("connecting", 150, "starting"),
        ("connecting", 800, "connecting"),
        ("connecting", 2400, "grown"),
        ("connected", 250, "success"),
        ("connected", 950, "connected"),
        ("disconnecting", 450, "disconnecting"),
        ("recovery", 950, "recovery"),
        ("idle", 950, "error"),
    ] {
        s.model["phase"] = json!(phase);
        s.model["last_error"] = json!(if tag == "error" {
            "Connection failed"
        } else {
            ""
        });
        s.update_buttons();
        s.hero_motion.elapsed_ms = elapsed;
        s.pulse_frame = (elapsed / PULSE_INTERVAL % PULSE_FRAMES as u32) as u8;
        s.layout();
        screenshot(s.hwnd, &s.root.join(format!("native-feedback-{tag}.bmp")))?;
    }
    // Optional lossless frames of the actual native control for animation review.
    if std::env::args().any(|arg| arg == "--smoke-hero-frames") {
        s.model["last_error"] = json!("");
        s.hero_motion = HeroMotion::default();
        let mut frame = 0;
        for (phase, duration) in [
            ("idle", 900),
            ("connecting", 3600),
            ("connected", 1500),
            ("disconnecting", 700),
            ("idle", 600),
        ] {
            s.model["phase"] = json!(phase);
            s.update_buttons();
            for time in (0..duration).step_by(PULSE_INTERVAL as usize) {
                s.hero_motion.elapsed_ms = time;
                paint_draw_item(s, &draw);
                screenshot(
                    s.hwnd,
                    &s.root.join(format!("hero-frames/frame-{frame:03}.bmp")),
                )?;
                frame += 1;
            }
        }
    }
    s.model = original.0;
    s.busy = original.1;
    s.pending_action = original.2;
    s.locale.design = original.3;
    s.pulse_enabled = original.4;
    s.hero_motion = original.5;
    s.page = original.6;
    s.notice = original.7;
    s.update_buttons();
    s.layout();
    Ok(checks)
}

#[cfg(test)]
mod hero_tests {
    use super::*;

    #[test]
    fn roots_grow_monotonically_and_complete_only_on_confirmed_connection() {
        let mut motion = HeroMotion::default();
        assert_eq!(motion.roots(true), 0);
        motion.sync(HeroPhase::Connecting);
        assert_eq!(motion.roots(true), 0);
        let mut previous = 0;
        for _ in 0..1200 {
            motion.advance(50);
            assert!(motion.roots(true) >= previous && motion.roots(true) < 1000);
            previous = motion.roots(true);
        }
        assert_eq!(motion.phase, HeroPhase::Connecting);
        assert!(motion.animating());
        motion.sync(HeroPhase::Connected);
        assert!(motion.animating());
        assert_eq!(motion.roots(true), previous);
        motion.advance(400);
        assert!(motion.roots(true) > previous && motion.roots(true) < 1000);
        motion.advance(400);
        assert_eq!(motion.roots(true), 1000);
        motion.advance(60_000);
        assert_eq!(motion.roots(true), 1000);
        assert!(!motion.sync(HeroPhase::Connected));
        assert!(motion.animating());
        motion.sync(HeroPhase::Disconnecting);
        assert!(motion.animating());
        assert_eq!(motion.roots(true), 1000);
        motion.advance(350);
        assert_eq!(motion.roots(true), 500);
        motion.sync(HeroPhase::Idle);
        assert!(motion.animating());
        assert_eq!(motion.roots(true), 0);
    }

    #[test]
    fn cancellation_and_failure_replace_pending_feedback_immediately() {
        let mut motion = HeroMotion::default();
        motion.sync(HeroPhase::Connecting);
        motion.advance(350);
        let partial_roots = motion.roots(true);
        motion.sync(HeroPhase::Disconnecting);
        assert_eq!(motion.elapsed_ms, 0);
        assert_eq!(motion.roots(true), partial_roots);
        motion.advance(225);
        assert!(motion.roots(true) < partial_roots);
        motion.sync(HeroPhase::Recovery);
        assert!(!motion.animating());
        assert_eq!(motion.roots(true), 0);
        assert_eq!(HeroPhase::read("idle", true), HeroPhase::Error);
        assert_eq!(HeroPhase::read("connecting", true), HeroPhase::Connecting);
        assert_eq!(HeroPhase::read("connected", true), HeroPhase::Connected);
    }

    #[test]
    fn every_root_is_reachable_and_grows_after_its_parent() {
        let growth = mycelium_growth();
        for row in MYCELIUM_ROW..mushroom::HERO_HEIGHT {
            for col in 0..mushroom::HERO_WIDTH {
                if mushroom::HERO[row][col] == b'.' {
                    continue;
                }
                let distance = growth.distance[row][col];
                assert_ne!(distance, u16::MAX, "unreachable root at {row},{col}");
                assert!(!growth.visible(row, col, 0));
                assert!(growth.visible(row, col, 1000));
                if row == MYCELIUM_ROW {
                    continue;
                }
                assert!(
                    (row.saturating_sub(1)..=(row + 1).min(mushroom::HERO_HEIGHT - 1)).any(|y| {
                        (col.saturating_sub(1)..=(col + 1).min(mushroom::HERO_WIDTH - 1))
                            .any(|x| growth.distance[y][x] < distance)
                    }),
                    "floating root at {row},{col}"
                );
            }
        }
    }

    #[test]
    fn reduced_motion_preserves_distinct_static_connection_states() {
        let mut motion = HeroMotion::default();
        assert_eq!(motion.roots(false), 0);
        motion.sync(HeroPhase::Connecting);
        let partial = motion.roots(false);
        assert!(partial > 0 && partial < 1000);
        motion.advance(60_000);
        assert_eq!(motion.roots(false), partial);
        motion.sync(HeroPhase::Connected);
        assert_eq!(motion.roots(false), 1000);
        motion.sync(HeroPhase::Disconnecting);
        assert_eq!(motion.roots(false), 0);
    }

    #[test]
    fn dots_hop_in_sequence_and_remain_within_the_reserved_line() {
        let mut motion = HeroMotion::default();
        for time in (0..1200).step_by(50) {
            motion.elapsed_ms = time;
            for index in 0..3 {
                assert!((0..=6).contains(&motion.dot_lift(index)));
            }
        }
        motion.elapsed_ms = 240;
        assert_eq!(motion.dot_lift(0), 6);
        assert!(motion.dot_lift(1) < motion.dot_lift(0));
        assert_eq!(motion.dot_lift(2), 0);
    }

    #[test]
    fn growth_has_intermediate_frames_and_each_spot_keeps_breathing() {
        let growth = mycelium_growth();
        let mut motion = HeroMotion::default();
        for phase in [HeroPhase::Idle, HeroPhase::Connecting, HeroPhase::Connected] {
            motion.sync(phase);
            let mut frames = std::collections::HashSet::new();
            for time in (0..3000).step_by(PULSE_INTERVAL as usize) {
                motion.elapsed_ms = time;
                frames.insert(std::array::from_fn::<_, { mushroom::HERO_SPOTS.len() }, _>(
                    |spot| motion.spot_glow(spot),
                ));
            }
            assert!(
                frames.len() >= 80,
                "{phase:?}: missing intermediate spot frames"
            );
        }
        motion.sync(HeroPhase::Connecting);
        let mut frames = std::collections::HashSet::new();
        let mut partially_grown_frames = 0;
        for time in (0..3600).step_by(PULSE_INTERVAL as usize) {
            motion.elapsed_ms = time;
            let amount = motion.roots(true);
            let frame: Vec<_> = (MYCELIUM_ROW..mushroom::HERO_HEIGHT)
                .flat_map(|row| {
                    (0..mushroom::HERO_WIDTH).filter_map(move |col| {
                        (mushroom::HERO[row][col] != b'.')
                            .then(|| growth.coverage(row, col, amount))
                    })
                })
                .collect();
            partially_grown_frames += usize::from(frame.iter().any(|&c| c > 0 && c < 1000));
            frames.insert(frame);
        }
        assert!(
            frames.len() >= 100,
            "root growth needs a distinct frame at 30 fps"
        );
        assert!(
            partially_grown_frames >= 100,
            "roots must fade in between branch steps"
        );
    }
}
