// Connection feedback stays local to the owner-drawn button. No network state
// is inferred from animation time; only the service can confirm a connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HeroPhase {
    #[default]
    Idle,
    Loading,
    Connecting,
    Connected,
    Disconnecting,
    Recovery,
    Error,
}
impl HeroPhase {
    fn read(phase: &str, error: bool) -> Self {
        match phase {
            "loading" => Self::Loading,
            "connecting" => Self::Connecting,
            "connected" => Self::Connected,
            "disconnecting" => Self::Disconnecting,
            "recovery" => Self::Recovery,
            _ if error => Self::Error,
            _ => Self::Idle,
        }
    }
    fn pending(self) -> bool {
        matches!(self, Self::Loading | Self::Connecting | Self::Disconnecting)
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
                | HeroPhase::Loading
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
            HeroPhase::Loading | HeroPhase::Connecting if animate => {
                // A readable opening sequence, then continued slow growth while
                // the service is working. Time alone can never complete it.
                let elapsed = u64::from(self.elapsed_ms);
                if elapsed <= 3600 {
                    (800 * elapsed / 3600) as u16
                } else {
                    (800 + 140 * (elapsed - 3600) / (elapsed - 3600 + 9000)) as u16
                }
            }
            HeroPhase::Loading | HeroPhase::Connecting => 450,
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
