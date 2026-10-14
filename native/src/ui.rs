#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    appearance::{InterfaceSize, PRIMARY_INK, Theme},
    backend,
    i18n::{Language, Locale},
    mushroom,
    platform::{OwnedHandle, wide},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    ptr::{null, null_mut},
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    Storage::Xps::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{
        Controls::Dialogs::*, Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
        WindowsAndMessaging::*,
    },
};
const REPLY: u32 = WM_APP + 1;
const TRAY: u32 = WM_APP + 2;
const ACTIVATE: u32 = WM_APP + 3;
const RETIRE: u32 = WM_APP + 4;
const LIST_SCROLL: u32 = WM_APP + 5;
const SCROLL_DRAG: u32 = WM_APP + 6;
const STARTUP_CONNECT: u32 = WM_APP + 7;
const BACKGROUND_ACTIVITY: u32 = WM_APP + 8;
const HOME_WIDTH: i32 = 560;
const HOME_HEIGHT: i32 = 740;
const NAV_HEIGHT: i32 = 80;
const NAV_GAP: i32 = 24;
const PULSE_TIMER: usize = 2;
const PULSE_INTERVAL: u32 = 33;
const PULSE_FRAMES: u8 = 90;
const BACKGROUND_TIMER: usize = 3;
const BACKGROUND_RESUME_TIMER: usize = 4;
const BACKGROUND_SPEED_SAVE_TIMER: usize = 5;
// windows-sys omits the zero-offset trackbar query from its bindings.
const TBM_GETPOS: u32 = WM_USER;
const IMPORT_LIMIT: usize = 512 * 1024;
// STATIC owner drawing/notifications; these styles are absent from the
// windows-sys Controls feature but have stable Win32 values.
const PIXEL_SCROLLBAR_STYLE: u32 = 0x0000_000d | 0x0000_0100;
type LayoutKey = (usize, usize, i32, i32, i32, i32, bool, usize, bool, bool);
include!(concat!(env!("OUT_DIR"), "/theme.rs"));
const PAGES: [&str; 4] = ["Подключение", "Правила", "Наборы", "Настройки"];
const KEYS: [&str; 6] = [
    "process_names",
    "process_paths",
    "domains",
    "domain_suffixes",
    "ip_cidrs",
    "rule_set_exclusions",
];
const CATEGORIES: [&str; 6] = [
    "Имена .exe",
    "Пути .exe",
    "Домены",
    "Поддомены",
    "IP / CIDR",
    "Через VPN",
];
unsafe fn make_fonts(dpi: i32) -> Vec<HFONT> {
    [(24, 400), (32, 400), (24, 400), (16, 400), (16, 400)]
        .iter()
        .map(|&(height, weight)| {
            CreateFontW(
                -height * dpi / 96,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                NONANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide(crate::font::family()).as_ptr(),
            )
        })
        .collect()
}
fn mushroom_colors(theme: Theme) -> [u32; mushroom::PALETTE_LEN] {
    mushroom::palette(
        theme.color(BG),
        theme.color(INDIGO),
        theme.color(TEAL),
        theme.color(STEM),
        theme == Theme::Paper,
    )
}
unsafe fn make_mushroom_brushes(theme: Theme) -> Vec<[HBRUSH; mushroom::PALETTE_LEN]> {
    let base = mushroom_colors(theme);
    // Static artwork and icons share this palette. Hero spot shimmer and root
    // interpolation use a stock brush without allocating per-frame palettes.
    [base, base]
        .into_iter()
        .map(|row| row.map(|color| CreateSolidBrush(color)))
        .collect()
}
struct HomeLayout {
    hero_y: i32,
    hero_h: i32,
    hero_art_height: i32,
    detail_y: i32,
    header_y: i32,
    list_y: i32,
    list_h: i32,
    actions_y: i32,
}
#[derive(Default)]
struct PaintBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    size: (i32, i32),
    allocations: u64,
}
impl PaintBuffer {
    unsafe fn ensure(&mut self, source: HDC, size: (i32, i32)) -> bool {
        if self.size == size && !self.dc.is_null() {
            return true;
        }
        if size.0 <= 0 || size.1 <= 0 {
            return false;
        }
        if self.dc.is_null() {
            self.dc = CreateCompatibleDC(source);
        }
        if self.dc.is_null() {
            return false;
        }
        let bitmap = CreateCompatibleBitmap(source, size.0, size.1);
        if bitmap.is_null() {
            return false;
        }
        if self.bitmap.is_null() {
            self.original = SelectObject(self.dc, bitmap);
        } else {
            SelectObject(self.dc, bitmap);
            DeleteObject(self.bitmap);
        }
        self.bitmap = bitmap;
        self.size = size;
        self.allocations += 1;
        true
    }
}
impl Drop for PaintBuffer {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                SelectObject(self.dc, self.original);
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                DeleteDC(self.dc);
            }
        }
    }
}

/// WS_EX_COMPOSITED publishes when the last window DC is released. Keep one
/// alive across parent AND child paints; RedrawWindow alone only sends those
/// messages synchronously and each EndPaint can otherwise present a partial
/// frame. Never retain this DC between messages (Windows has a 100 ms timeout).
struct CompositedFrame {
    hwnd: HWND,
    dc: HDC,
}
impl CompositedFrame {
    unsafe fn begin(hwnd: HWND) -> Self {
        Self {
            hwnd,
            dc: GetDC(hwnd),
        }
    }
}
impl Drop for CompositedFrame {
    fn drop(&mut self) {
        if !self.dc.is_null() {
            unsafe {
                ReleaseDC(self.hwnd, self.dc);
            }
        }
    }
}
struct StartupProbe {
    started: Instant,
    window_ready_ms: f64,
}
struct State {
    base: PathBuf,
    root: PathBuf,
    hwnd: HWND,
    controls: HashMap<i32, HWND>,
    captions: HashMap<i32, String>,
    locale: Locale,
    autostart_executable: Option<PathBuf>,
    autostart_enabled: bool,
    startup_pending: bool,
    startup_since: Instant,
    page: usize,
    category: usize,
    selected_preset: Option<String>,
    preset_manager_open: bool,
    dns_settings_open: bool,
    appearance_open: bool,
    model: Value,
    draft: Value,
    loaded: bool,
    busy: bool,
    rename_open: bool,
    import_open: bool,
    import_error: Option<String>,
    imported_count: Option<u64>,
    pending_action: String,
    generation: u64,
    status_pending: bool,
    status_revision: Option<String>,
    startup_probe: Option<StartupProbe>,
    status_attempts: Cell<u64>,
    pending_id: Option<String>,
    pending_since: Option<Instant>,
    request_nonce: String,
    checks: HashMap<i32, bool>,
    notice: String,
    next: Option<Value>,
    dpi: i32,
    monitor_dpi: i32,
    fonts: Vec<HFONT>,
    mushroom_brushes: Vec<[HBRUSH; mushroom::PALETTE_LEN]>,
    product_icons: ProductIcons,
    pulse_frame: u8,
    pulse_running: bool,
    pulse_enabled: bool,
    pulse_ticks: Cell<u64>,
    hero_motion: HeroMotion,
    hero_hover: bool,
    background_running: bool,
    background_enabled: bool,
    app_active: bool,
    resizing: bool,
    background_input_pause: bool,
    background_millis: u32,
    background_fraction: u32,
    background_speed_preview: Option<u32>,
    background_since: Instant,
    hero_draws: Cell<u64>,
    nav_hover: Option<i32>,
    smoke_min_height: Option<i32>,
    textures: crate::texture::Textures,
    regions: crate::design::Regions,
    paper: HBRUSH,
    white: HBRUSH,
    smoke: bool,
    hidden: bool,
    minimized: bool,
    scroll: i32,
    scroll_dragging: bool,
    scroll_drag_offset: i32,
    profile_wheel: i32,
    scrollbar_drag: Option<(i32, i32)>,
    exit_pending: bool,
    layout_key: Cell<Option<LayoutKey>>,
    control_bounds: RefCell<HashMap<i32, (i32, i32, i32, i32)>>,
    layout_passes: Cell<u64>,
    geometry_moves: Cell<u64>,
    parent_paints: Cell<u64>,
    buffer_dc: HDC,
    buffer_bitmap: HBITMAP,
    buffer_old: HGDIOBJ,
    buffer_size: (i32, i32),
    hero_buffer: PaintBuffer,
    speed_buffer: PaintBuffer,
    control_buffers: HashMap<i32, PaintBuffer>,
    hero_presentations: Cell<u64>,
    edit_ink_cache: RefCell<HashMap<usize, (TEXTMETRICW, i32, i32)>>,
}
struct Answer {
    response: Result<Value, String>,
    action: String,
    generation: u64,
    request_id: Option<String>,
}
include!("preset_ui.rs");
include!("appearance_ui.rs");
include!("hero.rs");
impl State {
    unsafe fn minimum_height(&self) -> i32 {
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(self.hwnd, &mut rect);
        self.minimum_height_for_rect(&rect)
    }
    unsafe fn work_area_for_rect(&self, rect: &RECT) -> RECT {
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(
            MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0
        {
            return monitor.rcWork;
        }
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            &mut work as *mut _ as *mut std::ffi::c_void,
            0,
        );
        work
    }
    unsafe fn minimum_height_for_rect(&self, rect: &RECT) -> i32 {
        if self.smoke {
            return self.smoke_min_height.unwrap_or(self.d(HOME_HEIGHT));
        }
        let work = self.work_area_for_rect(rect);
        self.d(HOME_HEIGHT)
            .min((work.bottom - work.top - self.d(16)).max(240))
    }
    unsafe fn home_layout(&self) -> HomeLayout {
        let visible = self.dimensions().1.max(460);
        let compact = visible < 680;
        let tiny = visible < 520;
        let error = self.connection_error().is_some();
        let actions_y = visible - NAV_HEIGHT - NAV_GAP - 42;
        let header_y = if tiny {
            192
        } else if compact {
            252
        } else {
            324
        } + if error { 44 } else { 0 };
        let list_y = header_y + 28;
        HomeLayout {
            hero_y: if compact { 88 } else { 96 },
            hero_h: if tiny {
                100
            } else if compact {
                160
            } else {
                196
            },
            hero_art_height: if tiny {
                48
            } else if compact {
                108
            } else {
                144
            },
            detail_y: if tiny {
                184
            } else if compact {
                248
            } else {
                308
            },
            header_y,
            list_y,
            list_h: (actions_y - 20 - list_y).max(44),
            actions_y,
        }
    }
    unsafe fn sync_pulse(&mut self) {
        self.sync_background();
        let phase = HeroPhase::read(self.phase(), self.connection_error().is_some());
        if self.hero_motion.sync(phase) {
            self.pulse_frame = 0;
            if let Some(&hero) = self.controls.get(&1007) {
                InvalidateRect(hero, null(), 0);
            }
        }
        let active = self.pulse_enabled
            && self.app_active
            && !self.resizing
            && self.locale.design.effects
            && self.locale.design.animation
            && self.hero_motion.animating()
            && self.page == 0
            && !self.import_open
            && !self.hidden
            && !self.minimized
            && IsIconic(self.hwnd) == 0
            && IsWindowVisible(self.hwnd) != 0
            && self
                .controls
                .get(&1007)
                .is_some_and(|control| IsWindowVisible(*control) != 0);
        if active == self.pulse_running {
            return;
        }
        self.pulse_running = active;
        if active {
            SetTimer(self.hwnd, PULSE_TIMER, PULSE_INTERVAL, None);
        } else {
            KillTimer(self.hwnd, PULSE_TIMER);
            self.pulse_frame = 0;
        }
    }
    unsafe fn sync_background(&mut self) {
        let focus = GetFocus();
        let mut class = [0u16; 32];
        let editing =
            if !focus.is_null() && GetParent(focus) == self.hwnd && IsWindowVisible(focus) != 0 {
                let n = GetClassNameW(focus, class.as_mut_ptr(), class.len() as i32);
                let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
                class.eq_ignore_ascii_case("Edit") || class.eq_ignore_ascii_case("ComboBox")
            } else {
                false
            };
        let active = self.background_enabled
            && self.app_active
            && !self.resizing
            && !editing
            && !self.background_input_pause
            && !self.scroll_dragging
            && self.scrollbar_drag.is_none()
            && self.locale.design.effects
            && self.locale.design.animation
            && !self.hidden
            && !self.minimized
            && IsIconic(self.hwnd) == 0
            && IsWindowVisible(self.hwnd) != 0;
        if active == self.background_running {
            return;
        }
        self.background_running = active;
        if active {
            self.background_since = Instant::now();
            SetTimer(self.hwnd, BACKGROUND_TIMER, 50, None);
        } else {
            KillTimer(self.hwnd, BACKGROUND_TIMER);
        }
    }
    unsafe fn pause_background_input(&mut self) {
        self.background_input_pause = true;
        self.sync_background();
        SetTimer(self.hwnd, BACKGROUND_RESUME_TIMER, 180, None);
    }
    fn background_speed(&self) -> u32 {
        self.background_speed_preview
            .unwrap_or(self.locale.design.animation_speed)
    }
    fn advance_background(&mut self) {
        let elapsed = self.background_since.elapsed().as_millis().min(1000) as u32;
        self.background_since = Instant::now();
        let scaled = elapsed * self.background_speed() + self.background_fraction;
        self.background_fraction = scaled % 100;
        self.background_millis =
            (self.background_millis + scaled / 100) % crate::texture::FLOW_PERIOD_MS;
    }
    unsafe fn save_background_speed(&mut self) {
        KillTimer(self.hwnd, BACKGROUND_SPEED_SAVE_TIMER);
        let Some(speed) = self.background_speed_preview else {
            return;
        };
        let mut design = self.locale.design;
        design.animation_speed = speed;
        if let Err(error) = self.locale.set_design(design) {
            self.notice = error;
            InvalidateRect(self.hwnd, null(), 0);
        }
        self.background_speed_preview = None;
        SendMessageW(
            self.controls[&4024],
            TBM_SETPOS,
            1,
            self.background_speed() as isize,
        );
        InvalidateRect(self.controls[&4025], null(), 0);
    }
    unsafe fn invalidate_background(&self) {
        RedrawWindow(
            self.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_NOCHILDREN | RDW_NOERASE,
        );
        // Editors, lists and opaque buttons do not change with the background.
        // Keep their text, caret and selection out of the animation paint loop.
        for id in [
            200, 201, 202, 203, 204, 205, 1007, 1011, 1010, 1024, 2110, 3010, 3307,
        ] {
            if let Some(&hwnd) = self.controls.get(&id)
                && IsWindowVisible(hwnd) != 0
            {
                InvalidateRect(hwnd, null(), 0);
            }
        }
    }
    unsafe fn prepare_background(&mut self) {
        let mut rect = RECT::default();
        GetClientRect(self.hwnd, &mut rect);
        self.textures
            .prepare_background(rect.right, rect.bottom, self.background_millis);
    }
    unsafe fn refresh_design(&mut self) {
        let design = self.locale.design;
        self.textures.set_design(design);
        if !design.effects {
            self.regions.clear();
        }
        for (id, value) in [
            (4020, design.effects),
            (4021, design.animation),
            (4023, design.rounding),
        ] {
            if self.controls.contains_key(&id) {
                self.check(id, value);
                self.enable(id, id == 4020 || design.effects);
            }
        }
        if let Some(&slider) = self.controls.get(&4024) {
            self.enable(4024, design.effects && design.animation);
            self.enable(4025, design.effects && design.animation);
            SendMessageW(slider, TBM_SETPOS, 1, self.background_speed() as isize);
        }
        let rounded = design.effects && design.rounding;
        self.control_bounds.borrow_mut().clear();
        self.layout_key.set(None);
        self.layout();
        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rect);
        crate::design::window_shape(self.hwnd, rect.right, rect.bottom, self.dpi, rounded);
        // Composited children may paint before the parent. Publish the frame
        // before invalidating either, including changes made while paused.
        self.prepare_background();
        self.sync_background();
        self.sync_pulse();
        RedrawWindow(
            self.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_NOERASE | RDW_FRAME,
        );
    }
    fn d(&self, n: i32) -> i32 {
        n * self.dpi / 96
    }
    unsafe fn set_dpi_fonts(&mut self, dpi: i32) {
        self.set_dpi_fonts_at(dpi, None);
    }
    unsafe fn set_dpi_fonts_at(&mut self, dpi: i32, suggested: Option<&RECT>) {
        self.monitor_dpi = dpi;
        if self.product_icons.dpi != dpi {
            if let Ok(icons) = ProductIcons::new(self.locale.design.theme, dpi) {
                icons.apply(self.hwnd);
                let old = std::mem::replace(&mut self.product_icons, icons);
                if !self.smoke {
                    tray_update(self);
                }
                drop(old);
            }
        }
        let mut current = RECT::default();
        GetWindowRect(self.hwnd, &mut current);
        let work = self.work_area_for_rect(suggested.unwrap_or(&current));
        let requested = self.locale.design.interface_size.dpi(dpi);
        self.dpi = if self.smoke {
            requested
        } else {
            crate::appearance::fit_dpi(requested, work.right - work.left)
        };
        let dpi = self.dpi;
        if let Some(&slider) = self.controls.get(&4024) {
            SendMessageW(slider, TBM_SETTHUMBLENGTH, self.d(24) as usize, 0);
        }
        if let Some(&list) = self.controls.get(&3301) {
            SendMessageW(list, LB_SETITEMHEIGHT, 0, self.d(44) as isize);
        }
        self.textures.set_dpi(dpi);
        self.paper = self.textures.brush(BG);
        self.white = self.textures.brush(PANEL_ALT);
        self.edit_ink_cache.borrow_mut().clear();
        let old = std::mem::replace(&mut self.fonts, make_fonts(dpi));
        for (&id, &control) in &self.controls {
            SendMessageW(
                control,
                WM_SETFONT,
                self.fonts[if [
                    1006, 1020, 2102, 2105, 3310, 3103, 3104, 3006, 3007, 3008, 3009, 3013, 3014,
                ]
                .contains(&id)
                {
                    4
                } else {
                    0
                }] as usize,
                1,
            );
        }
        for id in [3101] {
            if let Some(&control) = self.controls.get(&id) {
                SendMessageW(control, CB_SETITEMHEIGHT, usize::MAX, self.d(38) as isize);
                SendMessageW(control, CB_SETITEMHEIGHT, 0, self.d(34) as isize);
                let mut r = RECT::default();
                GetWindowRect(control, &mut r);
                crate::design::window_shape(
                    control,
                    r.right - r.left,
                    r.bottom - r.top,
                    self.dpi,
                    self.locale.design.effects && self.locale.design.rounding,
                );
            }
        }
        if let Some(&list) = self.controls.get(&1001) {
            SendMessageW(list, LB_SETITEMHEIGHT, 0, self.d(44) as isize);
        }
        for font in old {
            DeleteObject(font);
        }
    }
    unsafe fn ctrl(&mut self, id: i32, class: &str, text: &str, style: u32) {
        // Keep padding inside the client surface so composition paints the
        // field atomically. ES_WANTRETURN distinguishes true multiline lists.
        let style = if class == "EDIT" && style & ES_MULTILINE as u32 == 0 {
            style | ES_MULTILINE as u32 | ES_AUTOHSCROLL as u32
        } else {
            style
        };
        let caption = if matches!(class, "EDIT" | "LISTBOX") {
            text.to_owned()
        } else {
            self.captions.insert(id, text.to_owned());
            self.locale.text(text).into_owned()
        };
        let h = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(&caption).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            10,
            10,
            self.hwnd,
            id as usize as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SetWindowTheme(h, wide("DarkMode_Explorer").as_ptr(), wide("").as_ptr());
        SendMessageW(
            h,
            WM_SETFONT,
            self.fonts[if class == "EDIT" { 4 } else { 0 }] as usize,
            1,
        );
        self.controls.insert(id, h);
        if (class == "BUTTON" && style & 0xf == BS_OWNERDRAW as u32)
            || (class == "STATIC" && style & 0x1f == 0x0000_000d)
        {
            SetWindowSubclass(h, Some(buffered_control_procedure), 8, 0);
        }
        if class == "EDIT" {
            SetWindowSubclass(h, Some(edit_procedure), 5, 0);
            format_edit(h, self.dpi, true);
        }
        if class == "LISTBOX" {
            SetWindowSubclass(h, Some(list_frame_procedure), 9, 0);
        }
        if class == "COMBOBOX" {
            SendMessageW(h, CB_SETITEMHEIGHT, usize::MAX, self.d(38) as isize);
            SendMessageW(h, CB_SETITEMHEIGHT, 0, self.d(34) as isize);
            SetWindowSubclass(h, Some(combo_procedure), 6, 0);
            let mut info = COMBOBOXINFO::default();
            info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
            if GetComboBoxInfo(h, &mut info) != 0 {
                SetWindowSubclass(info.hwndList, Some(combo_list_procedure), 10, h as usize);
            }
        }
    }
    fn text(&self, id: i32) -> String {
        unsafe {
            let h = self.controls[&id];
            let n = GetWindowTextLengthW(h);
            let mut s = vec![0u16; n as usize + 1];
            GetWindowTextW(h, s.as_mut_ptr(), s.len() as i32);
            String::from_utf16_lossy(&s[..n as usize])
        }
    }
    unsafe fn set(&self, id: i32, text: &str) {
        let translated;
        let text = if [1007, 1021].contains(&id) {
            translated = self.locale.text(text);
            translated.as_ref()
        } else {
            text
        };
        if self.text(id) != text {
            SetWindowTextW(self.controls[&id], wide(text).as_ptr());
        }
    }
    unsafe fn change_language(&mut self, language: Language) -> bool {
        if let Err(error) = self.locale.set_language(language) {
            self.notice = error;
            if !self.smoke {
                MessageBoxW(
                    self.hwnd,
                    wide(&self.locale.error(&self.notice)).as_ptr(),
                    wide(&self.locale.text("Настройки")).as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            InvalidateRect(self.hwnd, null(), 0);
            return false;
        }
        // Captions have separate source strings; neither raw editor drafts nor
        // server names/selection/scroll are rebuilt on a language switch.
        for (&id, source) in &self.captions {
            let value = self.locale.text(source);
            if self.text(id) != value.as_ref() {
                SetWindowTextW(self.controls[&id], wide(&value).as_ptr());
            }
        }
        self.update_buttons();
        self.populate_preset_choices();
        self.rule_entry_hint();
        for &control in self.controls.values() {
            InvalidateRect(control, null(), 0);
        }
        if !self.smoke {
            tray_update(self);
        }
        InvalidateRect(self.hwnd, null(), 0);
        true
    }
    unsafe fn checked(&self, id: i32) -> bool {
        self.checks.get(&id).copied().unwrap_or(false)
    }
    unsafe fn check(&mut self, id: i32, yes: bool) {
        if self.checks.insert(id, yes) != Some(yes) {
            InvalidateRect(self.controls[&id], null(), 0);
        }
    }
    fn phase(&self) -> &str {
        if self.busy && self.pending_action == "Connect" {
            return "connecting";
        }
        if self.busy && matches!(self.pending_action.as_str(), "Disconnect" | "Exit") {
            return "disconnecting";
        }
        self.model["phase"].as_str().unwrap_or_else(|| {
            if self.model["dns_recovery_pending"] == true {
                "recovery"
            } else if self.model["running"] == true {
                "connected"
            } else if self.model["starting"] == true {
                "connecting"
            } else {
                "idle"
            }
        })
    }
    fn can_disconnect(&self) -> bool {
        self.model["can_disconnect"] == true
            || matches!(self.phase(), "connecting" | "connected" | "recovery")
    }
    unsafe fn request(&mut self, mut request: Value) {
        let action_name = request["action"].as_str().unwrap_or("").to_owned();
        let action = action_name.as_str();
        if matches!(action, "Connect" | "Disconnect" | "Exit" | "Select")
            && (!self.busy || matches!(action, "Disconnect" | "Exit"))
        {
            self.startup_pending = false;
        }
        if matches!(action, "ApplySettings" | "ValidateSettings")
            && !crate::presets::fits_transport(&request["settings"])
        {
            self.notice = crate::presets::SIZE_ERROR.into();
            MessageBoxW(
                self.hwnd,
                wide(&self.locale.error(&self.notice)).as_ptr(),
                wide(&self.locale.text("Проверка правил")).as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
            InvalidateRect(self.hwnd, null(), 0);
            return;
        }
        if self.smoke && action != "Exit" {
            if action == "Select" {
                self.model["selected"] = request["id"].clone();
            }
            if action == "Disconnect" {
                self.model["phase"] = json!("idle");
                self.model["running"] = json!(false);
                self.model["can_disconnect"] = json!(false);
                self.busy = false;
                self.pending_action.clear();
            }
            self.populate();
            return;
        }
        if self.busy && !matches!(action, "Disconnect" | "Exit") {
            return;
        }
        self.generation += 1;
        self.status_revision = None;
        let request_id = format!("{}-{}", self.request_nonce, self.generation);
        request["request_id"] = json!(request_id);
        self.pending_id = Some(request_id);
        self.pending_since = Some(Instant::now());
        self.busy = true;
        self.pending_action = action.to_owned();
        if matches!(action, "Disconnect" | "Exit") {
            self.next = None;
        }
        self.exit_pending = action == "Exit";
        self.imported_count = None;
        self.import_error = None;
        self.notice = match action {
            "Connect" => "Устанавливаем соединение…",
            "Select" => "Выбираем сервер…",
            "Import" => "Добавляем конфигурацию…",
            "Disconnect" => "Отключаем VPN…",
            "Exit" => "Отключаем VPN и завершаем Mukhomor…",
            "Diagnose" => "Проверяем соединение…",
            _ => "Сохраняем изменения…",
        }
        .into();
        if action == "Connect" {
            self.model["phase"] = json!("connecting");
        }
        if matches!(action, "Disconnect" | "Exit") {
            self.model["phase"] = json!("disconnecting");
        }
        if !self.smoke {
            // Pending shutdown still needs terminal cache reconciliation when
            // the window is in the tray and the RPC response is lost.
            SetTimer(self.hwnd, 1, 3000, None);
        }
        self.update_buttons();
        InvalidateRect(self.hwnd, null(), 0);
        if !self.smoke {
            launch(self.hwnd, self.root.clone(), request, self.generation);
        }
    }
    unsafe fn status(&mut self) {
        self.status_attempts.set(self.status_attempts.get() + 1);
        if !self.smoke && !self.status_pending {
            self.status_pending = true;
            let mut request = json!({"action":"Status"});
            if self.loaded
                && let Some(revision) = &self.status_revision
            {
                request["if_revision"] = json!(revision);
            }
            launch(self.hwnd, self.root.clone(), request, self.generation);
        }
    }
    unsafe fn profile_id(&self) -> Option<String> {
        let n = SendMessageW(self.controls[&1001], LB_GETCURSEL, 0, 0);
        if n < 0 {
            return None;
        }
        self.model["profiles"].as_array()?.get(n as usize)?["id"]
            .as_str()
            .map(str::to_owned)
    }
    fn lines(&self, id: i32) -> Value {
        json!(
            self.text(id)
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
                .collect::<Vec<_>>()
        )
    }
    fn dns_visible(&self) -> bool {
        self.page == 3 && self.dns_settings_open
    }
    unsafe fn collect(&mut self) -> bool {
        if self.dns_visible()
            && [3007, 3008, 3009, 3013, 3014]
                .iter()
                .any(|&id| self.text(id).parse::<u32>().is_err())
        {
            self.notice = "Интервалы и лимиты DNS должны быть целыми положительными числами".into();
            MessageBoxW(
                self.hwnd,
                wide(&self.locale.error(&self.notice)).as_ptr(),
                wide(&self.locale.text("Проверь настройки DNS")).as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
            return false;
        }
        self.collect_presets();
        if self.dns_visible() {
            self.draft["dns_update"]["enabled"] = json!(self.checked(3004));
            self.draft["dns_update"]["observe_subdomains"] = json!(self.checked(3005));
            self.draft["dns_update"]["seed_hosts"] = self.lines(3006);
            for (id, key) in [
                (3007, "min_refresh_seconds"),
                (3008, "max_refresh_seconds"),
                (3009, "max_stale_seconds"),
                (3013, "max_hosts"),
                (3014, "max_addresses_per_host"),
            ] {
                if let Ok(n) = self.text(id).parse::<u32>() {
                    self.draft["dns_update"][key] = json!(n);
                }
            }
        }
        if self.page == 3 && !self.dns_settings_open && !self.appearance_open {
            self.draft["routing"] = json!({
                "mode": if self.checked(4006) { "full" } else { "split" },
                "allow_lan": self.checked(4007)
            });
        }
        true
    }
    unsafe fn populate_profiles(&mut self) {
        let rename_draft = if self.rename_open {
            Some(self.text(1006))
        } else {
            None
        };
        let old_top = SendMessageW(self.controls[&1001], LB_GETTOPINDEX, 0, 0).max(0);
        let visible = GetWindowLongPtrW(self.controls[&1001], GWL_STYLE) as u32 & WS_VISIBLE != 0;
        if visible {
            SendMessageW(self.controls[&1001], WM_SETREDRAW, 0, 0);
        }
        SendMessageW(self.controls[&1001], LB_RESETCONTENT, 0, 0);
        if let Some(items) = self.model["profiles"].as_array() {
            for (n, item) in items.iter().enumerate() {
                SendMessageW(
                    self.controls[&1001],
                    LB_ADDSTRING,
                    0,
                    wide(item["name"].as_str().unwrap_or("Сервер")).as_ptr() as isize,
                );
                if item["id"] == self.model["selected"] {
                    SendMessageW(self.controls[&1001], LB_SETCURSEL, n, 0);
                    self.set(1006, item["name"].as_str().unwrap_or(""));
                }
            }
            if !items.is_empty() && SendMessageW(self.controls[&1001], LB_GETCURSEL, 0, 0) < 0 {
                SendMessageW(self.controls[&1001], LB_SETCURSEL, 0, 0);
                self.set(1006, items[0]["name"].as_str().unwrap_or("Сервер"));
            }
        }
        if let Some(value) = rename_draft {
            self.set(1006, &value);
        }
        SendMessageW(self.controls[&1001], LB_SETTOPINDEX, old_top as usize, 0);
        // WM_SETREDRAW(TRUE) also sets WS_VISIBLE. Never send it to a list
        // hidden on another page, or a status refresh can expose that page.
        if visible {
            SendMessageW(self.controls[&1001], WM_SETREDRAW, 1, 0);
        }
        InvalidateRect(self.controls[&1001], null(), 0);
        if let Some(&bar) = self.controls.get(&1010) {
            InvalidateRect(bar, null(), 0);
        }
    }
    unsafe fn rule_entry_hint(&self) {
        SendMessageW(
            self.controls[&2105],
            EM_SETCUEBANNER,
            0,
            wide(&self.locale.text("Новое правило")).as_ptr() as isize,
        );
    }
    unsafe fn populate_settings(&mut self) {
        self.rule_entry_hint();
        self.check(4006, self.draft["routing"]["mode"] == "full");
        self.check(4007, self.draft["routing"]["allow_lan"] != false);
        self.populate_presets();
        self.check(3004, self.draft["dns_update"]["enabled"] == true);
        self.check(3005, self.draft["dns_update"]["observe_subdomains"] == true);
        self.set(3006, &joined(&self.draft["dns_update"]["seed_hosts"]));
        for (id, key) in [
            (3007, "min_refresh_seconds"),
            (3008, "max_refresh_seconds"),
            (3009, "max_stale_seconds"),
            (3013, "max_hosts"),
            (3014, "max_addresses_per_host"),
        ] {
            self.set(id, &self.draft["dns_update"][key].to_string());
        }
    }
    // Background status and unrelated actions never rewrite editable settings.
    unsafe fn sync_snapshot(&mut self, mut data: Value, action: &str) {
        if data.get("rule_issues").is_none() && data["settings"] == self.model["settings"] {
            data["rule_issues"] = self.model["rule_issues"].clone();
            data["rule_issues_total"] = self.model["rule_issues_total"].clone();
        }
        let initial = !self.loaded;
        let profiles_changed = self.model["profiles"] != data["profiles"];
        let selection_changed = self.model["selected"] != data["selected"];
        self.model = data;
        if initial || action == "ApplySettings" {
            self.draft = self.model["settings"].clone();
            crate::presets::normalize(&mut self.draft);
            self.loaded = true;
            self.populate_settings();
        }
        if initial || profiles_changed || selection_changed {
            if selection_changed {
                self.rename_open = false;
            }
            self.populate_profiles();
        }
        if !(action == "Status" && self.busy && self.pending_action == "Autoconnect") {
            self.check(4001, self.model["autoconnect"] == true);
        }
    }
    unsafe fn populate(&mut self) {
        self.populate_profiles();
        self.populate_settings();
        self.check(4001, self.model["autoconnect"] == true);
        self.check(4002, self.autostart_enabled);
        self.update_buttons();
        self.populate_preset_choices();
        self.rule_entry_hint();
        self.layout();
        InvalidateRect(self.hwnd, null(), 0);
    }
    unsafe fn update_buttons(&mut self) {
        self.enable(4002, self.smoke || self.autostart_executable.is_some());
        let label = match self.phase() {
            "connecting" => "Отменить",
            "connected" => "Отключиться",
            "disconnecting" => "Отключаем…",
            "recovery" => "Вернуть сеть",
            _ => "Подключиться",
        };
        self.set(1007, label);
        for id in [
            1002, 1003, 1004, 1005, 2103, 2104, 3011, 3012, 4001, 4003, 4006, 4007, 4008,
        ] {
            let disabled = self.busy
                || self.model["busy"] == true
                || self.model["service_ready"] != true
                || ([4006, 4007].contains(&id)
                    && (self.model["running"] == true
                        || self.model["starting"] == true
                        || self.model["traffic_guard"]["configured"] == true))
                || ([1003, 1004, 1005].contains(&id) && self.profile_id().is_none());
            self.enable(id, !disabled);
        }
        let primary = self.model["service_ready"] == true
            && self.phase() != "disconnecting"
            && (self.can_disconnect() || (!self.busy && self.profile_id().is_some()));
        self.enable(1007, primary);
        self.enable(1001, !self.busy && self.model["busy"] != true);
        self.enable(2102, !(self.busy && self.pending_action == "ApplySettings"));
        if self.controls.contains_key(&1021) {
            let available = !self.busy && self.model["busy"] != true;
            let content = self.text(1020);
            self.enable(1020, available);
            self.enable(
                1021,
                available
                    && !content.trim().is_empty()
                    && content.len() <= IMPORT_LIMIT
                    && self.model["service_ready"] == true,
            );
            self.enable(1022, available);
            self.set(
                1021,
                if self.busy && self.pending_action == "Import" {
                    "Добавляем…"
                } else {
                    "Импортировать"
                },
            );
        }
        self.sync_pulse();
        self.preset_buttons();
    }
    unsafe fn enable(&self, id: i32, yes: bool) {
        if (IsWindowEnabled(self.controls[&id]) != 0) != yes {
            EnableWindow(self.controls[&id], i32::from(yes));
        }
    }
    fn visible_state(&self) -> Value {
        json!({"phase":self.phase(),"busy":self.busy,"backend_busy":self.model["busy"],"ready":self.model["service_ready"],"profiles":self.model["profiles"],"selected":self.model["selected"],"recovery":self.model["dns_recovery_pending"],"error":self.model["last_error"],"can_disconnect":self.can_disconnect(),"notice":self.notice,"import_open":self.import_open,"import_error":self.import_error,"imported_count":self.imported_count})
    }
    fn connection_error(&self) -> Option<&str> {
        if let Some(error) = self.import_error.as_deref() {
            return Some(error);
        }
        let error = self.model["last_error"]
            .as_str()
            .filter(|value| !value.is_empty());
        if self.notice.is_empty() {
            return error;
        }
        if error == Some(self.notice.as_str())
            || (self.model["last_operation"]["ok"] == false
                && self.model["last_operation"]["error"].as_str() == Some(self.notice.as_str()))
        {
            Some(&self.notice)
        } else {
            None
        }
    }
    unsafe fn close_import(&mut self) {
        self.import_open = false;
        self.import_error = None;
        self.set(1020, "");
    }
    unsafe fn submit_import(&mut self, content: String, name: &str, source: &str) {
        if content.trim().is_empty() {
            return;
        }
        if content.len() > IMPORT_LIMIT {
            self.import_error = Some("Конфигурация слишком большая (максимум 512 КБ)".into());
        } else {
            self.import_error = None;
            self.imported_count = None;
            self.request(import_request(content, name, source));
        }
        self.layout();
        self.update_buttons();
        InvalidateRect(self.hwnd, null(), 0);
    }
    fn settle_operation(&mut self, data: &Value) -> Option<bool> {
        let current = self.pending_id.as_deref()?;
        let last = &data["last_operation"];
        if last["request_id"].as_str() != Some(current) {
            return None;
        }
        let ok = last["ok"] == true;
        self.busy = false;
        self.pending_id = None;
        self.pending_since = None;
        if !ok {
            self.next = None;
            self.notice = last["error"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(if last["cancelled"] == true {
                    "Операция отменена"
                } else {
                    "Не удалось завершить операцию"
                })
                .to_owned();
        }
        Some(ok)
    }
    unsafe fn finish_exit(&mut self, ok: bool) {
        self.exit_pending = false;
        if ok {
            DestroyWindow(self.hwnd);
        } else {
            // Cleanup failure must remain visible and retryable. The controller
            // continues running so the network can be restored safely.
            self.hidden = false;
            ShowWindow(self.hwnd, SW_RESTORE);
            if !self.smoke {
                SetForegroundWindow(self.hwnd);
                SetTimer(self.hwnd, 1, 3000, None);
            }
        }
    }
    unsafe fn scroll_info(&self, id: i32) -> (i32, i32, i32, i32) {
        let handle = self.controls[&id];
        let mut rect = std::mem::zeroed();
        GetClientRect(handle, &mut rect);
        let (top, count, item) = if matches!(id, 1001 | 3301) {
            (
                SendMessageW(handle, LB_GETTOPINDEX, 0, 0),
                SendMessageW(handle, LB_GETCOUNT, 0, 0),
                SendMessageW(handle, LB_GETITEMHEIGHT, 0, 0),
            )
        } else {
            let dc = GetDC(handle);
            let old = SelectObject(dc, self.fonts[4]);
            let mut metrics = std::mem::zeroed();
            GetTextMetricsW(dc, &mut metrics);
            SelectObject(dc, old);
            ReleaseDC(handle, dc);
            (
                SendMessageW(handle, EM_GETFIRSTVISIBLELINE, 0, 0),
                SendMessageW(handle, EM_GETLINECOUNT, 0, 0),
                metrics.tmHeight as isize,
            )
        };
        (
            top.max(0) as i32,
            count.max(1) as i32,
            (rect.bottom / item.max(1) as i32).max(1),
            rect.bottom,
        )
    }
    unsafe fn scroll_to(&self, id: i32, next: i32) {
        let (top, count, rows, _) = self.scroll_info(id);
        let next = next.clamp(0, (count - rows).max(0));
        if top == next {
            return;
        }
        let handle = self.controls[&id];
        if matches!(id, 1001 | 3301) {
            SendMessageW(handle, LB_SETTOPINDEX, next as usize, 0);
        } else {
            SendMessageW(handle, EM_LINESCROLL, 0, (next - top) as isize);
        }
        InvalidateRect(handle, null(), 0);
        InvalidateRect(self.controls[&scrollbar_id(id)], null(), 0);
    }
    unsafe fn scroll_profiles(&mut self, delta: i32) {
        self.scroll_input(1001, delta);
    }
    unsafe fn scroll_input(&mut self, id: i32, delta: i32) {
        self.profile_wheel += delta;
        let steps = self.profile_wheel / 120;
        self.profile_wheel %= 120;
        if steps == 0 {
            return;
        }
        let (top, _, _, _) = self.scroll_info(id);
        self.scroll_to(id, top - steps * 3);
    }
    unsafe fn drag_scrollbar(&mut self, bar: i32, y: i32, begin: bool) {
        let target = scrollbar_target(bar);
        let (top, count, rows, height) = self.scroll_info(target);
        let thumb = (height * rows / count).max(self.d(12)).min(height);
        let range = (count - rows).max(0);
        let travel = (height - thumb).max(1);
        let thumb_y = top * travel / range.max(1);
        if begin {
            self.scrollbar_drag = Some((
                bar,
                if y >= thumb_y && y < thumb_y + thumb {
                    y - thumb_y
                } else {
                    thumb / 2
                },
            ));
        }
        let offset = self
            .scrollbar_drag
            .filter(|(id, _)| *id == bar)
            .map_or(thumb / 2, |(_, offset)| offset);
        self.scroll_to(target, ((y - offset) * range + travel / 2) / travel);
    }
    unsafe fn dimensions(&self) -> (i32, i32, i32) {
        let mut r = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut r);
        let width = r.right * 96 / self.dpi;
        let visible = r.bottom * 96 / self.dpi;
        let content = match self.page {
            0 => visible.max(460) - NAV_HEIGHT - NAV_GAP,
            1 | 2 => visible.max(740) - NAV_HEIGHT - NAV_GAP,
            3 if self.dns_settings_open => 956,
            3 if self.appearance_open => 1192 + APPEARANCE_EXTRA_HEIGHT,
            _ => 1384,
        };
        (width, visible, content.max(visible - NAV_HEIGHT - NAV_GAP))
    }
    unsafe fn rules_footer_y(&self) -> i32 {
        // Actions, an eight-pixel gap, then the twenty-pixel save hint.
        self.dimensions().1.max(740) - NAV_HEIGHT - NAV_GAP - 38 - 8 - 20
    }
    unsafe fn layout(&self) {
        if !self.controls.contains_key(&1001) {
            return;
        }
        let (width, visible, _height) = self.dimensions();
        let empty = self.model["profiles"]
            .as_array()
            .is_none_or(|a| a.is_empty());
        let key = (
            self.page,
            self.category,
            width,
            visible,
            self.dpi,
            self.scroll,
            self.rename_open,
            self.model["profiles"].as_array().map_or(0, Vec::len),
            self.connection_error().is_some(),
            self.import_open,
        );
        if self.layout_key.get() == Some(key) {
            return;
        }
        let viewport_changed = self
            .layout_key
            .get()
            .is_none_or(|old| old.3 != visible || old.4 != self.dpi);
        self.layout_key.set(Some(key));
        self.layout_passes.set(self.layout_passes.get() + 1);
        let left = ((width - 520) / 2).max(24);
        let area = width - left * 2;
        let mut placements = Vec::new();
        let mut mv = |id: i32, x: i32, y: i32, w: i32, h: i32| {
            if let Some(handle) = self.controls.get(&id) {
                let screen_y = y - if id >= 1000 { self.scroll } else { 0 };
                let page = if matches!(id, 3300..=3399 | 3102 | 3106) {
                    2
                } else if (3100..3200).contains(&id) {
                    1
                } else if id >= 4000 || id >= 3000 {
                    3
                } else if id == 2104 {
                    self.page
                } else if id >= 2000 {
                    1
                } else {
                    0
                };
                let mut show = id < 1000 || page == self.page;
                if matches!(id, 3000..=3099 | 4016) {
                    show &= self.dns_visible();
                }
                if id >= 4000 && id != 4016 {
                    show &= !self.dns_visible();
                    let appearance_control = matches!(id, 4020..=4025 | 4031..=4032 | 4060..=4062)
                        || is_theme_control(id);
                    show &= self.appearance_open == appearance_control;
                }
                if matches!(
                    id,
                    3100 | 3103 | 3104 | 3107 | 3108 | 3109 | 3110 | 3112 | 2302 | 2109
                ) {
                    show &= self.manager_visible();
                }
                if matches!(id, 3103 | 3104 | 3107 | 2302) {
                    show &= self.selected_preset_index().is_some();
                }
                if id == 3107 {
                    show &= self
                        .selected_preset_index()
                        .is_some_and(|i| self.draft["presets"][i]["builtin_id"].is_string());
                }
                if matches!(
                    id,
                    2102 | 2110 | 2103 | 2105 | 2106 | 2107 | 2311 | 2200..=2205
                ) {
                    show &= !self.manager_visible();
                }
                if id == 2104 {
                    show &= matches!(self.page, 1 | 2);
                }
                if id >= 1000 {
                    show &= screen_y + h > 88 && screen_y < visible - NAV_HEIGHT - NAV_GAP && h > 0;
                }
                if [1001, 1004].contains(&id) {
                    show &= !empty;
                }
                if id == 1010 {
                    show &= !empty;
                }
                if id == 1011 {
                    show &= self.connection_error().is_some();
                }
                if [1002, 1004].contains(&id) {
                    show &= !self.rename_open;
                }
                if [1003, 1005].contains(&id) {
                    show = false;
                }
                if [1006, 1009].contains(&id) {
                    show &= !empty && self.rename_open;
                }
                if (1000..1100).contains(&id) {
                    show &= if self.import_open {
                        [1011, 1020, 1021, 1022, 1024].contains(&id)
                    } else {
                        ![1020, 1021, 1022, 1024].contains(&id)
                    };
                }
                if id == 2103 {
                    show &= self.category < 2;
                }
                let was_visible = GetWindowLongPtrW(*handle, GWL_STYLE) as u32 & WS_VISIBLE != 0;
                if !show {
                    if was_visible {
                        placements.push((id, None, SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE));
                    }
                    return;
                }
                let bounds = (self.d(x), self.d(screen_y), self.d(w), self.d(h));
                if self.control_bounds.borrow().get(&id) != Some(&bounds)
                    || !was_visible
                    || viewport_changed
                {
                    placements.push((
                        id,
                        Some(bounds),
                        if was_visible { 0 } else { SWP_SHOWWINDOW },
                    ));
                }
            }
        };
        let nav_width = width / 4;
        for n in 0..4 {
            mv(
                200 + n,
                n * nav_width,
                visible - NAV_HEIGHT,
                nav_width,
                NAV_HEIGHT,
            );
        }
        mv(204, width - 90, 24, 28, 28);
        mv(205, width - 54, 24, 28, 28);
        let home = self.home_layout();
        mv(1007, (width - 240) / 2, home.hero_y, 240, home.hero_h);
        mv(1001, left, home.list_y, area - 16, home.list_h);
        mv(1010, width - left - 10, home.list_y, 10, home.list_h);
        mv(1011, width - left - 28, home.detail_y + 4, 28, 28);
        let import_y = if visible < 560 { 190 } else { 206 };
        let import_buttons_y = home.actions_y;
        let import_error_height = if self.connection_error().is_some() {
            48
        } else {
            0
        };
        mv(
            1020,
            left,
            import_y,
            area - 16,
            (import_buttons_y - import_y - 20 - import_error_height).max(44),
        );
        mv(
            1024,
            width - left - 10,
            import_y,
            10,
            (import_buttons_y - import_y - 20 - import_error_height).max(44),
        );
        mv(1022, left, import_buttons_y, 120, 42);
        mv(1021, left + 132, import_buttons_y, area - 132, 42);
        if self.import_open {
            mv(1011, width - left - 28, import_buttons_y - 44, 28, 28);
        }
        mv(
            1002,
            left,
            home.actions_y,
            if empty { area } else { area - 56 },
            42,
        );
        mv(1005, width - left - 106, 622, 106, 42);
        mv(1006, left, home.actions_y, area - 88, 42);
        mv(1004, width - left - 48, home.actions_y, 48, 42);
        mv(1009, width - left - 76, home.actions_y, 76, 42);
        mv(1003, width - left - 76, 678, 76, 34);
        let category_width = (area - 16) / 3;
        for n in 0..6 {
            mv(
                2200 + n,
                left + (n % 3) * (category_width + 8),
                188 + (n / 3) * 42,
                category_width,
                34,
            );
        }
        let selector_y = if self.manager_visible() { 188 } else { 282 };
        let footer = self.rules_footer_y();
        mv(
            3101,
            left,
            selector_y,
            if self.manager_visible() {
                area
            } else {
                area - 156
            },
            260,
        );
        mv(2311, width - left - 144, 282, 144, 38);
        let footer_width = (area - 24) / 3;
        mv(3100, left, footer, footer_width, 38);
        mv(3103, left, 266, area, 38);
        mv(3104, left, 336, area, 38);
        mv(2302, left + 8, 376, area - 16, 32);
        mv(3107, left, 526, area, 32);
        for (n, (id, _)) in crate::presets::DOMAIN_SOURCES.into_iter().enumerate() {
            mv(
                id,
                left + 8 + (n as i32 % 2) * ((area - 16) / 2),
                440 + (n as i32 / 2) * 40,
                (area - 24) / 2,
                36,
            );
        }
        mv(2102, left, 368, area - 16, footer - 460);
        mv(2110, width - left - 10, 368, 10, footer - 460);
        mv(2105, left, footer - 60, area - 144, 38);
        mv(2106, width - left - 132, footer - 60, 132, 38);
        mv(2107, left, footer, footer_width, 38);
        mv(2103, left + footer_width + 12, footer, footer_width, 38);
        mv(2109, left + footer_width + 12, footer, footer_width, 38);
        mv(2104, width - left - footer_width, footer, footer_width, 38);
        mv(3310, left, 212, area - 144, 38);
        mv(3312, width - left - 132, 212, 132, 38);
        let third = (area - 24) / 3;
        mv(3102, left, 266, third, 38);
        mv(3313, left + third + 12, 266, third, 38);
        mv(3314, left + 2 * (third + 12), 266, third, 38);
        mv(3301, left, 344, area - 16, footer - 388);
        mv(3307, width - left - 10, 344, 10, footer - 388);
        mv(3315, left, footer, footer_width, 38);
        mv(3106, left + footer_width + 12, footer, footer_width, 38);
        mv(4016, left, 188, area, 38);
        for (id, y) in [(3004, 270), (3005, 356)] {
            mv(id, left + 14, y, area - 28, 42);
        }
        mv(3006, left, 454, area - 16, 104);
        mv(3010, width - left - 10, 454, 10, 104);
        let third = (area - 24) / 3;
        for (id, n) in [(3007, 0), (3008, 1), (3009, 2)] {
            mv(id, left + n * (third + 12), 604, third, 38);
        }
        for (id, n) in [(3013, 0), (3014, 1)] {
            mv(
                id,
                left + n * ((area - 12) / 2 + 12),
                694,
                (area - 12) / 2,
                38,
            );
        }
        mv(3011, left, 804, area, 42);
        mv(3012, left, 868, area, 42);
        mv(4002, left + 14, 206, area - 28, 42);
        mv(4001, left + 14, 254, area - 28, 42);
        let language_width = (area - 16) / 3;
        for n in 0..3 {
            mv(
                4010 + n,
                left + n * (language_width + 8),
                472,
                language_width,
                42,
            );
        }
        mv(4005, left, 606, area, 44);
        mv(4003, left, 750, (area - 12) / 2, 44);
        mv(4004, left + (area - 12) / 2 + 12, 750, (area - 12) / 2, 44);
        mv(4006, left + 14, 928, area - 28, 42);
        mv(4007, left + 14, 976, area - 28, 42);
        mv(4008, left, 1094, area, 42);
        mv(4009, left, 1232, area, 42);
        mv(4015, left, 1316, area, 42);
        mv(4030, width - left - 180, 112, 180, 42);
        mv(4031, left, 188, area, 38);
        mv(4032, left, 1124 + APPEARANCE_EXTRA_HEIGHT, area, 38);
        let size_width = (area - 16) / 3;
        for n in 0..3 {
            mv(4060 + n, left + n * (size_width + 8), 282, size_width, 46);
        }
        let card_width = (area - 12) / 2;
        for n in 0..Theme::ALL.len() as i32 {
            mv(
                THEME_CONTROL_START + n,
                left + (n % 2) * (card_width + 12),
                416 + (n / 2) * 116,
                card_width,
                104,
            );
        }
        for (id, y) in [(4020, 876), (4021, 924), (4023, 1060)] {
            mv(id, left + 14, y + APPEARANCE_EXTRA_HEIGHT, area - 28, 42);
        }
        mv(
            4025,
            left + 24,
            976 + APPEARANCE_EXTRA_HEIGHT,
            area - 48,
            28,
        );
        mv(
            4024,
            left + 24,
            1008 + APPEARANCE_EXTRA_HEIGHT,
            area - 48,
            38,
        );
        // Whole-preset activation is drawn in the library rows only.
        ShowWindow(self.controls[&3105], SW_HIDE);
        // Commit sibling geometry together, without copying stale pixels.
        let flags = SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOREDRAW | SWP_NOCOPYBITS;
        let mut batch = BeginDeferWindowPos(placements.len() as i32);
        for &(id, bounds, visibility) in &placements {
            if batch.is_null() {
                break;
            }
            let (x, y, w, h) = bounds.unwrap_or_default();
            batch = DeferWindowPos(
                batch,
                self.controls[&id],
                null_mut(),
                x,
                y,
                w,
                h,
                flags | visibility,
            );
        }
        let committed = !batch.is_null() && EndDeferWindowPos(batch) != 0;
        for (id, bounds, visibility) in placements {
            let control = self.controls[&id];
            if !committed {
                let (x, y, w, h) = bounds.unwrap_or_default();
                SetWindowPos(control, null_mut(), x, y, w, h, flags | visibility);
            }
            if let Some(bounds) = bounds {
                // Preserve the native control height and caret coordinates.
                // A combo's actual height is its selected row, not its dropdown.
                let mut actual = RECT::default();
                GetWindowRect(control, &mut actual);
                let height = actual.bottom - actual.top;
                crate::design::clipped_window_shape(
                    control,
                    actual.right - actual.left,
                    height,
                    self.dpi,
                    self.locale.design.effects && self.locale.design.rounding,
                    if id >= 1000 { self.d(88) - bounds.1 } else { 0 },
                    if id >= 1000 {
                        self.d(visible - NAV_HEIGHT - NAV_GAP) - bounds.1
                    } else {
                        height
                    },
                );
                self.control_bounds.borrow_mut().insert(id, bounds);
                self.geometry_moves.set(self.geometry_moves.get() + 1);
            }
        }
        // Repaint newly exposed regions and edit nonclient padding together
        // after a structural layout change. Animation still uses its narrow
        // invalidation path and never repaints the editors every frame.
        RedrawWindow(
            self.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_NOERASE | RDW_FRAME,
        );
    }
    unsafe fn ensure_buffer(&mut self, source: HDC, size: (i32, i32)) -> bool {
        if size.0 <= 0 || size.1 <= 0 {
            return false;
        }
        if self.buffer_size == size && !self.buffer_dc.is_null() {
            return true;
        }
        if self.buffer_dc.is_null() {
            self.buffer_dc = CreateCompatibleDC(source);
        }
        if self.buffer_dc.is_null() {
            return false;
        }
        if !self.buffer_bitmap.is_null() {
            SelectObject(self.buffer_dc, self.buffer_old);
            DeleteObject(self.buffer_bitmap);
        }
        self.buffer_bitmap = CreateCompatibleBitmap(source, size.0, size.1);
        if self.buffer_bitmap.is_null() {
            self.buffer_size = (0, 0);
            return false;
        }
        self.buffer_old = SelectObject(self.buffer_dc, self.buffer_bitmap);
        self.buffer_size = size;
        true
    }
    unsafe fn paint(&self, dc: HDC) {
        let mut client = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut client);
        let (width, visible, _) = self.dimensions();
        let left = ((width - 520) / 2).max(24);
        let area = width - left * 2;
        if self.page != 0 {
            text(
                self,
                dc,
                if self.dns_visible() {
                    "DNS"
                } else if self.page == 3 && self.appearance_open {
                    "Оформление"
                } else {
                    PAGES[self.page]
                },
                left,
                112,
                area,
                42,
                1,
                INK,
            );
            text(
                self,
                dc,
                match self.page {
                    1 => "Шесть типов правил. Свои списки и готовые наборы.",
                    2 => "Создайте набор, включите его и настройте правила.",
                    3 if self.dns_settings_open => "Обновление доменных списков и IP-адресов",
                    3 if self.appearance_open => "Твой стиль. Твой размер. Без перезапуска.",
                    _ => "Твой VPN. Твои привычки.",
                },
                left,
                154,
                area,
                25,
                3,
                MUTED,
            );
        }
        match self.page {
            0 if self.import_open => {
                text(self, dc, "Новая конфигурация", left, 112, area, 42, 1, INK);
                text(
                    self,
                    dc,
                    "Вставь ссылку или текст конфигурации (Ctrl+V)",
                    left,
                    158,
                    area,
                    28,
                    3,
                    MUTED,
                );
                if let Some(error) = self.connection_error() {
                    centered_wrapped(
                        self,
                        dc,
                        &self.locale.error(error),
                        left,
                        self.home_layout().actions_y - 48,
                        area - 36,
                        36,
                        3,
                        DANGER,
                    );
                }
            }
            0 => {
                let home = self.home_layout();
                let phase = self.phase();
                let detail = self.connection_error().unwrap_or_default();
                let error = self.connection_error().is_some();
                centered_wrapped(
                    self,
                    dc,
                    &self.locale.error(detail),
                    left,
                    home.detail_y,
                    area - if error { 36 } else { 0 },
                    36,
                    3,
                    if error { DANGER } else { MUTED },
                );
                text(
                    self,
                    dc,
                    &format!(
                        "СЕРВЕРЫ · {}",
                        self.model["profiles"].as_array().map_or(0, Vec::len)
                    ),
                    left,
                    home.header_y,
                    area / 2,
                    20,
                    3,
                    MUTED,
                );
                if let Some(count) = self.imported_count {
                    text_right(
                        self,
                        dc,
                        &format!("Добавлено: {count}"),
                        left + area / 2,
                        home.header_y,
                        area / 2,
                        20,
                        3,
                        MUTED,
                    );
                } else if phase == "idle" && !self.busy {
                    text_right(
                        self,
                        dc,
                        "Выбери и подключись",
                        left + area / 2,
                        home.header_y,
                        area / 2,
                        20,
                        3,
                        MUTED,
                    );
                }
                if self.model["profiles"]
                    .as_array()
                    .is_none_or(|a| a.is_empty())
                {
                    centered(
                        self,
                        dc,
                        "Первый сервер ждёт тебя",
                        left,
                        home.list_y + 40,
                        area,
                        26,
                        2,
                        INK,
                    );
                    centered(
                        self,
                        dc,
                        "Добавь файл конфигурации или ссылку",
                        left + 12,
                        home.list_y + 76,
                        area - 24,
                        28,
                        3,
                        MUTED,
                    );
                }
                if self.rename_open
                    && !self.model["profiles"]
                        .as_array()
                        .is_none_or(|a| a.is_empty())
                {
                    text(
                        self,
                        dc,
                        "Имя сервера",
                        left,
                        home.actions_y - 22,
                        area,
                        18,
                        3,
                        MUTED,
                    );
                }
            }
            1 => self.paint_rules(dc, left, area),
            2 => self.paint_preset_library(dc, left, area),
            3 if self.appearance_open => self.paint_appearance(dc, left, area),
            3 if self.dns_settings_open => {
                text(self, dc, "IP ПО ДОМЕНАМ", left, 240, area, 20, 3, MUTED);
                text(
                    self,
                    dc,
                    "Доменные правила работают и без IP-кэша.",
                    left,
                    320,
                    area,
                    28,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Cloudflare → Google · A / AAAA · CNAME · TTL",
                    left,
                    415,
                    area,
                    24,
                    3,
                    TEAL,
                );
                text(
                    self,
                    dc,
                    "Домены для обновления IP",
                    left,
                    437,
                    area,
                    20,
                    3,
                    MUTED,
                );
                let third = (area - 24) / 3;
                for (label, n) in [
                    ("Мин. интервал, сек", 0),
                    ("Макс. интервал, сек", 1),
                    ("При ошибке, сек", 2),
                ] {
                    text(
                        self,
                        dc,
                        label,
                        left + n * (third + 12),
                        578,
                        third,
                        25,
                        3,
                        MUTED,
                    );
                }
                for (label, n) in [("Лимит доменов", 0), ("Адресов на домен", 1)]
                {
                    text(
                        self,
                        dc,
                        label,
                        left + n * ((area - 12) / 2 + 12),
                        668,
                        (area - 12) / 2,
                        25,
                        3,
                        MUTED,
                    );
                }
                text(
                    self,
                    dc,
                    "Учитывается TTL ответа. При сбое остаются последние\nпроверенные адреса в пределах заданного срока.",
                    left,
                    748,
                    area,
                    46,
                    3,
                    MUTED,
                );
            }
            _ => {
                rounded(self, dc, left, 192, area, 220, PANEL, LINE);
                text(
                    self,
                    dc,
                    "При входе в Windows приложение запустится в трее.\nVPN подключится к последнему включённому серверу,\nесли включено автоподключение.",
                    left + 14,
                    306,
                    area - 28,
                    70,
                    3,
                    MUTED,
                );
                text(self, dc, "ЯЗЫК ИНТЕРФЕЙСА", left, 438, area, 20, 3, MUTED);
                text(self, dc, "ЛЕГКО В ФОНЕ", left, 550, area, 20, 3, MUTED);
                text(
                    self,
                    dc,
                    "Можно закрыть окно — подключение продолжит работать.
В трее интерфейс ждёт без фоновой перерисовки.",
                    left,
                    576,
                    area,
                    38,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "ЕСЛИ ЧТО-ТО ПОШЛО НЕ ТАК",
                    left,
                    690,
                    area,
                    20,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Проверь связь и открой локальные журналы.",
                    left,
                    717,
                    area,
                    22,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Ключи конфигураций хранятся только на этом компьютере.
Никакой регистрации и загрузки профилей в облако.",
                    left,
                    828,
                    area,
                    47,
                    3,
                    MUTED,
                );
                text(self, dc, "МАРШРУТИЗАЦИЯ", left, 896, area, 20, 3, MUTED);
                text(
                    self,
                    dc,
                    "В полном режиме исключения приложений и сайтов не действуют.\nДля смены режима сначала отключите VPN.",
                    left,
                    1032,
                    area,
                    46,
                    3,
                    MUTED,
                );
            }
        }
        SetViewportOrgEx(dc, 0, 0, null_mut());
        {
            let header = RECT {
                left: 0,
                top: 0,
                right: client.right,
                bottom: self.d(88),
            };
            surface_fill(self, dc, &header, BG);
            mushroom(self, dc, left - 6, 20, 48);
            text(self, dc, crate::APP_NAME, left + 48, 34, 180, 30, 2, INK);
            text_raw(
                self,
                dc,
                concat!("v", env!("CARGO_PKG_VERSION")),
                left + 170,
                38,
                110,
                20,
                3,
                MUTED,
            );
        }
        let (_, _, content) = self.dimensions();
        if content > visible - NAV_HEIGHT - NAV_GAP {
            let track = page_scroll_track(visible, content, self.scroll);
            rounded(self, dc, width - 9, track.top, 5, track.height, LINE, LINE);
            rounded(
                self,
                dc,
                width - 9,
                track.position,
                5,
                track.thumb,
                INDIGO,
                INDIGO,
            );
        }
        let nav = RECT {
            left: 0,
            top: self.d(visible - NAV_HEIGHT - NAV_GAP),
            right: client.right,
            bottom: client.bottom,
        };
        surface_fill(self, dc, &nav, BG);
    }
}
struct PageScrollTrack {
    top: i32,
    height: i32,
    thumb: i32,
    position: i32,
    range: i32,
}
impl PageScrollTrack {
    fn scroll_at(&self, y: i32, offset: i32) -> i32 {
        let travel = (self.height - self.thumb).max(1);
        ((y - self.top - offset).clamp(0, travel) * self.range + travel / 2) / travel
    }
}
fn page_scroll_track(visible: i32, content: i32, scroll: i32) -> PageScrollTrack {
    let top = 96;
    let height = (visible - NAV_HEIGHT - NAV_GAP - 112).max(1);
    let viewport = (visible - NAV_HEIGHT - NAV_GAP - 88).max(1);
    let range = (content - visible + NAV_HEIGHT + NAV_GAP).max(0);
    let thumb = (height * viewport / (viewport + range)).max(42).min(height);
    let position = top + (height - thumb) * scroll / range.max(1);
    PageScrollTrack {
        top,
        height,
        thumb,
        position,
        range,
    }
}
fn scrollbar_id(target: i32) -> i32 {
    match target {
        1001 => 1010,
        1020 => 1024,
        2102 => 2110,
        3301 => 3307,
        _ => 3010,
    }
}
fn scrollbar_target(bar: i32) -> i32 {
    match bar {
        1010 => 1001,
        1024 => 1020,
        2110 => 2102,
        3307 => 3301,
        _ => 3006,
    }
}
fn joined(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .unwrap_or_default()
}
fn import_request(content: String, name: &str, source: &str) -> Value {
    json!({"action":"Import","content":content,"name":name,
        "format":"auto","source_name":source})
}
fn read_import_content(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path)
        .map_err(|_| "Не удалось прочитать конфигурацию (максимум 512 КБ)".to_string())?;
    let mut bytes = Vec::new();
    file.take((IMPORT_LIMIT + 4) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Не удалось прочитать конфигурацию (максимум 512 КБ)".to_string())?;
    let content =
        String::from_utf8(bytes).map_err(|_| "Файл должен иметь кодировку UTF-8".to_string())?;
    let content = content.trim_start_matches('\u{feff}');
    if content.len() > IMPORT_LIMIT {
        Err("Конфигурация слишком большая (максимум 512 КБ)".into())
    } else if content.trim().is_empty() {
        Err("Файл конфигурации пуст".into())
    } else {
        Ok(content.into())
    }
}
fn profile_protocol(profile: &Value) -> &str {
    match profile["protocol"].as_str().unwrap_or("") {
        "amneziawg" => "AWG",
        "wireguard" => "WG",
        "WireGuard / AmneziaWG" => "WG / AWG",
        "vless" => "VLESS",
        "vmess" => "VMess",
        "ss" => "Shadowsocks",
        "ssr" => "ShadowsocksR",
        "trojan" => "Trojan",
        "hysteria" => "Hysteria",
        "hysteria2" => "Hysteria2",
        "tuic" => "TUIC",
        "socks5" => "SOCKS5",
        "http" => "HTTP",
        "snell" => "Snell",
        "anytls" => "AnyTLS",
        "mieru" => "Mieru",
        "trusttunnel" => "TrustTunnel",
        "shadowquic" => "ShadowQUIC",
        "gost-relay" => "GOST Relay",
        "ssh" => "SSH",
        "masque" => "MASQUE",
        "sudoku" => "Sudoku",
        "openvpn" => "OpenVPN",
        other => other,
    }
}
unsafe fn measured_width(s: &State, dc: HDC, value: &str, font: usize) -> i32 {
    let old = SelectObject(dc, s.fonts[font]);
    let value = wide(value);
    let mut size: SIZE = std::mem::zeroed();
    GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
    SelectObject(dc, old);
    (size.cx * 96 + s.dpi - 1) / s.dpi
}
fn controller_request(
    request: &Value,
    mut rpc: impl FnMut(&Value) -> Result<Value, String>,
    recover: impl FnOnce() -> Result<(), String>,
) -> Result<Value, String> {
    if matches!(request["action"].as_str(), Some("Disconnect" | "Exit")) {
        // A reachable authenticated controller also covers portable QA workers.
        // Probe before a mutation, never restart/retry an ambiguous cleanup RPC.
        let ready = rpc(&json!({"action":"Status"}))
            .is_ok_and(|reply| reply["ok"] == true && reply["data"]["service_ready"] == true);
        if !ready {
            recover()?;
        }
    }
    rpc(request)
}
fn launch(hwnd: HWND, root: PathBuf, request: Value, generation: u64) {
    let window = hwnd as isize;
    let action = request["action"].as_str().unwrap_or("Status").to_owned();
    let request_id = request["request_id"].as_str().map(str::to_owned);
    thread::spawn(move || {
        let response = controller_request(
            &request,
            |value| backend::rpc(&root, value),
            || crate::setup::ensure_controller_for_recovery(&root),
        );
        let pointer = Box::into_raw(Box::new(Answer {
            response,
            action,
            generation,
            request_id,
        }));
        unsafe {
            if PostMessageW(window as HWND, REPLY, 0, pointer as isize) == 0 {
                drop(Box::from_raw(pointer));
            }
        }
    });
}
#[allow(clippy::too_many_arguments)] // GDI drawing primitive: position, size and colors.
unsafe fn rounded(s: &State, dc: HDC, x: i32, y: i32, w: i32, h: i32, color: u32, border: u32) {
    let rect = RECT {
        left: s.d(x),
        top: s.d(y),
        right: s.d(x + w),
        bottom: s.d(y + h),
    };
    pixel_panel(s, dc, rect, color, border);
}
unsafe fn pixel_panel(s: &State, dc: HDC, rect: RECT, color: u32, border: u32) {
    if !(s.locale.design.effects && s.locale.design.rounding) {
        surface_fill(s, dc, &rect, color);
        if border != color {
            FrameRect(dc, &rect, s.textures.brush(border));
        }
        return;
    }
    let region = s.regions.get(
        rect.right - rect.left,
        rect.bottom - rect.top,
        crate::design::corner_unit(s.dpi),
    );
    if region.is_null() {
        surface_fill(s, dc, &rect, color);
        return;
    }
    OffsetRgn(region, rect.left, rect.top);
    let saved = SaveDC(dc);
    if saved != 0 {
        let (mut viewport, mut window) = (POINT::default(), POINT::default());
        GetViewportOrgEx(dc, &mut viewport);
        GetWindowOrgEx(dc, &mut window);
        let (dx, dy) = (viewport.x - window.x, viewport.y - window.y);
        OffsetRgn(region, dx, dy);
        ExtSelectClipRgn(dc, region, RGN_AND);
        surface_fill(s, dc, &rect, color);
        OffsetRgn(region, -dx, -dy);
        RestoreDC(dc, saved);
        if border != color {
            FrameRgn(
                dc,
                region,
                s.textures.brush(border),
                s.d(1).max(1),
                s.d(1).max(1),
            );
        }
    }
    OffsetRgn(region, -rect.left, -rect.top);
}
unsafe fn surface_fill(s: &State, dc: HDC, rect: &RECT, color: u32) {
    s.textures.fill(dc, rect, color);
}
unsafe fn menu_label(data: usize) -> String {
    if data == 0 {
        return String::new();
    }
    let pointer = data as *const u16;
    let mut length = 0;
    while length < 1024 && *pointer.add(length) != 0 {
        length += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length))
}
unsafe fn pixel_menu(s: &State, items: &[(usize, &str)]) -> (HMENU, Vec<Vec<u16>>) {
    let menu = CreatePopupMenu();
    let labels: Vec<_> = items
        .iter()
        .map(|(_, label)| wide(&s.locale.text(label)))
        .collect();
    for ((id, _), label) in items.iter().zip(&labels) {
        AppendMenuW(menu, MF_STRING, *id, label.as_ptr());
        let mut info: MENUITEMINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = MIIM_FTYPE | MIIM_DATA;
        info.fType = MFT_OWNERDRAW;
        info.dwItemData = label.as_ptr() as usize;
        SetMenuItemInfoW(menu, *id as u32, 0, &info);
    }
    let mut info: MENUINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of_val(&info) as u32;
    info.fMask = MIM_BACKGROUND | MIM_STYLE;
    info.hbrBack = s.white;
    info.dwStyle = MNS_NOCHECK;
    SetMenuInfo(menu, &info);
    (menu, labels)
}
#[allow(clippy::too_many_arguments)] // GDI text primitive: bounds, font and color.
unsafe fn text(
    s: &State,
    dc: HDC,
    value: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: usize,
    color: u32,
) {
    text_raw(s, dc, &s.locale.text(value), x, y, w, h, font, color);
}
#[allow(clippy::too_many_arguments)]
unsafe fn text_raw(
    s: &State,
    dc: HDC,
    value: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: usize,
    color: u32,
) {
    let old = SelectObject(dc, s.fonts[font]);
    SetTextColor(dc, s.textures.color(color));
    SetBkMode(dc, TRANSPARENT as i32);
    let rect = RECT {
        left: s.d(x),
        top: s.d(y),
        right: s.d(x + w),
        bottom: s.d(y + h),
    };
    let mut metrics = std::mem::zeroed();
    GetTextMetricsW(dc, &mut metrics);
    let line_height = s.d(match font {
        0 | 2 => 24,
        1 => 32,
        _ => 16,
    });
    let mut line_y = rect.top;
    let measure = |v: &str| {
        let value = wide(v);
        let mut size = std::mem::zeroed();
        GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
        size.cx
    };
    let mut draw = |v: &str| {
        if line_y < rect.bottom {
            let value = wide(v);
            ExtTextOutW(
                dc,
                rect.left,
                line_y - metrics.tmInternalLeading,
                ETO_CLIPPED,
                &rect,
                value.as_ptr(),
                value.len() as u32 - 1,
                null(),
            );
        }
        line_y += line_height;
    };
    for paragraph in value.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{} {}", line, word)
            };
            if !line.is_empty() && measure(&candidate) > rect.right - rect.left {
                draw(&line);
                line = word.to_owned();
            } else {
                line = candidate;
            }
        }
        draw(&line);
    }
    SelectObject(dc, old);
}
#[allow(clippy::too_many_arguments)]
unsafe fn centered(
    s: &State,
    dc: HDC,
    value: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: usize,
    color: u32,
) {
    let translated = s.locale.text(value);
    let value = translated.as_ref();
    let old = SelectObject(dc, s.fonts[font]);
    SetTextColor(dc, s.textures.color(color));
    SetBkMode(dc, TRANSPARENT as i32);
    let mut metrics = std::mem::zeroed();
    GetTextMetricsW(dc, &mut metrics);
    let ink_height = s.d(match font {
        1 => 16,
        0 | 2 => 12,
        _ => 8,
    });
    let top = s.d(y) + (s.d(h) - ink_height) / 2 - metrics.tmInternalLeading;
    let mut rect = RECT {
        left: s.d(x),
        top,
        right: s.d(x + w),
        bottom: top + metrics.tmHeight,
    };
    DrawTextW(
        dc,
        wide(value).as_ptr(),
        -1,
        &mut rect,
        DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SelectObject(dc, old);
}
#[allow(clippy::too_many_arguments)]
unsafe fn text_right(
    s: &State,
    dc: HDC,
    value: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: usize,
    color: u32,
) {
    let value = s.locale.text(value);
    let old = SelectObject(dc, s.fonts[font]);
    let mut size = std::mem::zeroed();
    let wide_value = wide(&value);
    GetTextExtentPoint32W(
        dc,
        wide_value.as_ptr(),
        wide_value.len() as i32 - 1,
        &mut size,
    );
    SelectObject(dc, old);
    text_raw(
        s,
        dc,
        &value,
        x + (w - size.cx * 96 / s.dpi).max(0),
        y,
        w,
        h,
        font,
        color,
    );
}

fn wrap_lines(
    value: &str,
    width: i32,
    maximum: usize,
    measure: impl Fn(&str) -> i32,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in value.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && measure(&candidate) > width {
                lines.push(line);
                if lines.len() == maximum {
                    return lines;
                }
                line = word.to_owned();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
        if lines.len() == maximum {
            break;
        }
    }
    lines
}

#[allow(clippy::too_many_arguments)]
unsafe fn centered_wrapped(
    s: &State,
    dc: HDC,
    value: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: usize,
    color: u32,
) {
    let old = SelectObject(dc, s.fonts[font]);
    SetTextColor(dc, s.textures.color(color));
    SetBkMode(dc, TRANSPARENT as i32);
    let mut metrics = std::mem::zeroed();
    GetTextMetricsW(dc, &mut metrics);
    let line_height = s.d(16);
    let ink_height = s.d(8);
    let measure = |text: &str| {
        let text = wide(text);
        let mut size = std::mem::zeroed();
        GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
        size.cx
    };
    let lines = wrap_lines(
        value,
        s.d(w),
        (s.d(h) / line_height).max(1) as usize,
        measure,
    );
    let ink_block = (lines.len().saturating_sub(1) as i32) * line_height + ink_height;
    let top = s.d(y) + (s.d(h) - ink_block) / 2;
    let rect = RECT {
        left: s.d(x),
        top: s.d(y),
        right: s.d(x + w),
        bottom: s.d(y + h),
    };
    for (row, line) in lines.iter().enumerate() {
        let line_width = measure(line);
        let value = wide(line);
        ExtTextOutW(
            dc,
            rect.left + ((rect.right - rect.left - line_width) / 2).max(0),
            top + row as i32 * line_height - metrics.tmInternalLeading,
            ETO_CLIPPED,
            &rect,
            value.as_ptr(),
            value.len() as u32 - 1,
            null(),
        );
    }
    SelectObject(dc, old);
}
unsafe fn mushroom(s: &State, dc: HDC, x: i32, y: i32, size: i32) {
    mushroom_palette(s, dc, x, y, size, 1);
}
fn mushroom_grid_bounds(s: &State, x: i32, y: i32, size: i32) -> RECT {
    RECT {
        left: s.d(x),
        top: s.d(y),
        right: s.d(x) + s.d(size),
        bottom: s.d(y) + s.d(size),
    }
}
unsafe fn mushroom_palette(s: &State, dc: HDC, x: i32, y: i32, size: i32, palette: usize) {
    draw_mushroom(
        s,
        dc,
        &mushroom::MINI,
        mushroom_grid_bounds(s, x, y, size),
        palette,
    );
}
fn hero_mushroom_bounds(s: &State, width: i32, height: i32) -> RECT {
    let art_height = s.d(height);
    let art_width = art_height * mushroom::HERO_WIDTH as i32 / mushroom::HERO_HEIGHT as i32;
    let x = (s.d(width) - art_width) / 2;
    RECT {
        left: x,
        top: s.d(8),
        right: x + art_width,
        bottom: s.d(8) + art_height,
    }
}
unsafe fn draw_mushroom<const W: usize, const H: usize>(
    s: &State,
    dc: HDC,
    grid: &[&[u8; W]; H],
    bounds: RECT,
    palette: usize,
) {
    draw_mushroom_visible(s, dc, grid, bounds, palette, |_, _| true);
}
unsafe fn draw_mushroom_visible<const W: usize, const H: usize>(
    s: &State,
    dc: HDC,
    grid: &[&[u8; W]; H],
    bounds: RECT,
    palette: usize,
    visible: impl Fn(usize, usize) -> bool,
) {
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let brushes = s.mushroom_brushes[palette];
    for (row, line) in grid.iter().enumerate() {
        let mut col = 0;
        while col < W {
            let start = col;
            let pixel = line[col];
            let Some(index) = mushroom::palette_index(pixel) else {
                col += 1;
                continue;
            };
            if !visible(row, col) {
                col += 1;
                continue;
            }
            col += 1;
            while col < W && line[col] == pixel && visible(row, col) {
                col += 1;
            }
            // Project source pixel edges without filtering. Fractional DPI and
            // interface sizes fit the allotted space instead of rounding up
            // every source cell and overflowing into the caption.
            let r = RECT {
                left: bounds.left + start as i32 * width / W as i32,
                top: bounds.top + row as i32 * height / H as i32,
                right: bounds.left + col as i32 * width / W as i32,
                bottom: bounds.top + (row as i32 + 1) * height / H as i32,
            };
            FillRect(dc, &r, brushes[index]);
        }
    }
}
fn icon_grid(kind: usize) -> &'static [&'static str; 12] {
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
            "....XXXX....",
            "....XXXX....",
            "..XXXXXXXX..",
            "..XXXXXXXX..",
            "XXXX....XXXX",
            "XXXX....XXXX",
            "XXXX....XXXX",
            "XXXX....XXXX",
            "..XXXXXXXX..",
            "..XXXXXXXX..",
            "....XXXX....",
            "....XXXX....",
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
fn icon_grid_bounds(s: &State, x: i32, y: i32, size: i32) -> (i32, i32, i32) {
    let cell = s
        .locale
        .design
        .interface_size
        .cell((size / 12).max(1), s.dpi);
    let inset = (s.d(size) - cell * 12) / 2;
    (s.d(x) + inset, s.d(y) + inset, cell)
}
unsafe fn icon(s: &State, dc: HDC, kind: usize, x: i32, y: i32, size: i32, color: u32) {
    if kind == 4 {
        // One checkmark everywhere: exactly four large square pixels.
        // Integer device cells keep the chunky silhouette sharp at any DPI.
        let cell = (s.d(size) / 4).max(1);
        let left = s.d(x) + (s.d(size) - 4 * cell) / 2;
        let top = s.d(y) + (s.d(size) - 3 * cell) / 2;
        for (column, row) in [(0, 1), (1, 2), (2, 1), (3, 0)] {
            FillRect(
                dc,
                &RECT {
                    left: left + column * cell,
                    top: top + row * cell,
                    right: left + (column + 1) * cell,
                    bottom: top + (row + 1) * cell,
                },
                s.textures.brush(color),
            );
        }
        return;
    }
    let (origin_x, origin_y, cell) = icon_grid_bounds(s, x, y, size);
    let brush = s.textures.brush(color);
    for (row, line) in icon_grid(kind).iter().enumerate() {
        for (col, ch) in line.bytes().enumerate() {
            if ch == b'X' {
                let a = origin_x + col as i32 * cell;
                let b = origin_y + row as i32 * cell;
                let rect = RECT {
                    left: a,
                    top: b,
                    right: a + cell,
                    bottom: b + cell,
                };
                FillRect(dc, &rect, brush);
            }
        }
    }
}
// An integer physical pixel cell keeps these glyphs square at fractional DPI.
// Caption hitboxes and button semantics remain native, independent of the art.
unsafe fn caption_glyph(
    dc: HDC,
    rect: &RECT,
    dpi: i32,
    size: InterfaceSize,
    close: bool,
    color: u32,
) {
    let grid = if close {
        [
            "XX....XX", "XXX..XXX", ".XXXXXX.", "..XXXX..", "..XXXX..", ".XXXXXX.", "XXX..XXX",
            "XX....XX",
        ]
    } else {
        [
            "........", "........", "........", "XXXXXXXX", "XXXXXXXX", "........", "........",
            "........",
        ]
    };
    let cell = size
        .cell(3, dpi)
        .min(((rect.right - rect.left).min(rect.bottom - rect.top) / 8).max(1));
    let left = rect.left + (rect.right - rect.left - cell * 8) / 2;
    let top = rect.top + (rect.bottom - rect.top - cell * 8) / 2;
    let brush = CreateSolidBrush(color);
    for (row, line) in grid.iter().enumerate() {
        for (column, value) in line.bytes().enumerate() {
            if value == b'X' {
                FillRect(
                    dc,
                    &RECT {
                        left: left + column as i32 * cell,
                        top: top + row as i32 * cell,
                        right: left + (column as i32 + 1) * cell,
                        bottom: top + (row as i32 + 1) * cell,
                    },
                    brush,
                );
            }
        }
    }
    DeleteObject(brush);
}
unsafe fn choice_file(s: &State, exe: bool) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let description = s.locale.text(if exe {
        "Приложения Windows"
    } else {
        "Конфигурации VPN"
    });
    let filter = wide(&format!(
        "{}\0{}\0\0",
        description,
        if exe {
            "*.exe"
        } else {
            "*.conf;*.ovpn;*.yaml;*.yml;*.json;*.txt"
        }
    ));
    let title = wide(&s.locale.text(if exe {
        "Добавить .exe"
    } else {
        "Добавить конфигурацию"
    }));
    let mut dialog: OPENFILENAMEW = std::mem::zeroed();
    dialog.lStructSize = std::mem::size_of_val(&dialog) as u32;
    dialog.hwndOwner = s.hwnd;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
    if GetOpenFileNameW(&mut dialog) == 0 {
        return None;
    }
    let n = buffer.iter().position(|b| *b == 0).unwrap();
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..n])))
}
unsafe fn shell_open(s: &State, target: &str, parameters: Option<&str>, verb: &str) {
    let params = parameters.map(wide);
    ShellExecuteW(
        s.hwnd,
        wide(verb).as_ptr(),
        wide(target).as_ptr(),
        params.as_ref().map_or(null(), |p| p.as_ptr()),
        wide(&s.base.to_string_lossy()).as_ptr(),
        SW_SHOWNORMAL,
    );
}
#[allow(clippy::manual_dangling_ptr)] // Win32 MAKEINTRESOURCE: identifier 1, never a dereferenced pointer.
unsafe fn product_icon() -> HICON {
    LoadIconW(GetModuleHandleW(null()), 1usize as *const u16)
}
struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}
struct ProductIcons {
    small: OwnedIcon,
    large: OwnedIcon,
    dpi: i32,
}
impl ProductIcons {
    unsafe fn new(theme: Theme, dpi: i32) -> Result<Self, String> {
        let palette = mushroom_colors(theme)
            .map(|color| (color & 255) << 16 | (color & 0xff00) | (color >> 16));
        let create = |metric| -> Result<OwnedIcon, String> {
            let size = GetSystemMetricsForDpi(metric, dpi as u32).clamp(16, 256);
            let dib = mushroom::icon_dib(size as usize, &palette);
            let icon = CreateIconFromResourceEx(
                dib.as_ptr(),
                dib.len() as u32,
                1,
                0x0003_0000,
                size,
                size,
                LR_DEFAULTCOLOR,
            );
            if icon.is_null() {
                Err("Не удалось создать значок приложения".into())
            } else {
                Ok(OwnedIcon(icon))
            }
        };
        Ok(Self {
            small: create(SM_CXSMICON)?,
            large: create(SM_CXICON)?,
            dpi,
        })
    }
    unsafe fn apply(&self, hwnd: HWND) {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, self.small.0 as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, self.large.0 as isize);
    }
}
unsafe fn tray(s: &State, add: bool) {
    tray_notify(s, if add { NIM_ADD } else { NIM_DELETE });
}
unsafe fn tray_update(s: &State) {
    tray_notify(s, NIM_MODIFY);
}
unsafe fn tray_notify(s: &State, operation: u32) {
    let mut icon: NOTIFYICONDATAW = std::mem::zeroed();
    icon.cbSize = std::mem::size_of_val(&icon) as u32;
    icon.hWnd = s.hwnd;
    icon.uID = 1;
    icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    icon.uCallbackMessage = TRAY;
    icon.hIcon = s.product_icons.small.0;
    let tip = wide(&s.locale.text("Mukhomor · открыть настройки"));
    icon.szTip[..tip.len()].copy_from_slice(&tip);
    Shell_NotifyIconW(operation, &icon);
}
unsafe fn configure_custom_chrome(hwnd: HWND) {
    // WS_THICKFRAME retains native vertical sizing, but the whole visible
    // window belongs to our client renderer. DWM must not add its own frame.
    let policy = DWMNCRP_DISABLED;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY as u32,
        &policy as *const _ as *const std::ffi::c_void,
        std::mem::size_of_val(&policy) as u32,
    );
    let square = DWMWCP_DONOTROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &square as *const _ as *const std::ffi::c_void,
        std::mem::size_of_val(&square) as u32,
    );
}
unsafe fn paint_draw_item(s: &mut State, draw: &DRAWITEMSTRUCT) -> isize {
    if !matches!(draw.CtlType, ODT_BUTTON | ODT_STATIC) || draw.CtlID == 1007 {
        return render_draw_item(s, draw);
    }
    let id = draw.CtlID as i32;
    let mut buffer = s.control_buffers.remove(&id).unwrap_or_default();
    if !buffer.ensure(draw.hDC, (draw.rcItem.right, draw.rcItem.bottom)) {
        return render_draw_item(s, draw);
    }
    let mut buffered = *draw;
    buffered.hDC = buffer.dc;
    let saved = SaveDC(buffer.dc);
    let result = render_draw_item(s, &buffered);
    RestoreDC(buffer.dc, saved);
    let r = draw.rcItem;
    // Owner-drawn gallery cards also receive offscreen print DCs, which can
    // omit the HWND's viewport clip. Keep partial rows below the fixed header
    // and above navigation when publishing the cached card.
    let gallery_clip = if is_theme_control(id) {
        let saved = SaveDC(draw.hDC);
        if let Some(bounds) = s.control_bounds.borrow().get(&id) {
            IntersectClipRect(
                draw.hDC,
                r.left,
                (s.d(88) - bounds.1).max(r.top),
                r.right,
                (s.d(s.dimensions().1 - NAV_HEIGHT - NAV_GAP) - bounds.1).min(r.bottom),
            );
        }
        saved
    } else {
        0
    };
    BitBlt(
        draw.hDC,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        buffer.dc,
        r.left,
        r.top,
        SRCCOPY,
    );
    if gallery_clip != 0 {
        RestoreDC(draw.hDC, gallery_clip);
    }
    s.control_buffers.insert(id, buffer);
    result
}
unsafe fn render_draw_item(s: &mut State, draw: &DRAWITEMSTRUCT) -> isize {
    let dc = draw.hDC;
    let r = draw.rcItem;
    let id = draw.CtlID as i32;
    if is_theme_control(id) {
        return render_theme_card(s, draw);
    }
    if id == 4025 {
        FillRect(dc, &r, s.textures.brush(PANEL));
        let w = (r.right - r.left) * 96 / s.dpi;
        let color = if IsWindowEnabled(draw.hwndItem) != 0 {
            INK
        } else {
            MUTED
        };
        text(s, dc, "Скорость фона", 0, 5, w - 80, 22, 3, color);
        centered(
            s,
            dc,
            &format!("{}%", s.background_speed()),
            w - 76,
            0,
            76,
            28,
            0,
            color,
        );
        return 1;
    }
    if draw.CtlType == ODT_MENU {
        FillRect(dc, &r, s.white);
        let on = draw.itemState & ODS_SELECTED != 0;
        pixel_panel(s, dc, r, PANEL_ALT, if on { TEAL } else { PANEL_ALT });
        text_raw(
            s,
            dc,
            &menu_label(draw.itemData),
            r.left * 96 / s.dpi + 14,
            r.top * 96 / s.dpi + 13,
            (r.right - r.left) * 96 / s.dpi - 28,
            24,
            0,
            if on { TEAL } else { INK },
        );
        return 1;
    }
    let origin = s
        .control_bounds
        .borrow()
        .get(&id)
        .copied()
        .unwrap_or((0, 0, 0, 0));
    SetBrushOrgEx(dc, -origin.0, -origin.1, null_mut());
    if draw.CtlType == ODT_COMBOBOX {
        let color = if draw.itemState & ODS_SELECTED != 0 {
            LINE
        } else {
            PANEL_ALT
        };
        FillRect(dc, &r, s.textures.brush(color));
        if draw.itemID != u32::MAX {
            let len = SendMessageW(draw.hwndItem, CB_GETLBTEXTLEN, draw.itemID as usize, 0);
            if (0..=1024).contains(&len) {
                let mut label = vec![0u16; len as usize + 1];
                SendMessageW(
                    draw.hwndItem,
                    CB_GETLBTEXT,
                    draw.itemID as usize,
                    label.as_mut_ptr() as isize,
                );
                text_raw(
                    s,
                    dc,
                    &String::from_utf16_lossy(&label[..len as usize]),
                    r.left * 96 / s.dpi + 10,
                    r.top * 96 / s.dpi + 11,
                    (r.right - r.left) * 96 / s.dpi - 20,
                    24,
                    0,
                    INK,
                );
            }
        }
        return 1;
    }
    if [1010, 1024, 2110, 3010, 3307].contains(&id) {
        surface_fill(s, dc, &r, BG);
        let (top, count, rows, height) = s.scroll_info(scrollbar_target(id));
        if count <= rows {
            return 1;
        }
        let thumb = (height * rows / count).max(s.d(12)).min(height);
        let travel = (height - thumb).max(0);
        let pos = top * travel / (count - rows).max(1);
        let brush = CreateSolidBrush(s.textures.color(if count > rows { INDIGO } else { LINE }));
        let rect = RECT {
            left: s.d(2),
            top: pos,
            right: (r.right - s.d(2)).max(s.d(3)),
            bottom: pos + thumb,
        };
        FillRect(dc, &rect, brush);
        DeleteObject(brush);
        return 1;
    }
    if id == 1007 {
        s.hero_draws.set(s.hero_draws.get() + 1);
        let buffered = s
            .hero_buffer
            .ensure(dc, (r.right - r.left, r.bottom - r.top));
        let target = if buffered { s.hero_buffer.dc } else { dc };
        let saved = SaveDC(target);
        SetBrushOrgEx(target, -origin.0, -origin.1, null_mut());
        surface_fill(s, target, &r, BG);
        let w = (r.right - r.left) * 96 / s.dpi;
        let h = (r.bottom - r.top) * 96 / s.dpi;
        let phase = s.phase();
        let art_height = s.home_layout().hero_art_height;
        let art_saved = SaveDC(target);
        if s.locale.design.effects && draw.itemState & ODS_SELECTED != 0 {
            OffsetViewportOrgEx(target, 0, s.d(2), null_mut());
        }
        draw_hero_mushroom(s, target, w, art_height);
        RestoreDC(target, art_saved);
        let color = if draw.itemState & ODS_DISABLED != 0 {
            MUTED
        } else if phase == "recovery" {
            DANGER
        } else if s.hero_hover || draw.itemState & (ODS_FOCUS | ODS_SELECTED) != 0 {
            TEAL
        } else {
            INK
        };
        draw_hero_caption(s, target, w, h - 40, color);
        RestoreDC(target, saved);
        if buffered {
            // The button's native update region clips this one transfer.
            // No cleared background or half-drawn mushroom reaches it.
            BitBlt(
                dc,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                target,
                r.left,
                r.top,
                SRCCOPY,
            );
            s.hero_presentations.set(s.hero_presentations.get() + 1);
        }
        return 1;
    }
    if id == 3301 {
        surface_fill(s, dc, &r, PANEL);
        if draw.itemState & ODS_SELECTED != 0 {
            pixel_panel(
                s,
                dc,
                RECT {
                    left: r.left + s.d(2),
                    top: r.top + s.d(1),
                    right: r.right - s.d(2),
                    bottom: r.bottom - s.d(1),
                },
                PANEL_ALT,
                PANEL_ALT,
            );
        }
        if let Some(p) = s.draft["presets"]
            .as_array()
            .and_then(|a| a.get(draw.itemID as usize))
        {
            let y = r.top * 96 / s.dpi;
            let width = (r.right - r.left) * 96 / s.dpi;
            rounded(s, dc, 12, y + 12, 20, 20, PANEL, LINE);
            if p["enabled"] == true {
                icon(s, dc, 4, 14, y + 14, 16, TEAL);
            }
            text_raw(
                s,
                dc,
                p["name"].as_str().unwrap_or(""),
                46,
                y + 12,
                width - 60,
                24,
                0,
                if p["enabled"] == true { INK } else { MUTED },
            );
        }
        return 1;
    }
    if id == 1001 && draw.itemID == u32::MAX {
        surface_fill(s, dc, &r, PANEL);
        return 1;
    }
    if id == 1001 && draw.itemID != u32::MAX {
        let selected = draw.itemState & ODS_SELECTED != 0;
        surface_fill(s, dc, &r, PANEL);
        if selected {
            pixel_panel(
                s,
                dc,
                RECT {
                    left: r.left + s.d(2),
                    top: r.top + s.d(1),
                    right: r.right - s.d(2),
                    bottom: r.bottom - s.d(1),
                },
                PANEL_ALT,
                PANEL_ALT,
            );
        }
        if let Some(item) = s.model["profiles"]
            .as_array()
            .and_then(|a| a.get(draw.itemID as usize))
        {
            let y = r.top * 96 / s.dpi;
            let width = (r.right - r.left) * 96 / s.dpi;
            let selected_color = if selected { TEAL } else { MUTED };
            icon(s, dc, 8, 16, y + 10, 24, selected_color);
            let protocol = profile_protocol(item);
            let protocol_width = if protocol.is_empty() {
                0
            } else {
                measured_width(s, dc, protocol, 3).min(100) + 12
            };
            text_raw(
                s,
                dc,
                item["name"].as_str().unwrap_or("Сервер"),
                52,
                y + 12,
                width - 94 - protocol_width,
                24,
                0,
                INK,
            );
            if protocol_width > 0 {
                text_raw(
                    s,
                    dc,
                    protocol,
                    width - 42 - protocol_width,
                    y + 16,
                    protocol_width - 12,
                    18,
                    3,
                    MUTED,
                );
            }
            if selected {
                icon(s, dc, 4, width - 28, y + 12, 16, TEAL);
            }
        }
    } else {
        let nav = (200..204).contains(&id);
        let chosen = nav && id - 200 == s.page as i32;
        let disabled = draw.itemState & ODS_DISABLED != 0;
        let focus = draw.itemState & ODS_FOCUS != 0;
        let pressed = draw.itemState & ODS_SELECTED != 0;
        let w = (r.right - r.left) * 96 / s.dpi;
        let h = (r.bottom - r.top) * 96 / s.dpi;
        surface_fill(s, dc, &r, BG);
        if id == 1011 {
            if pressed || focus {
                rounded(s, dc, 0, 0, w, h, PANEL_ALT, PANEL_ALT);
            }
            for x in [5, 12, 19] {
                rounded(s, dc, x, 12, 3, 3, TEAL, TEAL);
            }
        } else if id == 204 || id == 205 {
            if pressed || focus {
                rounded(s, dc, 0, 0, w, h, PANEL_ALT, PANEL_ALT);
            }
            caption_glyph(
                dc,
                &r,
                s.dpi,
                s.locale.design.interface_size,
                id == 205,
                s.textures
                    .color(if id == 205 && pressed { DANGER } else { MUTED }),
            );
        } else if nav {
            let hover = s.nav_hover == Some(id);
            if hover || focus || pressed {
                // Keep the continuous field visible through navigation.
                let bounds = RECT {
                    left: s.d(8),
                    top: s.d(6),
                    right: r.right - s.d(8),
                    bottom: r.bottom - s.d(6),
                };
                let shape = if s.locale.design.effects && s.locale.design.rounding {
                    crate::design::region(bounds, crate::design::corner_unit(s.dpi))
                } else {
                    CreateRectRgn(bounds.left, bounds.top, bounds.right, bounds.bottom)
                };
                FrameRgn(
                    dc,
                    shape,
                    s.textures.brush(if focus || pressed { TEAL } else { LINE }),
                    s.d(1).max(1),
                    s.d(1).max(1),
                );
                DeleteObject(shape);
            }
            let c = if chosen {
                TEAL
            } else if hover || focus {
                INK
            } else {
                MUTED
            };
            icon(s, dc, (id - 200) as usize, (w - 24) / 2, 14, 24, c);
            centered(s, dc, PAGES[(id - 200) as usize], 0, 50, w, 20, 3, c);
        } else if s.checks.contains_key(&id) {
            rounded(
                s,
                dc,
                0,
                0,
                w,
                h,
                PANEL_ALT,
                if focus { TEAL } else { PANEL_ALT },
            );
            let on = s.checked(id);
            let c = if disabled {
                LINE
            } else if on {
                TEAL
            } else {
                LINE
            };
            rounded(
                s,
                dc,
                w - 34,
                9,
                24,
                24,
                if on { c } else { PANEL },
                if on { c } else { LINE },
            );
            if on {
                icon(s, dc, 4, w - 31, 12, 18, BG);
            }
            text(
                s,
                dc,
                &s.text(id),
                10,
                12,
                w - 56,
                27,
                0,
                if disabled { MUTED } else { INK },
            );
        } else {
            let category = (2200..2206).contains(&id);
            let selected_category = ((4060..=4062).contains(&id)
                && InterfaceSize::ALL[(id - 4060) as usize] == s.locale.design.interface_size)
                || (category && id - 2200 == s.category as i32)
                || (id
                    == match s.locale.language() {
                        Language::Auto => 4010,
                        Language::Russian => 4011,
                        Language::English => 4012,
                    });
            let accent = [1002, 1021, 2104, 3012].contains(&id);
            let bg = if disabled {
                PANEL
            } else if selected_category {
                PANEL_ALT
            } else if pressed {
                LINE
            } else if accent {
                INDIGO
            } else {
                PANEL_ALT
            };
            rounded(
                s,
                dc,
                0,
                0,
                w,
                h,
                bg,
                if focus || selected_category { TEAL } else { bg },
            );
            centered(
                s,
                dc,
                &s.text(id),
                6,
                0,
                w - 12,
                h,
                0,
                if disabled {
                    MUTED
                } else if selected_category {
                    TEAL
                } else if id == 1005 {
                    DANGER
                } else if accent && !pressed {
                    PRIMARY_INK
                } else {
                    INK
                },
            );
        }
    }
    1
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if message == WM_NCCREATE {
        let c = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, c.lpCreateParams as isize);
        return 1;
    }
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let s = &mut *pointer;
    s.hwnd = hwnd;
    match message {
        BACKGROUND_ACTIVITY => {
            s.sync_background();
            0
        }
        WM_ACTIVATEAPP => {
            s.app_active = wparam != 0;
            s.sync_pulse();
            0
        }
        WM_ENTERSIZEMOVE | WM_EXITSIZEMOVE => {
            s.resizing = message == WM_ENTERSIZEMOVE;
            s.sync_pulse();
            0
        }
        ACTIVATE => {
            if !s.smoke
                && let Some(exe) = &s.autostart_executable
                && let Ok(enabled) = crate::autostart::enabled(exe)
            {
                s.autostart_enabled = enabled;
                s.check(4002, enabled);
            }
            s.hidden = false;
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            if !s.smoke {
                SetForegroundWindow(hwnd);
            }
            if !s.smoke {
                SetTimer(hwnd, 1, 3000, None);
                s.status();
            }
            s.sync_pulse();
            0
        }
        WM_LBUTTONDOWN => {
            let (width, visible, content) = s.dimensions();
            let x = (lparam as u32 & 0xffff) as i16 as i32 * 96 / s.dpi;
            let y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32 * 96 / s.dpi;
            if x >= width - 16
                && (88..visible - NAV_HEIGHT - NAV_GAP).contains(&y)
                && content > visible - NAV_HEIGHT - NAV_GAP
            {
                let track = page_scroll_track(visible, content, s.scroll);
                let on_thumb = (track.position..track.position + track.thumb).contains(&y);
                s.scroll_drag_offset = if on_thumb {
                    y - track.position
                } else {
                    track.thumb / 2
                };
                s.scroll_dragging = true;
                s.sync_background();
                SetCapture(hwnd);
                if !on_thumb {
                    s.scroll = track.scroll_at(y, s.scroll_drag_offset);
                    s.layout();
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            0
        }
        WM_MOUSEMOVE if s.scroll_dragging => {
            let (_, visible, content) = s.dimensions();
            let y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32 * 96 / s.dpi;
            let next =
                page_scroll_track(visible, content, s.scroll).scroll_at(y, s.scroll_drag_offset);
            if s.scroll != next {
                s.scroll = next;
                s.layout();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP if s.scroll_dragging => {
            s.scroll_dragging = false;
            s.sync_background();
            ReleaseCapture();
            0
        }
        WM_CAPTURECHANGED => {
            s.scroll_dragging = false;
            s.sync_background();
            0
        }
        WM_NCCALCSIZE => 0,
        WM_NCPAINT if IsIconic(hwnd) == 0 => 0,
        WM_NCACTIVATE => DefWindowProcW(
            hwnd,
            message,
            wparam,
            if IsIconic(hwnd) != 0 { lparam } else { -1 },
        ),
        WM_DWMCOMPOSITIONCHANGED => {
            configure_custom_chrome(hwnd);
            0
        }
        WM_NCHITTEST => {
            let mut rect = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            let x = (lparam as u32 & 0xffff) as i16 as i32 - rect.left;
            let y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32 - rect.top;
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let edge = s.d(7);
            if IsZoomed(hwnd) == 0 {
                let hit = if y < edge {
                    HTTOP
                } else if y >= height - edge {
                    HTBOTTOM
                } else {
                    HTCLIENT
                };
                if hit != HTCLIENT {
                    return hit as isize;
                }
            }
            if y < s.d(88) && x < width - s.d(100) {
                HTCAPTION as isize
            } else {
                HTCLIENT as isize
            }
        }
        WM_NCLBUTTONDBLCLK if wparam == HTCAPTION as usize => 0,
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_MAXIMIZE as usize => 0,
        WM_WINDOWPOSCHANGING => {
            let position = &mut *(lparam as *mut WINDOWPOS);
            if position.flags & SWP_NOSIZE == 0 && IsIconic(hwnd) == 0 {
                position.cx = s.d(HOME_WIDTH);
                let mut current: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut current);
                let x = if position.flags & SWP_NOMOVE != 0 {
                    current.left
                } else {
                    position.x
                };
                let y = if position.flags & SWP_NOMOVE != 0 {
                    current.top
                } else {
                    position.y
                };
                let proposed = RECT {
                    left: x,
                    top: y,
                    right: x + position.cx,
                    bottom: y + position.cy,
                };
                position.cy = position.cy.max(s.minimum_height_for_rect(&proposed));
            }
            0
        }
        WM_PAINT => {
            let mut p = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut p);
            let mut rect = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            s.textures
                .prepare_background(rect.right, rect.bottom, s.background_millis);
            let target = if s.ensure_buffer(dc, (rect.right, rect.bottom)) {
                s.buffer_dc
            } else {
                dc
            };
            let saved = SaveDC(target);
            // A fully occluded/offscreen window can have queued damage but an
            // empty BeginPaint rectangle. Keep its reusable buffer valid for
            // PrintWindow and the next exposure instead of retaining old pixels.
            let damage = if IsRectEmpty(&p.rcPaint) != 0 {
                rect
            } else {
                p.rcPaint
            };
            IntersectClipRect(target, damage.left, damage.top, damage.right, damage.bottom);
            // Clear in screen coordinates before applying the content scroll.
            // Otherwise a scrolled page leaves the lower backbuffer unchanged
            // and pixels from another page show through its empty spaces.
            SetViewportOrgEx(target, 0, 0, null_mut());
            SetBrushOrgEx(target, 0, 0, null_mut());
            surface_fill(s, target, &rect, BG);
            SetViewportOrgEx(target, 0, -s.d(s.scroll), null_mut());
            s.paint(target);
            RestoreDC(target, saved);
            if target != dc {
                BitBlt(dc, 0, 0, rect.right, rect.bottom, target, 0, 0, SRCCOPY);
            }
            s.parent_paints.set(s.parent_paints.get() + 1);
            EndPaint(hwnd, &p);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            if wparam as u32 == SIZE_MINIMIZED {
                s.minimized = true;
                s.sync_pulse();
                if s.pending_id.is_none() && !s.startup_pending {
                    KillTimer(hwnd, 1);
                }
                return 0;
            }
            let was_minimized = s.minimized;
            s.minimized = false;
            if was_minimized && !s.hidden && !s.smoke {
                SetTimer(hwnd, 1, 3000, None);
                s.status();
            }
            let mut r = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            crate::design::window_shape(
                hwnd,
                r.right,
                r.bottom,
                s.dpi,
                s.locale.design.effects && s.locale.design.rounding,
            );
            s.scroll = s
                .scroll
                .min((s.dimensions().2 - r.bottom * 96 / s.dpi + NAV_HEIGHT + NAV_GAP).max(0));
            s.layout();
            s.prepare_background();
            s.sync_pulse();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam as *const RECT);
            s.set_dpi_fonts_at((wparam & 0xffff) as i32, Some(suggested));
            s.scroll = 0;
            let work = s.work_area_for_rect(suggested);
            let height =
                (suggested.bottom - suggested.top).max(s.minimum_height_for_rect(suggested));
            let height = if s.smoke {
                height
            } else {
                height.min((work.bottom - work.top - s.d(16)).max(240))
            };
            SetWindowPos(
                hwnd,
                null_mut(),
                suggested.left,
                if s.smoke {
                    suggested.top
                } else {
                    suggested
                        .top
                        .clamp(work.top, (work.bottom - height).max(work.top))
                },
                s.d(HOME_WIDTH),
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            s.layout();
            InvalidateRect(hwnd, null(), 1);
            0
        }
        RETIRE => {
            DestroyWindow(hwnd);
            0
        }
        SCROLL_DRAG => {
            let bar = (wparam & 0xffff) as i32;
            let event = (wparam >> 16) as u32;
            if event == 2 {
                s.scrollbar_drag = None;
            } else {
                s.drag_scrollbar(bar, lparam as i32, event == 0);
            }
            s.sync_background();
            0
        }
        LIST_SCROLL => {
            s.pause_background_input();
            s.scroll_input(
                if lparam == 0 { 1001 } else { lparam as i32 },
                (wparam >> 16) as i16 as i32,
            );
            0
        }
        WM_MOUSEWHEEL
            if s.page == 0
                && GetWindowLongPtrW(s.controls[&1001], GWL_STYLE) as u32 & WS_VISIBLE != 0
                && {
                    let mut r = std::mem::zeroed();
                    GetWindowRect(s.controls[&1001], &mut r);
                    let x = (lparam as u32 & 0xffff) as i16 as i32;
                    let y = (lparam as u32 >> 16) as i16 as i32;
                    x >= r.left && x < r.right && y >= r.top && y < r.bottom
                } =>
        {
            s.pause_background_input();
            s.scroll_profiles((wparam >> 16) as i16 as i32);
            0
        }
        WM_VSCROLL | WM_MOUSEWHEEL => {
            s.pause_background_input();
            let mut r = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let visible = r.bottom * 96 / s.dpi - NAV_HEIGHT - NAV_GAP;
            let next = if message == WM_MOUSEWHEEL {
                s.scroll - ((wparam >> 16) as i16 as i32 / 120) * 64
            } else {
                match (wparam & 0xffff) as i32 {
                    SB_LINEUP => s.scroll - 24,
                    SB_LINEDOWN => s.scroll + 24,
                    SB_PAGEUP => s.scroll - visible / 2,
                    SB_PAGEDOWN => s.scroll + visible / 2,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut info: SCROLLINFO = std::mem::zeroed();
                        info.cbSize = std::mem::size_of_val(&info) as u32;
                        info.fMask = SIF_TRACKPOS;
                        GetScrollInfo(hwnd, SB_VERT, &mut info);
                        info.nTrackPos
                    }
                    SB_TOP => 0,
                    SB_BOTTOM => s.dimensions().2 - visible,
                    _ => s.scroll,
                }
            };
            let next = next.clamp(0, (s.dimensions().2 - visible).max(0));
            if s.scroll != next {
                s.scroll = next;
                s.layout();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_HSCROLL
            if lparam == s.controls.get(&4024).copied().unwrap_or(null_mut()) as isize
                && lparam != 0 =>
        {
            if IsWindowEnabled(lparam as HWND) != 0 {
                if s.background_running {
                    s.advance_background();
                }
                let speed = (SendMessageW(lparam as HWND, TBM_GETPOS, 0, 0) as u32).clamp(25, 200);
                s.background_speed_preview = Some(speed);
                InvalidateRect(lparam as HWND, null(), 0);
                InvalidateRect(s.controls[&4025], null(), 0);
                if wparam as u32 & 0xffff == TB_ENDTRACK {
                    s.save_background_speed();
                } else {
                    // Preview every drag position without disk I/O on each move.
                    SetTimer(hwnd, BACKGROUND_SPEED_SAVE_TIMER, 500, None);
                }
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            SetTextColor(wparam as HDC, s.textures.color(INK));
            SetBkMode(wparam as HDC, TRANSPARENT as i32);
            s.white as isize
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let color = if message == WM_CTLCOLORLISTBOX
                && matches!(GetDlgCtrlID(lparam as HWND), 1001 | 3301)
            {
                PANEL
            } else {
                PANEL_ALT
            };
            SetTextColor(wparam as HDC, s.textures.color(INK));
            SetBkColor(wparam as HDC, s.textures.color(color));
            SetBrushOrgEx(wparam as HDC, 0, 0, null_mut());
            s.textures.brush(color) as isize
        }
        WM_MEASUREITEM => {
            let item = &mut *(lparam as *mut MEASUREITEMSTRUCT);
            if item.CtlType == ODT_MENU {
                let dc = GetDC(hwnd);
                item.itemWidth =
                    s.d(measured_width(s, dc, &menu_label(item.itemData), 0) + 28) as u32;
                item.itemHeight = s.d(38) as u32;
                ReleaseDC(hwnd, dc);
                return 1;
            }
            item.itemHeight = s.d(if item.CtlType == ODT_COMBOBOX { 34 } else { 44 }) as u32;
            1
        }
        WM_DRAWITEM => paint_draw_item(s, &*(lparam as *const DRAWITEMSTRUCT)),
        WM_TIMER => {
            if wparam == BACKGROUND_SPEED_SAVE_TIMER {
                s.save_background_speed();
                return 0;
            }
            if wparam == BACKGROUND_RESUME_TIMER {
                KillTimer(hwnd, BACKGROUND_RESUME_TIMER);
                s.background_input_pause = false;
                s.sync_background();
                return 0;
            }
            if wparam == BACKGROUND_TIMER {
                s.sync_background();
                if s.background_running {
                    s.advance_background();
                    s.prepare_background();
                    let _frame = CompositedFrame::begin(hwnd);
                    s.invalidate_background();
                    // Finish this composited frame as one paint transaction,
                    // before a new timer can publish a different background.
                    // Clean native editors/lists are not invalidated here.
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_UPDATENOW | RDW_ALLCHILDREN | RDW_NOERASE,
                    );
                }
                return 0;
            }
            if wparam == PULSE_TIMER {
                s.sync_pulse();
                if s.pulse_running {
                    s.pulse_frame = (s.pulse_frame + 1) % PULSE_FRAMES;
                    s.hero_motion.advance(PULSE_INTERVAL);
                    s.pulse_ticks.set(s.pulse_ticks.get() + 1);
                    // Art, status and dots share one buffer. Never repaint the
                    // parent or rebuild layout for an animation frame.
                    InvalidateRect(s.controls[&1007], null(), 0);
                    s.sync_pulse();
                }
                return 0;
            }
            if wparam == 1
                && (s.startup_pending
                    || s.pending_id.is_some()
                    || (!s.hidden && IsIconic(hwnd) == 0))
            {
                if s.startup_pending && s.startup_since.elapsed().as_secs() >= 180 {
                    s.startup_pending = false;
                }
                if s.pending_since.is_some_and(|started| {
                    // Explicit recovery may include synchronous SCM startup;
                    // other actions retain their original receipt deadline.
                    let timeout = if matches!(s.pending_action.as_str(), "Disconnect" | "Exit") {
                        180
                    } else {
                        115
                    };
                    started.elapsed().as_secs() > timeout
                }) {
                    // A stopped controller and missing local intent cannot
                    // prove persistent WFP policy was removed. Only the bound
                    // terminal Exit receipt may finish a successful shutdown.
                    s.busy = false;
                    s.pending_id = None;
                    s.pending_since = None;
                    s.next = None;
                    s.model["busy"] = json!(false);
                    s.model["phase"] = json!("recovery");
                    s.notice="Нет подтверждения состояния VPN. Нажми «Вернуть сеть» для повторного отключения.".into();
                    s.model["last_error"] = json!(s.notice);
                    if s.exit_pending {
                        s.finish_exit(false);
                    }
                    s.update_buttons();
                    InvalidateRect(hwnd, null(), 0);
                    if !s.startup_pending && (s.hidden || IsIconic(hwnd) != 0) {
                        // Every receipt may have been lost. With no pending
                        // operation left, the hidden UI has no idle work.
                        KillTimer(hwnd, 1);
                        return 0;
                    }
                }
                s.status();
            }
            0
        }
        REPLY => {
            let answer = Box::from_raw(lparam as *mut Answer);
            let status = answer.action == "Status";
            if status {
                s.status_pending = false;
            }
            if answer.generation != s.generation {
                return 0;
            }
            if !status && answer.request_id.is_some() && answer.request_id != s.pending_id {
                return 0;
            }
            let before = s.visible_state();
            let mut advance = false;
            let exiting = s.exit_pending && s.pending_action == "Exit";
            let mut completed = None;
            // A mutation reply can overtake a poll. Never reuse a token for a
            // different local model, including failed operations and reconnects.
            if !status {
                s.status_revision = None;
            }
            match answer.response {
                Ok(reply) if reply["ok"] == true => {
                    if status && reply["not_modified"] == true {
                        if !s.loaded
                            || !s.status_revision.as_deref().is_some_and(|revision| {
                                reply["revision"].as_str() == Some(revision)
                            })
                        {
                            s.status_revision = None;
                            s.status();
                        }
                        return 0;
                    }
                    if status {
                        s.status_revision = reply["revision"].as_str().map(str::to_owned);
                    }
                    if status && s.loaded && s.model == reply["data"] && s.pending_id.is_none() {
                        return 0;
                    }
                    let initial = !s.loaded;
                    let terminal = if status {
                        s.settle_operation(&reply["data"])
                    } else {
                        s.busy = false;
                        s.pending_id = None;
                        s.pending_since = None;
                        Some(true)
                    };
                    s.sync_snapshot(reply["data"].clone(), &answer.action);
                    if answer.action == "ValidateSettings" {
                        s.show_rule_issues(&reply["data"]["rule_issues"]);
                    }
                    if terminal == Some(true)
                        && (answer.action == "Import" || (status && s.pending_action == "Import"))
                    {
                        s.imported_count = reply["data"]["imported_count"]
                            .as_u64()
                            .or_else(|| reply["data"]["last_operation"]["imported_count"].as_u64());
                        s.close_import();
                    }
                    if !status || initial || terminal == Some(true) {
                        s.notice = match if status && terminal.is_some() {
                            s.pending_action.as_str()
                        } else {
                            answer.action.as_str()
                        } {
                            "Import" => "Конфигурация добавлена",
                            "ApplySettings" => "Изменения сохранены",
                            "Update" => "Списки и IP обновлены",
                            "Diagnose" => "Проверка завершена. Результаты в журналах.",
                            _ => "",
                        }
                        .into();
                        if let Some(warning) = reply["data"]["connection_history_warning"].as_str()
                        {
                            s.notice = warning.into();
                        }
                    }
                    advance = terminal == Some(true);
                    completed = terminal;
                }
                Ok(reply) => {
                    s.status_revision = None;
                    s.busy = false;
                    s.pending_id = None;
                    s.pending_since = None;
                    s.next = None;
                    if reply["data"].is_object() {
                        s.sync_snapshot(reply["data"].clone(), "Status");
                    }
                    s.model["busy"] = json!(false);
                    if matches!(
                        s.model["phase"].as_str(),
                        Some("connecting" | "disconnecting")
                    ) {
                        s.model["phase"] = json!(if s.model["running"] == true {
                            "connected"
                        } else if s.model["dns_recovery_pending"] == true {
                            "recovery"
                        } else {
                            "idle"
                        });
                    }
                    s.notice = reply["error"]
                        .as_str()
                        .unwrap_or("Не удалось выполнить действие")
                        .to_owned();
                    if answer.action == "Autoconnect" {
                        s.check(4001, s.model["autoconnect"] == true);
                    }
                    completed = Some(false);
                }
                Err(error) => {
                    s.status_revision = None;
                    if status {
                        s.notice = "Нет ответа от Mukhomor. Повторяем проверку состояния…".into();
                    } else {
                        // A transport timeout is not proof that the VPN operation
                        // failed. Its terminal cache record settles this request.
                        s.notice = format!("{} · проверяем состояние", error);
                        s.status();
                    }
                }
            }
            if exiting && let Some(ok) = completed {
                s.finish_exit(ok);
                if ok {
                    return 0;
                }
            }
            if advance && let Some(next) = s.next.take() {
                s.request(next);
            }
            if (s.hidden || IsIconic(hwnd) != 0) && s.pending_id.is_none() && !s.startup_pending {
                KillTimer(hwnd, 1);
            }
            if s.startup_pending {
                PostMessageW(hwnd, STARTUP_CONNECT, 0, 0);
            }
            s.update_buttons();
            s.layout();
            if before != s.visible_state() {
                InvalidateRect(hwnd, null(), 0);
                InvalidateRect(s.controls[&1007], null(), 0);
            }
            if s.loaded
                && s.model["service_ready"] == true
                && let Some(probe) = s.startup_probe.take()
            {
                UpdateWindow(hwnd);
                let report = json!({"schema":1,"window_presented_ms":probe.window_ready_ms,
                    "controls_presented_ms":probe.started.elapsed().as_secs_f64()*1000.0,
                    "service_ready":true,"status_uncertain":s.model["status_uncertain"] == true,
                    "scope":"QA UI process entry to first window and first controller-backed controls; installation verification excluded"});
                let _ = std::fs::write(
                    s.root.join("native-startup-performance.json"),
                    report.to_string(),
                );
                DestroyWindow(hwnd);
            }
            0
        }
        STARTUP_CONNECT => {
            if s.startup_pending && s.model["service_ready"] == true {
                s.startup_pending = false;
                if s.model["autoconnect"] == true
                    && s.phase() == "idle"
                    && !s.busy
                    && s.pending_id.is_none()
                    && s.model["status_uncertain"] != true
                    && (s.model["last_error"].is_null()
                        || s.model["last_error"].as_str() == Some(""))
                {
                    s.request(json!({"action":"Connect","startup":true}));
                } else if s.hidden && s.pending_id.is_none() {
                    KillTimer(hwnd, 1);
                }
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xffff) as i32;
            let event = ((wparam >> 16) & 0xffff) as u32;
            if s.appearance_command(id, event) {
                return 0;
            }
            if s.preset_command(id, event) {
                return 0;
            }
            if ([2102, 3006].contains(&id) && [EN_CHANGE, EN_VSCROLL].contains(&event))
                || (id == 1020 && event == EN_VSCROLL)
            {
                InvalidateRect(s.controls[&scrollbar_id(id)], null(), 0);
                return 0;
            }
            if (200..204).contains(&id) {
                if !s.collect() {
                    return 0;
                }
                let old_page = s.page;
                s.page = (id - 200) as usize;
                s.populate_presets();
                InvalidateRect(s.controls[&(200 + old_page as i32)], null(), 0);
                InvalidateRect(s.controls[&id], null(), 0);
                s.scroll = 0;
                s.layout();
                s.sync_pulse();
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            match id {
                4015 | 4016 => {
                    if !s.collect() {
                        return 0;
                    }
                    s.dns_settings_open = id == 4015;
                    s.scroll = 0;
                    s.layout_key.set(None);
                    s.populate_settings();
                    s.layout();
                    InvalidateRect(hwnd, null(), 0);
                }
                4020 | 4021 | 4023 => {
                    let mut design = s.locale.design;
                    match id {
                        4020 => design.effects = !design.effects,
                        4021 => design.animation = !design.animation,
                        _ => design.rounding = !design.rounding,
                    }
                    if let Err(error) = s.locale.set_design(design) {
                        s.notice = error;
                        InvalidateRect(hwnd, null(), 0);
                    } else {
                        s.refresh_design();
                    }
                }
                IDCANCEL if s.import_open && !s.busy => {
                    s.close_import();
                    s.layout();
                    s.update_buttons();
                    InvalidateRect(hwnd, null(), 0);
                }
                204 => {
                    ShowWindow(hwnd, SW_MINIMIZE);
                }
                205 => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                1001 if event == LBN_SELCHANGE => {
                    if let Some(id) = s.profile_id()
                        && let Some(item) = s.model["profiles"]
                            .as_array()
                            .and_then(|a| a.iter().find(|p| p["id"] == id))
                    {
                        s.set(1006, item["name"].as_str().unwrap_or(""));
                    }
                    s.update_buttons();
                    if let Some(id) = s.profile_id()
                        && s.model["selected"] != id
                    {
                        s.rename_open = false;
                        s.request(json!({"action":"Select","id":id}));
                    }
                }
                1002 => {
                    if !s.smoke {
                        let menu = CreatePopupMenu();
                        for (id, caption) in
                            [(1030, "Из файла…"), (1031, "Вставить ссылку или текст…")]
                        {
                            AppendMenuW(
                                menu,
                                MF_STRING,
                                id,
                                wide(&s.locale.text(caption)).as_ptr(),
                            );
                        }
                        let mut bounds: RECT = std::mem::zeroed();
                        GetWindowRect(s.controls[&1002], &mut bounds);
                        let action = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON,
                            bounds.left,
                            bounds.bottom,
                            0,
                            hwnd,
                            null(),
                        );
                        DestroyMenu(menu);
                        if action != 0 {
                            PostMessageW(hwnd, WM_COMMAND, action as usize, 0);
                        }
                    }
                }
                1030 => {
                    if !s.smoke
                        && let Some(path) = choice_file(s, false)
                    {
                        match read_import_content(&path) {
                            Ok(content) => s.submit_import(
                                content,
                                &path.file_stem().unwrap_or_default().to_string_lossy(),
                                &path.file_name().unwrap_or_default().to_string_lossy(),
                            ),
                            Err(error) => s.import_error = Some(error),
                        }
                        s.layout();
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                1031 => {
                    s.import_open = true;
                    s.import_error = (s.text(1020).len() > IMPORT_LIMIT)
                        .then(|| "Конфигурация слишком большая (максимум 512 КБ)".into());
                    s.imported_count = None;
                    s.rename_open = false;
                    s.scroll = 0;
                    s.layout();
                    s.update_buttons();
                    InvalidateRect(hwnd, null(), 0);
                    SetFocus(s.controls[&1020]);
                }
                1020 if event == EN_CHANGE => {
                    InvalidateRect(s.controls[&1024], null(), 0);
                    if s.text(1020).len() > IMPORT_LIMIT {
                        s.import_error =
                            Some("Конфигурация слишком большая (максимум 512 КБ)".into());
                    } else {
                        s.import_error = None;
                    }
                    s.update_buttons();
                    s.layout();
                    InvalidateRect(hwnd, null(), 0);
                }
                1021 => {
                    let content = s.text(1020);
                    let name = s.locale.text("Новая конфигурация").into_owned();
                    s.submit_import(content, &name, "pasted-text");
                }
                1022 => {
                    if !s.busy {
                        s.close_import();
                        s.layout();
                        s.update_buttons();
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                1003 => {
                    if let Some(id) = s.profile_id() {
                        s.request(json!({"action":"Select","id":id}));
                    }
                }
                1004 => {
                    let (menu, _labels) =
                        pixel_menu(s, &[(1008, "Переименовать"), (1005, "Удалить профиль")]);
                    let mut point = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut point);
                    let action = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        0,
                        hwnd,
                        null(),
                    );
                    DestroyMenu(menu);
                    if action != 0 {
                        PostMessageW(hwnd, WM_COMMAND, action as usize, 0);
                    }
                }
                1008 => {
                    s.rename_open = !s.rename_open;
                    s.layout();
                    InvalidateRect(hwnd, null(), 0);
                    SetFocus(s.controls[&1006]);
                }
                1009 => {
                    if let Some(id) = s.profile_id() {
                        s.rename_open = false;
                        s.request(json!({"action":"Rename","id":id,"name":s.text(1006)}));
                    }
                }
                1005 => {
                    if let Some(id) = s.profile_id()
                        && MessageBoxW(
                            hwnd,
                            wide(
                                &s.locale
                                    .text("Удалить выбранный профиль с этого компьютера?"),
                            )
                            .as_ptr(),
                            wide(&s.locale.text("Удаление сервера")).as_ptr(),
                            MB_YESNO | MB_ICONQUESTION,
                        ) == IDYES
                    {
                        s.request(json!({"action":"Remove","id":id}));
                    }
                }
                1007 => {
                    if s.can_disconnect() {
                        s.request(json!({"action":"Disconnect"}));
                    } else if let Some(id) = s.profile_id() {
                        if s.model["selected"] != id {
                            s.next = Some(json!({"action":"Connect"}));
                            s.request(json!({"action":"Select","id":id}));
                        } else {
                            s.request(json!({"action":"Connect"}));
                        }
                    } else {
                        MessageBoxW(
                            hwnd,
                            wide(
                                &s.locale
                                    .text("Сначала добавь конфигурацию и выбери сервер."),
                            )
                            .as_ptr(),
                            wide(&s.locale.text("Нужен сервер")).as_ptr(),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                1011 => {
                    if !s.smoke
                        && let Some(error) = s.connection_error()
                    {
                        MessageBoxW(
                            hwnd,
                            wide(&s.locale.error(error)).as_ptr(),
                            wide(&s.locale.text("Подробности ошибки")).as_ptr(),
                            MB_OK | MB_ICONERROR,
                        );
                    }
                }
                2200..=2205 => {
                    if !s.collect() {
                        return 0;
                    }
                    let old = s.category;
                    s.category = (id - 2200) as usize;
                    InvalidateRect(s.controls[&(2200 + old as i32)], null(), 0);
                    InvalidateRect(s.controls[&id], null(), 0);
                    s.populate();
                }
                2103 => {
                    if let Some(path) = choice_file(s, true) {
                        let v = if s.category == 0 {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        } else {
                            path.to_string_lossy().into_owned()
                        };
                        s.set(
                            2102,
                            format!("{}\r\n{}", s.text(2102).trim_end(), v).trim_start(),
                        );
                    }
                }
                2104 | 3012 => {
                    if s.dns_visible()
                        && [3007, 3008, 3009, 3013, 3014]
                            .iter()
                            .any(|&id| s.text(id).parse::<u32>().is_err())
                    {
                        s.notice = "Интервалы DNS должны быть целыми числами в секундах".into();
                        MessageBoxW(
                            hwnd,
                            wide(&s.locale.error(&s.notice)).as_ptr(),
                            wide(&s.locale.text("Проверь интервалы")).as_ptr(),
                            MB_OK | MB_ICONINFORMATION,
                        );
                        return 0;
                    }
                    if !s.collect() {
                        return 0;
                    }
                    s.request(json!({"action":"ApplySettings","settings":s.draft}));
                }
                3011 => s.request(json!({"action":"Update"})),
                3004..=3005 => {
                    s.check(id, !s.checked(id));
                }
                4001 => {
                    s.check(4001, !s.checked(4001));
                    s.request(json!({"action":"Autoconnect","enabled":s.checked(4001)}));
                }
                4002 => {
                    let enabled = !s.autostart_enabled;
                    let result = if s.smoke {
                        Ok(())
                    } else {
                        s.autostart_executable
                            .as_ref()
                            .ok_or_else(|| "Не удалось найти установленное приложение".to_owned())
                            .and_then(|exe| crate::autostart::set(exe, enabled))
                    };
                    match result {
                        Ok(()) => {
                            s.autostart_enabled = enabled;
                            s.check(4002, enabled);
                        }
                        Err(error) => {
                            s.notice = error;
                            MessageBoxW(
                                hwnd,
                                wide(&s.locale.error(&s.notice)).as_ptr(),
                                wide(&s.locale.text("Запуск с Windows")).as_ptr(),
                                MB_OK | MB_ICONINFORMATION,
                            );
                        }
                    }
                }
                4003 => s.request(json!({"action":"Diagnose"})),
                4004 => shell_open(s, &s.root.join("runtime").to_string_lossy(), None, "open"),
                4005 => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                4006 | 4007 => {
                    s.check(id, !s.checked(id));
                }
                4008 => {
                    if s.collect() {
                        s.request(json!({"action":"ApplySettings","settings":s.draft}));
                    }
                }
                4010..=4012 => {
                    s.change_language(match id {
                        4010 => Language::Auto,
                        4011 => Language::Russian,
                        _ => Language::English,
                    });
                }
                _ => {}
            }
            0
        }
        TRAY => {
            if lparam as u32 == WM_LBUTTONUP || lparam as u32 == WM_LBUTTONDBLCLK {
                SendMessageW(hwnd, ACTIVATE, 0, 0);
            } else if lparam as u32 == WM_RBUTTONUP {
                let (menu, _labels) = pixel_menu(
                    s,
                    &[
                        (1, "Открыть настройки"),
                        (2, "Отключить VPN"),
                        (3, "Выход из Mukhomor"),
                    ],
                );
                let mut p = POINT { x: 0, y: 0 };
                GetCursorPos(&mut p);
                SetForegroundWindow(hwnd);
                let n = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    p.x,
                    p.y,
                    0,
                    hwnd,
                    null(),
                );
                DestroyMenu(menu);
                if n == 1 {
                    SendMessageW(hwnd, ACTIVATE, 0, 0);
                } else if n == 2 {
                    s.request(json!({"action":"Disconnect"}));
                } else if n == 3 {
                    s.request(json!({"action":"Exit"}));
                }
            }
            0
        }
        WM_CLOSE => {
            if s.smoke {
                s.hidden = true;
                ShowWindow(hwnd, SW_HIDE);
                s.sync_pulse();
            } else {
                s.hidden = true;
                ShowWindow(hwnd, SW_HIDE);
                if s.pending_id.is_none() && !s.startup_pending {
                    KillTimer(hwnd, 1);
                }
            }
            s.sync_pulse();
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION if wparam != 0 => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            s.save_background_speed();
            KillTimer(hwnd, 1);
            KillTimer(hwnd, PULSE_TIMER);
            KillTimer(hwnd, BACKGROUND_TIMER);
            KillTimer(hwnd, BACKGROUND_RESUME_TIMER);
            s.background_running = false;
            s.pulse_running = false;
            tray(s, false);
            PostQuitMessage(0);
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: s.d(HOME_WIDTH),
                y: s.minimum_height(),
            };
            info.ptMaxTrackSize.x = s.d(HOME_WIDTH);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
fn window_identity(root: &std::path::Path) -> Result<String, String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let identity = format!(
        "{}|{}",
        root.to_string_lossy().to_lowercase(),
        crate::platform::user_sid()?
    );
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("Mukhomor.Native.{hash:016x}"))
}
// Held until the message loop exits; only the already installed, unprivileged
// controller uses this guard. Bootstrap and smoke remain independent.
unsafe fn single_instance(class: &str, activate: bool) -> Result<Option<OwnedHandle>, String> {
    SetLastError(ERROR_SUCCESS);
    let mutex = CreateMutexW(null(), 1, wide(&format!("Local\\{class}")).as_ptr());
    if mutex.is_null() {
        return Err("Не удалось открыть окно Mukhomor".into());
    }
    let existed = GetLastError() == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle(mutex);
    if !existed {
        return Ok(Some(mutex));
    }
    for _ in 0..20 {
        let wait = WaitForSingleObject(mutex.0, 0);
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Ok(Some(mutex));
        }
        if !activate {
            return Ok(None);
        }
        let window = FindWindowW(wide(class).as_ptr(), null());
        if !window.is_null() {
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            AllowSetForegroundWindow(pid);
            if PostMessageW(window, ACTIVATE, 0, 0) != 0 {
                return Ok(None);
            }
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("Окно Mukhomor уже открывается. Попробуй ещё раз через несколько секунд.".into())
}
// Measurement mode is reachable exclusively inside --smoke. It pumps the real
// native message queue without service/network operations. An explicit smoke
// flag can measure the real connecting animation; idle still has no timer.
unsafe fn smoke_idle_hold(s: &mut State) -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let duration = args
        .windows(2)
        .find(|a| a[0] == "--smoke-ui-hold-ms")
        .and_then(|a| a[1].parse::<u64>().ok())
        .unwrap_or(0)
        .min(15_000);
    if duration == 0 {
        return Ok(());
    }
    let hidden = args.iter().any(|a| a == "--smoke-ui-hold-hidden");
    let connecting = args.iter().any(|a| a == "--smoke-ui-hold-connecting");
    let original_model = s.model.clone();
    let original_busy = s.busy;
    let original_animation = s.pulse_enabled;
    if connecting {
        s.model["phase"] = json!("connecting");
        s.model["busy"] = json!(true);
        s.model["can_disconnect"] = json!(true);
        s.busy = true;
        s.pulse_enabled = true;
        s.update_buttons();
        UpdateWindow(s.controls[&1007]);
    }
    if hidden {
        s.hidden = true;
        ShowWindow(s.hwnd, SW_HIDE);
    }
    s.sync_pulse();
    UpdateWindow(s.hwnd);
    let started = Instant::now();
    let paints = s.parent_paints.get();
    let layouts = s.layout_passes.get();
    let moves = s.geometry_moves.get();
    let pulse_ticks = s.pulse_ticks.get();
    let hero_draws = s.hero_draws.get();
    std::fs::write(
        s.root.join("smoke-ui-idle-ready.json"),
        serde_json::to_vec_pretty(&json!({
            "pid":std::process::id(),"smoke":true,"hidden":hidden,"connecting":connecting,"hold_ms":duration
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    while started.elapsed().as_millis() < duration as u128 {
        let mut message = std::mem::zeroed();
        while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        MsgWaitForMultipleObjectsEx(0, null(), 100, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
    std::fs::write(
        s.root.join("smoke-ui-idle-result.json"),
        serde_json::to_vec_pretty(&json!({
            "paints":s.parent_paints.get()-paints,"layouts":s.layout_passes.get()-layouts,
            "geometry_moves":s.geometry_moves.get()-moves,
            "pulse_ticks":s.pulse_ticks.get()-pulse_ticks,"hero_draws":s.hero_draws.get()-hero_draws,"connecting":connecting,"pulse_enabled":s.pulse_enabled,"pulse_running":s.pulse_running
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    if hidden {
        s.hidden = false;
        ShowWindow(s.hwnd, SW_SHOWNOACTIVATE);
    }
    s.model = original_model;
    s.busy = original_busy;
    s.pulse_enabled = original_animation;
    s.update_buttons();
    UpdateWindow(s.controls[&1007]);
    Ok(())
}
unsafe fn smoke_window_chrome(s: &mut State) -> Result<(bool, bool), String> {
    let mut rendering: i32 = 1;
    let query = DwmGetWindowAttribute(
        s.hwnd,
        DWMWA_NCRENDERING_ENABLED as u32,
        &mut rendering as *mut _ as *mut std::ffi::c_void,
        std::mem::size_of_val(&rendering) as u32,
    );
    let original_page = s.page;
    let original_scroll = s.scroll;
    let mut frame_disabled = query >= 0 && rendering == 0;
    let mut styles = Vec::new();
    let mut owner_draw = true;
    for id in 200..204 {
        SendMessageW(s.controls[&id], BM_CLICK, 0, 0);
        SetFocus(s.controls[&id]);
        for _ in 0..12 {
            let mut message: MSG = std::mem::zeroed();
            message.hwnd = GetFocus();
            message.message = WM_KEYDOWN;
            message.wParam = VK_TAB as usize;
            IsDialogMessageW(s.hwnd, &message);
            for button in 200..206 {
                let style = GetWindowLongPtrW(s.controls[&button], GWL_STYLE) as u32;
                owner_draw &= style & BS_TYPEMASK as u32 == BS_OWNERDRAW as u32;
                styles.push((button, style & BS_TYPEMASK as u32));
            }
        }
    }
    let activation = [
        SendMessageW(s.hwnd, WM_NCACTIVATE, 0, 0),
        SendMessageW(s.hwnd, WM_NCACTIVATE, 1, 0),
    ];
    SendMessageW(s.hwnd, WM_NCPAINT, 1, 0);
    let paints = s.parent_paints.get();
    let layouts = s.layout_passes.get();
    let moves = s.geometry_moves.get();
    for cycle in 0..20 {
        frame_disabled &= SendMessageW(s.hwnd, WM_NCACTIVATE, cycle % 2, 0) != 0;
        SendMessageW(s.hwnd, WM_NCPAINT, 1, 0);
    }
    frame_disabled &= s.parent_paints.get() == paints
        && s.layout_passes.get() == layouts
        && s.geometry_moves.get() == moves;
    SendMessageW(s.hwnd, WM_DWMCOMPOSITIONCHANGED, 0, 0);
    ShowWindow(s.hwnd, SW_MINIMIZE);
    SendMessageW(s.hwnd, WM_NCACTIVATE, 0, 0);
    SendMessageW(s.hwnd, ACTIVATE, 0, 0);
    let mut restored_rendering: i32 = 1;
    frame_disabled &= DwmGetWindowAttribute(
        s.hwnd,
        DWMWA_NCRENDERING_ENABLED as u32,
        &mut restored_rendering as *mut _ as *mut std::ffi::c_void,
        std::mem::size_of_val(&restored_rendering) as u32,
    ) >= 0
        && restored_rendering == 0
        && IsIconic(s.hwnd) == 0;
    let mut window: RECT = std::mem::zeroed();
    let mut client: RECT = std::mem::zeroed();
    GetWindowRect(s.hwnd, &mut window);
    GetClientRect(s.hwnd, &mut client);
    frame_disabled &= window.right - window.left == client.right
        && window.bottom - window.top == client.bottom
        && GetWindowLongPtrW(s.hwnd, GWL_STYLE) as u32 & WS_THICKFRAME != 0;
    for (x, y, hit) in [
        ((window.left + window.right) / 2, window.top + 1, HTTOP),
        (
            (window.left + window.right) / 2,
            window.bottom - 1,
            HTBOTTOM,
        ),
        (window.left + 1, (window.top + window.bottom) / 2, HTCLIENT),
    ] {
        frame_disabled &= SendMessageW(
            s.hwnd,
            WM_NCHITTEST,
            0,
            (((y as u32 & 0xffff) << 16) | (x as u32 & 0xffff)) as isize,
        ) == hit as isize;
    }
    std::fs::write(
        s.root.join("native-chrome-observations.json"),
        serde_json::to_vec_pretty(&json!({
            "dwm_query":query,
            "native_frame_rendering_enabled":rendering != 0,
            "restored_native_frame_rendering_enabled":restored_rendering != 0,
            "activation_results":activation,
            "button_types_after_click_and_tab":styles,
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    s.page = original_page;
    s.scroll = original_scroll;
    s.nav_hover = None;
    SetFocus(s.controls[&1007]);
    s.layout();
    s.update_buttons();
    Ok((frame_disabled && activation[0] != 0, owner_draw))
}
unsafe fn smoke_home_design(s: &mut State) -> Result<[bool; 7], String> {
    let hwnd = s.hwnd;
    let original_model = s.model.clone();
    let original_notice = s.notice.clone();
    let original_busy = s.busy;
    let original_action = s.pending_action.clone();
    let original_language = s.locale.language();
    let original_dpi = s.dpi;
    let original_animation = s.pulse_enabled;
    let mut checks = [true; 7];
    let mut rectangle: RECT = std::mem::zeroed();
    SetWindowPos(
        hwnd,
        null_mut(),
        -5000,
        0,
        s.d(400),
        s.d(600),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    GetWindowRect(hwnd, &mut rectangle);
    checks[0] &= rectangle.right - rectangle.left == s.d(HOME_WIDTH)
        && rectangle.bottom - rectangle.top == s.d(HOME_HEIGHT);
    SendMessageW(hwnd, WM_SYSCOMMAND, SC_MAXIMIZE as usize, 0);
    checks[0] &= IsZoomed(hwnd) == 0;
    SetWindowPos(
        hwnd,
        null_mut(),
        -5000,
        0,
        s.d(900),
        s.d(900),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    GetWindowRect(hwnd, &mut rectangle);
    checks[0] &= rectangle.right - rectangle.left == s.d(HOME_WIDTH)
        && rectangle.bottom - rectangle.top == s.d(900);
    s.page = 0;
    s.scroll = 0;
    s.pulse_enabled = true;
    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        GetWindowRect(hwnd, &mut rectangle);
        checks[0] &= rectangle.right - rectangle.left == s.d(HOME_WIDTH);
        for (language, tag) in [(Language::Russian, "ru"), (Language::English, "en")] {
            checks[4] &= s.change_language(language);
            for (phase, ru, en) in [
                ("idle", "Подключиться", "Connect"),
                ("connected", "Отключиться", "Disconnect"),
                ("connecting", "Отменить", "Cancel"),
                ("recovery", "Вернуть сеть", "Restore network"),
            ] {
                s.busy = phase == "connecting";
                s.pending_action = if s.busy { "Connect" } else { "" }.into();
                s.model["phase"] = json!(phase);
                s.model["busy"] = json!(s.busy);
                s.model["running"] = json!(phase == "connected");
                s.model["can_disconnect"] = json!(phase != "idle");
                s.model["last_error"] = json!(if phase == "recovery" {
                    "Fixture recovery · 0x80070057"
                } else {
                    ""
                });
                s.notice.clear();
                s.update_buttons();
                s.layout();
                checks[4] &= s.text(1007)
                    == if language == Language::Russian {
                        ru
                    } else {
                        en
                    };
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
                screenshot(
                    hwnd,
                    &s.root.join(format!("native-home-{tag}-{phase}-{dpi}.bmp")),
                )?;
            }
        }
    }
    s.set_dpi_fonts(144);
    s.smoke_min_height = Some(s.d(469));
    SetWindowPos(
        hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(469),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    for error in [false, true] {
        s.busy = false;
        s.pending_action.clear();
        s.model["phase"] = json!("idle");
        s.model["last_error"] = json!(if error {
            "Приложение содержит недопустимые знаки или имеет недопустимую длину."
        } else {
            ""
        });
        s.notice.clear();
        s.update_buttons();
        s.layout();
        let mut add: RECT = std::mem::zeroed();
        let mut list: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rectangle);
        GetWindowRect(s.controls[&1002], &mut add);
        GetClientRect(s.controls[&1001], &mut list);
        checks[1] &= add.bottom - add.top == s.d(42)
            && add.bottom - rectangle.top <= s.d(s.dimensions().1 - NAV_HEIGHT - NAV_GAP)
            && list.bottom >= s.d(44)
            && IsWindowVisible(s.controls[&1007]) != 0;
        InvalidateRect(hwnd, null(), 0);
        UpdateWindow(hwnd);
        screenshot(
            hwnd,
            &s.root
                .join(format!("native-short-workarea-469-{error}.bmp")),
        )?;
    }
    s.smoke_min_height = None;
    s.set_dpi_fonts(96);
    SetWindowPos(
        hwnd,
        null_mut(),
        -5000,
        0,
        HOME_WIDTH,
        HOME_HEIGHT,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    s.model["last_error"] = json!("");
    s.model["phase"] = json!("connecting");
    s.busy = true;
    s.pending_action = "Connect".into();
    s.update_buttons();
    s.layout();
    InvalidateRect(hwnd, null(), 0);
    UpdateWindow(hwnd);
    UpdateWindow(s.controls[&1007]);
    let parent = s.parent_paints.get();
    let layouts = s.layout_passes.get();
    let moves = s.geometry_moves.get();
    let ticks = s.pulse_ticks.get();
    let mut pulse_observations = Vec::new();
    for _ in 0..PULSE_FRAMES {
        ValidateRect(s.controls[&1007], null());
        SendMessageW(hwnd, WM_TIMER, PULSE_TIMER, 0);
        let mut update: RECT = std::mem::zeroed();
        let dirty = GetUpdateRect(s.controls[&1007], &mut update, 0);
        pulse_observations.push(json!({"active":s.pulse_running,"dirty":dirty,"width":update.right-update.left,"height":update.bottom-update.top,"frame":s.pulse_frame}));
        // Windows can discard an update region on a wholly offscreen window
        // with SetWindowRgn. The buffered hero pixel test below exercises all
        // actual frames; the dirty region is limited to the complete button.
        let shape = CreateRectRgn(0, 0, 0, 0);
        checks[3] &= if dirty != 0 {
            update.right - update.left == s.d(240)
                && update.bottom - update.top == s.d(s.home_layout().hero_h)
        } else {
            GetWindowRgn(s.controls[&1007], shape) != 0
        };
        DeleteObject(shape);
        UpdateWindow(s.controls[&1007]);
    }
    checks[3] &= s.pulse_ticks.get() == ticks + PULSE_FRAMES as u64
        && s.pulse_frame == 0
        && s.parent_paints.get() == parent
        && s.layout_passes.get() == layouts
        && s.geometry_moves.get() == moves;
    pulse_observations.push(json!({"ticks":s.pulse_ticks.get()-ticks,"frame":s.pulse_frame,"parent_paints":s.parent_paints.get()-parent,"layouts":s.layout_passes.get()-layouts,"moves":s.geometry_moves.get()-moves}));
    for mode in 0..3 {
        if mode == 0 {
            SendMessageW(hwnd, WM_COMMAND, 201, 0);
        }
        if mode == 1 {
            SendMessageW(hwnd, WM_COMMAND, 204, 0);
        }
        if mode == 2 {
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        let ticks = s.pulse_ticks.get();
        SendMessageW(hwnd, WM_TIMER, PULSE_TIMER, 0);
        checks[3] &= !s.pulse_running && s.pulse_ticks.get() == ticks;
        if mode == 0 {
            SendMessageW(hwnd, WM_COMMAND, 200, 0);
        } else {
            SendMessageW(hwnd, ACTIVATE, 0, 0);
        }
        checks[3] &= s.pulse_running;
        pulse_observations
            .push(json!({"mode":mode,"resumed":s.pulse_running,"ticks":s.pulse_ticks.get()-ticks}));
    }
    s.model["phase"] = json!("idle");
    s.busy = false;
    s.pending_action.clear();
    s.update_buttons();
    checks[3] &= s.pulse_running;
    std::fs::write(
        s.root.join("native-pulse-observations.json"),
        serde_json::to_vec_pretty(&pulse_observations).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let dc = CreateCompatibleDC(s.buffer_dc);
    let bitmap = CreateCompatibleBitmap(s.buffer_dc, 400, 400);
    if dc.is_null() || bitmap.is_null() {
        return Err("Could not create design smoke bitmap".into());
    }
    let old = SelectObject(dc, bitmap);
    for (id, state) in [
        (200, 0),
        (203, 0),
        (200, ODS_FOCUS),
        (203, ODS_FOCUS),
        (200, ODS_SELECTED),
    ] {
        s.nav_hover = if id == 203 { Some(id) } else { None };
        let mut draw: DRAWITEMSTRUCT = std::mem::zeroed();
        draw.CtlID = id as u32;
        draw.CtlType = ODT_BUTTON;
        draw.itemState = state;
        draw.hDC = dc;
        draw.rcItem = RECT {
            left: 0,
            top: 0,
            right: 140,
            bottom: 80,
        };
        let origin = s.control_bounds.borrow()[&id];
        SetBrushOrgEx(dc, -origin.0, -origin.1, null_mut());
        surface_fill(s, dc, &draw.rcItem, BG);
        let edges: Vec<_> = (0..140)
            .flat_map(|x| [(x, 0), (x, 79)])
            .chain((0..80).flat_map(|y| [(0, y), (139, y)]))
            .map(|(x, y)| (x, y, GetPixel(dc, x, y)))
            .collect();
        SendMessageW(hwnd, WM_DRAWITEM, id as usize, &draw as *const _ as isize);
        for (x, y, background) in edges {
            checks[2] &= GetPixel(dc, x, y) == background;
        }
    }
    s.nav_hover = None;
    for id in 200..204 {
        let bounds = s.control_bounds.borrow()[&id];
        checks[2] &= bounds == ((id - 200) * 140, 660, 140, 80)
            && GetWindowLongPtrW(s.controls[&id], GWL_STYLE) as u32 & WS_TABSTOP != 0;
    }
    let solid = CreateSolidBrush(BG);
    for dpi in [96, 120, 144, 192] {
        s.dpi = dpi;
        for kind in 0..4 {
            let rect = RECT {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            };
            FillRect(dc, &rect, solid);
            icon(s, dc, kind, 20, 20, 24, TEAL);
            let (x, y, cell) = icon_grid_bounds(s, 20, 20, 24);
            for (row, line) in icon_grid(kind).iter().enumerate() {
                for (column, value) in line.bytes().enumerate() {
                    let color = if value == b'X' { TEAL } else { BG };
                    for b in 0..cell {
                        for a in 0..cell {
                            checks[6] &= GetPixel(
                                dc,
                                x + column as i32 * cell + a,
                                y + row as i32 * cell + b,
                            ) == color;
                        }
                    }
                }
            }
            for (a, b) in [
                (x - 1, y),
                (x + cell * 12, y),
                (x, y - 1),
                (x, y + cell * 12),
            ] {
                checks[6] &= GetPixel(dc, a, b) == BG;
            }
        }
        checks[6] &= icon_grid(1) != icon_grid(2)
            && icon_grid(2) != icon_grid(3)
            && icon_grid(1) != icon_grid(3);
        for size in [48, 120] {
            let rect = RECT {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            };
            FillRect(dc, &rect, solid);
            mushroom_palette(s, dc, 20, 20, size, 1);
            let bounds = mushroom_grid_bounds(s, 20, 20, size);
            let width = bounds.right - bounds.left;
            let height = bounds.bottom - bounds.top;
            for (row, line) in mushroom::MINI.iter().enumerate() {
                for (column, value) in line.iter().copied().enumerate() {
                    let color = mushroom::palette_index(value).map_or(BG, |index| {
                        let mut info: LOGBRUSH = std::mem::zeroed();
                        GetObjectW(
                            s.mushroom_brushes[1][index],
                            std::mem::size_of_val(&info) as i32,
                            &mut info as *mut _ as *mut std::ffi::c_void,
                        );
                        info.lbColor
                    });
                    let left = bounds.left + column as i32 * width / mushroom::LOGO_WIDTH as i32;
                    let right =
                        bounds.left + (column + 1) as i32 * width / mushroom::LOGO_WIDTH as i32;
                    let top = bounds.top + row as i32 * height / mushroom::LOGO_HEIGHT as i32;
                    let bottom =
                        bounds.top + (row + 1) as i32 * height / mushroom::LOGO_HEIGHT as i32;
                    for (a, b) in [
                        (left, top),
                        (right - 1, top),
                        (left, bottom - 1),
                        (right - 1, bottom - 1),
                    ] {
                        checks[5] &= GetPixel(dc, a, b) == color;
                    }
                }
            }
        }
    }
    DeleteObject(solid);
    SelectObject(dc, old);
    DeleteObject(bitmap);
    DeleteDC(dc);
    s.dpi = 96;
    s.model = original_model;
    s.notice = original_notice;
    s.busy = original_busy;
    s.pending_action = original_action;
    s.pulse_enabled = original_animation;
    s.set_dpi_fonts(original_dpi);
    s.change_language(original_language);
    SetWindowPos(
        hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    s.update_buttons();
    s.layout();
    Ok(checks)
}
unsafe fn smoke_import_flow(s: &mut State) -> Result<[bool; 5], String> {
    // Every reply and every file below belongs to this isolated, synthetic UI
    // fixture. No parser, clipboard, subscription or real controller is used.
    let original_model = s.model.clone();
    let original_notice = s.notice.clone();
    let original_language = s.locale.language();
    let original_dpi = s.dpi;
    let original_page = s.page;
    let original_scroll = s.scroll;
    let original_top = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
    let original_minimum = s.smoke_min_height;
    let original_busy = s.busy;
    let original_action = s.pending_action.clone();
    let original_pending = s.pending_id.clone();
    let original_since = s.pending_since;
    let mut checks = [true; 5];
    let mut observations = Vec::new();
    s.page = 0;
    s.busy = false;
    s.pending_action.clear();
    s.pending_id = None;
    s.pending_since = None;
    s.notice.clear();
    s.model["last_error"] = json!("");
    SendMessageW(s.hwnd, WM_COMMAND, 1031, 0);
    let generation = s.generation;
    SendMessageW(s.hwnd, WM_COMMAND, 1021, 0);
    checks[0] &= s.import_open
        && !s.busy
        && s.generation == generation
        && IsWindowEnabled(s.controls[&1021]) == 0;
    let oversized = "я".repeat(IMPORT_LIMIT / 2 + 1);
    SendMessageW(
        s.controls[&1020],
        EM_REPLACESEL,
        0,
        wide(&oversized).as_ptr() as isize,
    );
    checks[0] &= s.text(1020) == oversized
        && IsWindowEnabled(s.controls[&1021]) == 0
        && s.import_error.is_some();
    SendMessageW(s.hwnd, WM_COMMAND, 1021, 0);
    checks[0] &= !s.busy && s.generation == generation && s.text(1020) == oversized;
    let content = "vless://00000000-0000-4000-8000-000000000001@fixture.example:443#Fixture\r\n";
    let expected = import_request(content.into(), "Fixture", "pasted-text");
    checks[0] &= expected["action"] == "Import"
        && expected["format"] == "auto"
        && expected["source_name"] == "pasted-text"
        && expected["content"] == content
        && expected.get("url").is_none();
    let escaped = "\"\n\t".repeat(IMPORT_LIMIT / 3);
    let escaped_request = import_request(escaped.clone(), "Escaped fixture", "pasted-text");
    checks[0] &= escaped.len() <= IMPORT_LIMIT
        && serde_json::to_vec(&escaped_request).unwrap().len() > 1048576
        && escaped_request["content"].as_str() == Some(escaped.as_str());
    let files = s.root.join("import-file-fixtures");
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    for (filename, bytes, valid) in [
        (
            "bom.txt",
            [b"\xef\xbb\xbf".as_slice(), content.as_bytes()].concat(),
            true,
        ),
        ("empty.txt", b" \r\n".to_vec(), false),
        ("invalid.txt", vec![0xff, 0xfe], false),
        ("large.txt", vec![b'x'; IMPORT_LIMIT + 1], false),
    ] {
        let path = files.join(filename);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        let result = read_import_content(&path);
        checks[0] &= if valid {
            result.as_deref() == Ok(content)
        } else {
            result.is_err()
        };
    }
    SendMessageW(s.controls[&1020], EM_SETSEL, 0, -1);
    SendMessageW(
        s.controls[&1020],
        EM_REPLACESEL,
        0,
        wide(content).as_ptr() as isize,
    );
    checks[0] &= s.import_error.is_none() && IsWindowEnabled(s.controls[&1021]) != 0;
    SendMessageW(s.controls[&1020], EM_SETSEL, 5, 19);
    let mut data = s.model.clone();
    data["updater"] = json!({"last_run":"synthetic-status-only"});
    let answer = Box::into_raw(Box::new(Answer {
        response: Ok(json!({"ok":true,"data":data})),
        action: "Status".into(),
        generation: s.generation,
        request_id: None,
    }));
    SendMessageW(s.hwnd, REPLY, 0, answer as isize);
    let mut start = 0u32;
    let mut end = 0u32;
    SendMessageW(
        s.controls[&1020],
        EM_GETSEL,
        &mut start as *mut _ as usize,
        &mut end as *mut _ as isize,
    );
    checks[1] &= s.import_open && s.text(1020) == content && (start, end) == (5, 19);
    s.busy = true;
    s.pending_action = "Import".into();
    s.pending_id = Some("fixture-import-error".into());
    s.pending_since = Some(Instant::now());
    let mut data = s.model.clone();
    data["last_error"] = json!("Invalid configuration: fixture protocol option");
    let answer = Box::into_raw(Box::new(Answer {
        response: Ok(json!({"ok":false,"error":data["last_error"],"data":data})),
        action: "Import".into(),
        generation: s.generation,
        request_id: Some("fixture-import-error".into()),
    }));
    SendMessageW(s.hwnd, REPLY, 0, answer as isize);
    checks[1] &= !s.busy
        && s.import_open
        && s.text(1020) == content
        && IsWindowEnabled(s.controls[&1021]) != 0
        && IsWindowVisible(s.controls[&1011]) != 0;
    s.model["last_error"] = json!("");
    s.notice.clear();
    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        s.smoke_min_height = None;
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        for (language, tag) in [(Language::Russian, "ru"), (Language::English, "en")] {
            checks[1] &= s.change_language(language) && s.text(1020) == content;
            SendMessageW(
                s.controls[&1020],
                EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            checks[1] &= (start, end) == (5, 19)
                && s.text(1021)
                    == if language == Language::English {
                        "Import"
                    } else {
                        "Импортировать"
                    };
            s.layout();
            InvalidateRect(s.hwnd, null(), 0);
            UpdateWindow(s.hwnd);
            screenshot(
                s.hwnd,
                &s.root.join(format!("native-import-{tag}-{dpi}.bmp")),
            )?;
            let mut window: RECT = std::mem::zeroed();
            let mut caption: RECT = std::mem::zeroed();
            GetWindowRect(s.hwnd, &mut window);
            GetWindowRect(s.controls[&204], &mut caption);
            let version = concat!("v", env!("CARGO_PKG_VERSION"));
            let brand_right = s.d(72 + measured_width(s, s.buffer_dc, crate::APP_NAME, 2));
            let version_right = s.d(194 + measured_width(s, s.buffer_dc, version, 3));
            let mut version_pixels = 0;
            for y in s.d(38)..s.d(58) {
                for x in s.d(194)..version_right {
                    version_pixels += usize::from(GetPixel(s.buffer_dc, x, y) == MUTED);
                }
            }
            checks[4] &= brand_right <= s.d(194)
                && version_right < caption.left - window.left
                && version_pixels > 5;
            observations.push(json!({"dpi":dpi,"language":tag,"brand_right":brand_right,"version_right":version_right,"version_ink":version_pixels}));
        }
    }
    s.set_dpi_fonts(144);
    s.smoke_min_height = Some(s.d(469));
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(469),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    for with_error in [false, true] {
        s.import_error =
            with_error.then(|| "Invalid configuration: fixture protocol option".into());
        s.layout();
        let mut window: RECT = std::mem::zeroed();
        GetWindowRect(s.hwnd, &mut window);
        for id in [1020, 1021, 1022] {
            let mut rect: RECT = std::mem::zeroed();
            GetWindowRect(s.controls[&id], &mut rect);
            checks[4] &= IsWindowVisible(s.controls[&id]) != 0
                && rect.top >= window.top + s.d(88)
                && rect.bottom <= window.top + s.d(389)
                && rect.bottom - rect.top >= s.d(42);
        }
        InvalidateRect(s.hwnd, null(), 0);
        UpdateWindow(s.hwnd);
        screenshot(
            s.hwnd,
            &s.root.join(format!(
                "native-import-short-{}.bmp",
                if with_error { "error" } else { "idle" }
            )),
        )?;
    }
    SendMessageW(s.hwnd, WM_COMMAND, 1022, 0);
    checks[2] &= !s.import_open && s.text(1020).is_empty() && s.import_error.is_none();
    s.smoke_min_height = None;
    s.set_dpi_fonts(96);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    s.model = original_model.clone();
    s.model["last_error"] = json!("");
    s.notice.clear();
    s.populate_profiles();
    SendMessageW(s.controls[&1001], LB_SETTOPINDEX, 3, 0);
    let selected = s.model["selected"].clone();
    let top = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
    let protocols = [
        "VLESS",
        "VMess",
        "Trojan",
        "Shadowsocks",
        "Hysteria2",
        "TUIC",
        "WireGuard",
        "AmneziaWG",
        "SOCKS5",
        "HTTP",
        "AnyTLS",
        "OpenVPN",
    ];
    for (receipt, action) in ["direct", "lost"].into_iter().zip(["Import", "Status"]) {
        SendMessageW(s.hwnd, WM_COMMAND, 1031, 0);
        s.set(1020, content);
        s.busy = true;
        s.pending_action = "Import".into();
        s.pending_id = Some(format!("fixture-import-{receipt}"));
        s.pending_since = Some(Instant::now());
        let mut data = s.model.clone();
        for (n, protocol) in protocols.iter().enumerate() {
            data["profiles"].as_array_mut().unwrap().push(json!({"id":format!("fixture-{receipt}-{n}"),"name":format!("Imported {receipt} {:02}",n+1),"protocol":protocol}));
        }
        data["last_operation"] = json!({"request_id":s.pending_id,"ok":true,"imported_count":12});
        if action == "Import" {
            data["imported_count"] = json!(12);
        } else {
            data.as_object_mut().unwrap().remove("imported_count");
        }
        let answer = Box::into_raw(Box::new(Answer {
            response: Ok(json!({"ok":true,"data":data})),
            action: action.into(),
            generation: s.generation,
            request_id: if action == "Import" {
                s.pending_id.clone()
            } else {
                None
            },
        }));
        SendMessageW(s.hwnd, REPLY, 0, answer as isize);
        checks[2] &= !s.import_open
            && s.text(1020).is_empty()
            && !s.busy
            && s.pending_id.is_none()
            && s.imported_count == Some(12)
            && s.next.is_none();
        checks[3] &= s.model["selected"] == selected
            && s.model["running"] == true
            && SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) == top;
    }
    let count = SendMessageW(s.controls[&1001], LB_GETCOUNT, 0, 0);
    checks[3] &= count == original_model["profiles"].as_array().unwrap().len() as isize + 24
        && profile_protocol(&json!({"name":"Legacy"})).is_empty();
    for (raw, display) in [
        ("amneziawg", "AWG"),
        ("wireguard", "WG"),
        ("vless", "VLESS"),
        ("vmess", "VMess"),
        ("ss", "Shadowsocks"),
        ("ssr", "ShadowsocksR"),
        ("hysteria2", "Hysteria2"),
        ("trusttunnel", "TrustTunnel"),
        ("shadowquic", "ShadowQUIC"),
        ("gost-relay", "GOST Relay"),
    ] {
        let profile = json!({"protocol":raw});
        checks[3] &= profile_protocol(&profile) == display && profile["protocol"] == raw;
    }
    for item in s.model["profiles"].as_array().unwrap().iter().skip(12) {
        checks[3] &= protocols.contains(&profile_protocol(item));
    }
    SendMessageW(
        s.controls[&1001],
        WM_MOUSEWHEEL,
        (-120i16 as u16 as usize) << 16,
        0,
    );
    checks[3] &= SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) > top;
    SendMessageW(s.controls[&1001], WM_KEYDOWN, VK_END as usize, 0);
    checks[3] &= SendMessageW(s.controls[&1001], LB_GETCURSEL, 0, 0) == count - 1;
    SendMessageW(s.controls[&1001], LB_SETTOPINDEX, 12, 0);
    InvalidateRect(s.hwnd, null(), 0);
    UpdateWindow(s.hwnd);
    screenshot(s.hwnd, &s.root.join("native-imported-protocols.bmp"))?;
    std::fs::write(
        s.root.join("native-import-layout.json"),
        serde_json::to_vec_pretty(&observations).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    s.close_import();
    s.imported_count = None;
    s.model = original_model;
    s.notice = original_notice;
    s.busy = original_busy;
    s.pending_action = original_action;
    s.pending_id = original_pending;
    s.pending_since = original_since;
    s.page = original_page;
    s.scroll = original_scroll;
    s.smoke_min_height = original_minimum;
    s.set_dpi_fonts(original_dpi);
    s.change_language(original_language);
    s.populate_profiles();
    SendMessageW(s.controls[&1001], LB_SETTOPINDEX, original_top as usize, 0);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    s.update_buttons();
    s.layout();
    Ok(checks)
}
unsafe fn smoke_hero_and_edit_padding(s: &mut State) -> Result<[bool; 3], String> {
    let original_dpi = s.dpi;
    let original_page = s.page;
    let original_dns = s.dns_settings_open;
    let original_scroll = s.scroll;
    let original_rename = s.rename_open;
    let original_language = s.locale.language();
    let original_model = s.model.clone();
    let original_busy = s.busy;
    let original_action = s.pending_action.clone();
    let original_frame = s.pulse_frame;
    let original_effects = s.locale.design.effects;
    let ids = [1006, 2102, 2105, 3310, 3006, 3007, 3008, 3009, 3013, 3014];
    let original_texts: Vec<_> = ids.iter().map(|id| (*id, s.text(*id))).collect();
    let dc = CreateCompatibleDC(s.buffer_dc);
    let bitmap = CreateCompatibleBitmap(s.buffer_dc, 1100, 600);
    if dc.is_null() || bitmap.is_null() {
        return Err("Could not create editor smoke bitmap".into());
    }
    let previous = SelectObject(dc, bitmap);
    let mut checks = [true; 3];
    let mut observations = Vec::new();
    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(1220),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        for page in [0, 1, 2, 3] {
            s.dns_settings_open = page == 3;
            s.page = page;
            s.scroll = 0;
            s.rename_open = true;
            s.layout();
            for id in ids {
                if IsWindowVisible(s.controls[&id]) == 0 {
                    continue;
                }
                let multiline = [2102, 3006].contains(&id);
                s.set(
                    id,
                    if multiline {
                        "fixture.example\r\nsecond.example"
                    } else if id == 1006 {
                        "Fixture server"
                    } else {
                        "123"
                    },
                );
                let control = s.controls[&id];
                let mut rect: RECT = std::mem::zeroed();
                let mut window: RECT = std::mem::zeroed();
                GetWindowRect(control, &mut window);
                SendMessageW(control, EM_GETRECT, 0, &mut rect as *mut _ as isize);
                let metrics = edit_font_metrics(control);
                let height = window.bottom - window.top;
                let width = window.right - window.left;
                let mut client_origin = POINT { x: 0, y: 0 };
                ClientToScreen(control, &mut client_origin);
                let inset = client_origin.y - window.top;
                checks[1] &= rect.left >= s.d(10)
                    && if multiline {
                        rect.top >= s.d(10)
                    } else {
                        inset == 0 && rect.top >= 0
                    }
                    && (GetWindowLongPtrW(control, GWL_STYLE) as u32 & ES_WANTRETURN as u32 != 0)
                        == multiline;
                let mut selection_start: u32 = 0;
                let mut selection_end: u32 = 0;
                SendMessageW(control, EM_SETSEL, 1, 4);
                let top = SendMessageW(control, EM_GETFIRSTVISIBLELINE, 0, 0);
                format_edit(control, dpi, true);
                SendMessageW(
                    control,
                    EM_GETSEL,
                    &mut selection_start as *mut _ as usize,
                    &mut selection_end as *mut _ as isize,
                );
                let after_top = SendMessageW(control, EM_GETFIRSTVISIBLELINE, 0, 0);
                checks[2] &= selection_start == 1
                    && selection_end == 4.min(s.text(id).chars().count() as u32)
                    && after_top == top;
                SendMessageW(control, EM_SETSEL, 0, 0);
                SendMessageW(
                    control,
                    WM_PRINT,
                    dc as usize,
                    (PRF_CLIENT | PRF_NONCLIENT | PRF_ERASEBKGND) as isize,
                );
                let (mut first, mut last) = (height, -1);
                let probe_height = if multiline {
                    s.d(80).min(height)
                } else {
                    height
                };
                for y in 0..probe_height.min(600) {
                    for x in 0..width.min(s.d(250)).min(1100) {
                        if GetPixel(dc, x, y) == INK {
                            first = first.min(y);
                            last = last.max(y);
                        }
                    }
                }
                checks[1] &= first >= s.d(8) && last >= first && last < height - s.d(8);
                if !multiline {
                    checks[1] &= ((first + last + 1) - height).abs() <= s.d(2).max(1);
                    let point = POINT {
                        x: window.left + s.d(2),
                        y: window.top + s.d(2),
                    };
                    checks[2] &= SendMessageW(
                        control,
                        WM_NCHITTEST,
                        0,
                        (((point.y as u32 & 0xffff) << 16) | (point.x as u32 & 0xffff)) as isize,
                    ) == HTCLIENT as isize;
                }
                let (_, cap_top, cap_height) = edit_cap_metrics(control);
                observations.push(json!({"dpi":dpi,"id":id,"height":height,"inset":inset,
                    "format_top":rect.top,"font_height":metrics.tmHeight,
                    "font_leading":metrics.tmInternalLeading,"first_ink":first,"last_ink":last,
                    "selection_start":selection_start,"selection_end":selection_end,
                    "top_before":top,"top_after":after_top,
                    "cap_top":cap_top,"cap_height":cap_height}));
            }
            InvalidateRect(s.hwnd, null(), 0);
            UpdateWindow(s.hwnd);
            let tag = if dpi == 96 || dpi == 144 { "ru" } else { "en" };
            s.change_language(if tag == "ru" {
                Language::Russian
            } else {
                Language::English
            });
            screenshot(
                s.hwnd,
                &s.root.join(format!("native-padded-{tag}-{page}-{dpi}.bmp")),
            )?;
        }
    }
    s.set_dpi_fonts(96);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        HOME_WIDTH,
        HOME_HEIGHT,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    s.page = 0;
    s.scroll = 0;
    s.rename_open = false;
    s.busy = false;
    s.pending_action.clear();
    s.model["phase"] = json!("connected");
    // The confirmed connection renders the complete original artwork.
    // Partial growth is exercised separately by smoke_hero_feedback.
    s.locale.design.effects = false;
    s.update_buttons();
    s.layout();
    let rect = RECT {
        left: 0,
        top: 0,
        right: 240,
        bottom: 196,
    };
    let mut draw: DRAWITEMSTRUCT = std::mem::zeroed();
    draw.CtlID = 1007;
    draw.hDC = dc;
    draw.rcItem = rect;
    SendMessageW(s.hwnd, WM_DRAWITEM, 1007, &draw as *const _ as isize);
    let buffer_dc = s.hero_buffer.dc;
    let buffer_bitmap = s.hero_buffer.bitmap;
    let allocations = s.hero_buffer.allocations;
    let presentations = s.hero_presentations.get();
    let draws = s.hero_draws.get();
    GdiFlush();
    let resources = GetGuiResources(GetCurrentProcess(), 0);
    let caption: Vec<_> = (156..188)
        .step_by(4)
        .flat_map(|y| (0..240).step_by(4).map(move |x| (x, y)))
        .map(|(x, y)| (x, y, GetPixel(dc, x, y)))
        .collect();
    let saved = SaveDC(dc);
    IntersectClipRect(dc, 40, 8, 200, 152);
    for frame in 0..200 {
        s.pulse_frame = (frame % PULSE_FRAMES as usize) as u8;
        draw.itemState = match frame % 4 {
            0 => 0,
            1 => ODS_FOCUS,
            2 => ODS_SELECTED,
            _ => ODS_FOCUS | ODS_SELECTED,
        };
        SendMessageW(s.hwnd, WM_DRAWITEM, 1007, &draw as *const _ as isize);
        let mut colors = [0; mushroom::PALETTE_LEN];
        for (color, brush) in colors.iter_mut().zip(s.mushroom_brushes[0]) {
            let mut info: LOGBRUSH = std::mem::zeroed();
            GetObjectW(
                brush,
                std::mem::size_of_val(&info) as i32,
                &mut info as *mut _ as *mut std::ffi::c_void,
            );
            *color = info.lbColor;
        }
        let origin = s.control_bounds.borrow()[&1007];
        for (row, line) in mushroom::HERO.iter().enumerate() {
            for (column, value) in line.iter().copied().enumerate() {
                let x = 40 + column as i32 * 2 + 1;
                let y = 8 + row as i32 * 2 + 1;
                let color = mushroom::palette_index(value).map_or_else(
                    || crate::texture::sample(BG, 96, origin.0 + x, origin.1 + y),
                    |index| colors[index],
                );
                checks[0] &= GetPixel(dc, x, y) == color;
            }
        }
    }
    RestoreDC(dc, saved);
    GdiFlush();
    checks[0] &= s.hero_buffer.dc == buffer_dc
        && s.hero_buffer.bitmap == buffer_bitmap
        && s.hero_buffer.allocations == allocations
        && s.hero_presentations.get() == presentations + 200
        && s.hero_draws.get() == draws + 200
        && GetGuiResources(GetCurrentProcess(), 0) == resources
        && caption
            .iter()
            .all(|&(x, y, pixel)| GetPixel(dc, x, y) == pixel);
    SetPixel(dc, 0, 0, 0x00ff00ff);
    checks[0] &= SendMessageW(s.controls[&1007], WM_ERASEBKGND, dc as usize, 0) == 1
        && GetPixel(dc, 0, 0) == 0x00ff00ff;
    std::fs::write(
        s.root.join("native-editor-observations.json"),
        serde_json::to_vec_pretty(&observations).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    SelectObject(dc, previous);
    DeleteObject(bitmap);
    DeleteDC(dc);
    s.model = original_model;
    s.busy = original_busy;
    s.pending_action = original_action;
    s.pulse_frame = original_frame;
    s.locale.design.effects = original_effects;
    s.set_dpi_fonts(original_dpi);
    s.change_language(original_language);
    for (id, value) in original_texts {
        s.set(id, &value);
    }
    s.page = original_page;
    s.dns_settings_open = original_dns;
    s.scroll = original_scroll;
    s.rename_open = original_rename;
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    s.update_buttons();
    s.layout();
    Ok(checks)
}
unsafe fn smoke_design_preferences(s: &mut State) -> Result<[bool; 8], String> {
    let original_appearance = s.appearance_open;
    s.appearance_open = true;
    SetFocus(s.hwnd);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_RESUME_TIMER, 0);
    let original = (
        s.locale.design,
        s.page,
        s.scroll,
        s.background_enabled,
        s.pulse_enabled,
        s.background_millis,
    );
    let draft = s.draft.clone();
    let model = s.model.clone();
    let text = s.text(2102);
    let mut selection = (0u32, 0u32);
    SendMessageW(
        s.controls[&2102],
        EM_GETSEL,
        &mut selection.0 as *mut _ as usize,
        &mut selection.1 as *mut _ as isize,
    );
    s.locale.set_design(crate::design::Design::default())?;
    s.background_enabled = true;
    s.pulse_enabled = true;
    s.page = 3;
    s.scroll = 488 + APPEARANCE_EXTRA_HEIGHT;
    s.layout();
    s.refresh_design();
    UpdateWindow(s.hwnd);
    screenshot(s.hwnd, &s.root.join("native-design.bmp"))?;
    let mut checks = [true; 8];
    let mut viewport = POINT::default();
    GetViewportOrgEx(s.buffer_dc, &mut viewport);
    checks[5] &= (viewport.x, viewport.y) == (0, 0)
        && GetWindowLongPtrW(s.hwnd, GWL_EXSTYLE) as u32 & WS_EX_COMPOSITED != 0;
    let (menu, labels) = pixel_menu(s, &[(1, "Открыть настройки"), (2, "Выход из Mukhomor")]);
    let mut item: MENUITEMINFOW = std::mem::zeroed();
    item.cbSize = std::mem::size_of_val(&item) as u32;
    item.fMask = MIIM_FTYPE | MIIM_DATA;
    checks[0] &= GetMenuItemInfoW(menu, 1, 0, &mut item) != 0
        && item.fType & MFT_OWNERDRAW != 0
        && menu_label(item.dwItemData) == s.locale.text("Открыть настройки");
    let mut measure: MEASUREITEMSTRUCT = std::mem::zeroed();
    measure.CtlType = ODT_MENU;
    measure.itemData = labels[0].as_ptr() as usize;
    SendMessageW(s.hwnd, WM_MEASUREITEM, 0, &mut measure as *mut _ as isize);
    checks[0] &= measure.itemHeight == s.d(38) as u32 && measure.itemWidth > s.d(28) as u32;
    DestroyMenu(menu);
    checks[0] &= s.background_running
        && [4020, 4021, 4023]
            .into_iter()
            .all(|id| IsWindowVisible(s.controls[&id]) != 0 && s.checked(id));
    SendMessageW(s.hwnd, WM_COMMAND, 4020, 0);
    UpdateWindow(s.hwnd);
    screenshot(s.hwnd, &s.root.join("native-design-simple.bmp"))?;
    let region = CreateRectRgn(0, 0, 0, 0);
    checks[1] &= !s.background_running
        && !s.locale.design.effects
        && !Locale::new(&s.root, true)?.design.effects
        && GetWindowRgn(s.hwnd, region) == 0
        && [4021, 4023]
            .into_iter()
            .all(|id| IsWindowEnabled(s.controls[&id]) == 0)
        && GetPixel(s.buffer_dc, 10, 500) == BG;
    let frames = s.textures.background_frames;
    let millis = s.background_millis;
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
    checks[1] &= s.textures.background_frames == frames && s.background_millis == millis;
    SendMessageW(s.hwnd, WM_COMMAND, 4020, 0);
    SendMessageW(s.hwnd, WM_COMMAND, 4021, 0);
    checks[2] &= !s.background_running && !s.locale.design.animation;
    SendMessageW(s.hwnd, WM_COMMAND, 4023, 0);
    checks[2] &= !s.locale.design.rounding
        && GetWindowRgn(s.controls[&4020], region) == 0
        && Locale::new(&s.root, true)?.design == s.locale.design;
    SendMessageW(s.hwnd, WM_COMMAND, 4023, 0);
    SendMessageW(s.hwnd, WM_COMMAND, 4021, 0);
    checks[3] &= s.background_running;
    // A child-first paint must see the next frame before the parent's WM_PAINT.
    let frames = s.textures.background_frames;
    let millis = s.background_millis;
    s.background_since = Instant::now() - std::time::Duration::from_millis(500);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
    checks[6] &= s.textures.background_frames > frames && s.background_millis != millis;
    s.background_millis = crate::texture::FLOW_PERIOD_MS / 2;
    s.prepare_background();
    s.invalidate_background();
    UpdateWindow(s.hwnd);
    screenshot(s.hwnd, &s.root.join("native-design-flow.bmp"))?;
    RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
    GdiFlush();
    // Exercise animated paints before sampling resources. Native controls can
    // retire temporary GDI caches on their first frame after the mode switch.
    for _ in 0..3 {
        s.background_since = Instant::now() - std::time::Duration::from_millis(50);
        SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
        InvalidateRect(s.hwnd, null(), 0);
        RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
        GdiFlush();
    }
    let resources = GetGuiResources(GetCurrentProcess(), 0);
    let layouts = s.layout_passes.get();
    let moves = s.geometry_moves.get();
    let paints = s.parent_paints.get();
    let frames = s.textures.background_frames;
    let mut timings = Vec::new();
    for _ in 0..60 {
        s.background_since = Instant::now() - std::time::Duration::from_millis(50);
        let started = Instant::now();
        let before_paint = s.parent_paints.get();
        SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
        // Windows may discard a partial dirty rectangle for this wholly
        // off-screen shaped fixture. Force a full parent paint for a real,
        // conservative rendering measurement; children retain scoped damage.
        if s.parent_paints.get() == before_paint {
            InvalidateRect(s.hwnd, null(), 0);
        }
        RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
        GdiFlush();
        timings.push(started.elapsed().as_micros() as u64);
    }
    checks[7] &= s.layout_passes.get() == layouts
        && s.geometry_moves.get() == moves
        && s.parent_paints.get() == paints + 60
        && s.textures.background_frames == frames + 60
        && GetGuiResources(GetCurrentProcess(), 0) == resources;
    std::fs::write(
        s.root.join("native-animation-performance.json"),
        serde_json::to_vec_pretty(&json!({
            "frames":60, "dpi":s.dpi,
            "mean_frame_us":timings.iter().sum::<u64>() / timings.len() as u64,
            "max_frame_us":timings.iter().max(),
            "gdi_objects_before":resources,
            "gdi_objects_after":GetGuiResources(GetCurrentProcess(), 0),
            "parent_paints":s.parent_paints.get()-paints,
            "background_frames":s.textures.background_frames-frames,
            "layout_passes":s.layout_passes.get()-layouts,
            "geometry_moves":s.geometry_moves.get()-moves
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    for mode in 0..2 {
        if mode == 0 {
            ShowWindow(s.hwnd, SW_MINIMIZE);
        } else {
            SendMessageW(s.hwnd, WM_CLOSE, 0, 0);
        }
        let millis = s.background_millis;
        let frames = s.textures.background_frames;
        SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
        checks[3] &= !s.background_running
            && s.background_millis == millis
            && s.textures.background_frames == frames;
        SendMessageW(s.hwnd, ACTIVATE, 0, 0);
        checks[3] &= s.background_running;
    }
    s.pulse_enabled = false;
    s.sync_background();
    // The explicit in-app animation switch is independent of Windows' native
    // control animation preference, which still governs the mushroom pulse.
    checks[6] &= s.background_running;
    let mut after = (0u32, 0u32);
    SendMessageW(
        s.controls[&2102],
        EM_GETSEL,
        &mut after.0 as *mut _ as usize,
        &mut after.1 as *mut _ as isize,
    );
    checks[2] &= s.draft == draft && s.model == model && s.text(2102) == text && selection == after;
    DeleteObject(region);
    // A native combo has separate selected-row and dropdown heights. Verify
    // actual row corners and square arrow pixels after DPI transitions.
    let original_dpi = s.dpi;
    let combo = s.controls[&3101];
    let original_selection = SendMessageW(combo, CB_GETCURSEL, 0, 0);
    let mut combo_observations = Vec::new();
    let dc = CreateCompatibleDC(s.buffer_dc);
    let bitmap = CreateCompatibleBitmap(s.buffer_dc, 1200, 200);
    let old = SelectObject(dc, bitmap);
    let shape = CreateRectRgn(0, 0, 0, 0);
    s.page = 1;
    s.scroll = 0;
    SendMessageW(combo, CB_SETCURSEL, 0, 0);
    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        s.layout();
        let mut r = RECT::default();
        GetClientRect(combo, &mut r);
        let mut window = RECT::default();
        GetWindowRect(combo, &mut window);
        let shape_type = GetWindowRgn(combo, shape);
        let bottom = PtInRegion(shape, r.right / 2, r.bottom - 1);
        let corner = PtInRegion(shape, r.right - 1, r.bottom - 1);
        checks[4] &= r.bottom >= s.d(38)
            && r.bottom <= s.d(50)
            && GetWindowRgn(combo, shape) != 0
            && PtInRegion(shape, r.right / 2, r.bottom - 1) != 0
            && PtInRegion(shape, r.right - 1, r.bottom - 1) == 0
            && !s.text(3101).is_empty();
        SendMessageW(combo, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
        let unit = (2 * dpi + 48) / 96;
        let (x, y) = (r.right - s.d(24), (r.bottom - 4 * unit) / 2);
        let mut arrow = true;
        for (row, line) in ["XX....XX", ".XX..XX.", "..XXXX..", "...XX..."]
            .iter()
            .enumerate()
        {
            for (col, bit) in line.bytes().enumerate() {
                for b in 0..unit {
                    for a in 0..unit {
                        arrow &=
                            (GetPixel(dc, x + col as i32 * unit + a, y + row as i32 * unit + b)
                                == MUTED)
                                == (bit == b'X');
                    }
                }
            }
        }
        checks[4] &= arrow;
        combo_observations.push(json!({"dpi":dpi,"client_height":r.bottom,"window_height":window.bottom-window.top,"shape_type":shape_type,"bottom":bottom,"corner":corner,"text":s.text(3101),"arrow":arrow}));
    }
    std::fs::write(
        s.root.join("native-combo-observations.json"),
        serde_json::to_vec_pretty(&combo_observations).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    DeleteObject(shape);
    SelectObject(dc, old);
    DeleteObject(bitmap);
    DeleteDC(dc);
    s.set_dpi_fonts(original_dpi);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    SendMessageW(combo, CB_SETCURSEL, original_selection as usize, 0);
    s.locale.set_design(original.0)?;
    s.appearance_open = original_appearance;
    s.page = original.1;
    s.scroll = original.2;
    s.background_enabled = original.3;
    s.pulse_enabled = original.4;
    s.background_millis = original.5;
    s.layout();
    s.refresh_design();
    Ok(checks)
}
// Exercise native repaint/visibility behavior, including the boundary pixels
// that a screenshot taken after a full redraw would otherwise miss.
unsafe fn smoke_list_surfaces(s: &mut State) -> Result<[bool; 6], String> {
    let original = (
        s.model.clone(),
        s.draft.clone(),
        s.selected_preset.clone(),
        s.page,
        s.scroll,
        s.monitor_dpi,
        s.locale.design,
    );
    let mut checks = [true; 6];
    s.model["profiles"] = json!(
        (0..16)
            .map(|n| json!({
                "id": format!("qa-{n}"), "name": format!("Server {n}"), "protocol":"WireGuard"
            }))
            .collect::<Vec<_>>()
    );
    s.model["selected"] = json!("qa-0");
    s.draft["presets"] = json!(
        (0..16)
            .map(|n| json!({
                "id": format!("qa-{n}"), "name": format!("Набор {n}"),
                "enabled": n % 2 == 0, "rules":{}, "domain_rule_sets":[]
            }))
            .collect::<Vec<_>>()
    );
    s.selected_preset = Some("qa-0".into());
    s.populate_profiles();
    s.populate_presets();
    let mut observations = Vec::new();
    for dpi in [96, 120, 144, 192] {
        s.locale.design = crate::design::Design::default();
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        for (page, id) in [(0, 1001), (2, 3301)] {
            s.page = page;
            s.scroll = 0;
            let list = s.controls[&id];
            SendMessageW(list, LB_SETCURSEL, 8, 0);
            SendMessageW(list, LB_SETTOPINDEX, 5, 0);
            for (effects, rounding) in [(true, true), (true, false), (false, true)] {
                s.locale.design.effects = effects;
                s.locale.design.rounding = rounding;
                s.refresh_design();
                SetFocus(s.hwnd);
                let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
                let selected = SendMessageW(list, LB_GETCURSEL, 0, 0);
                let mut bounds = RECT::default();
                GetClientRect(list, &mut bounds);
                let mut snapshot = PaintBuffer::default();
                if !snapshot.ensure(s.buffer_dc, (bounds.right, bounds.bottom)) {
                    return Err("Cannot allocate list QA bitmap".into());
                }
                SendMessageW(list, WM_PRINT, snapshot.dc as usize, PRF_CLIENT as isize);
                let shape = CreateRectRgn(0, 0, 0, 0);
                let has_shape = GetWindowRgn(list, shape) != 0;
                let u = crate::design::corner_unit(dpi).min(bounds.bottom / 12);
                let edge = s.textures.color(LINE);
                let rounded = effects && rounding;
                let frame = if rounded {
                    [
                        (3 * u, 0),
                        (2 * u, u),
                        (u, 2 * u),
                        (0, 3 * u),
                        (bounds.right - 3 * u - 1, bounds.bottom - 1),
                        (bounds.right - 1, bounds.bottom - 3 * u - 1),
                    ]
                    .into_iter()
                    .all(|(x, y)| GetPixel(snapshot.dc, x, y) == edge)
                } else {
                    GetPixel(snapshot.dc, 0, 0) == edge
                        && GetPixel(snapshot.dc, bounds.right - 1, bounds.bottom - 1) == edge
                };
                if rounded {
                    checks[0] &= frame && has_shape && PtInRegion(shape, 0, 0) == 0;
                } else {
                    checks[1] &= frame && !has_shape;
                }
                DeleteObject(shape);
                SetFocus(list);
                SendMessageW(
                    list,
                    WM_PRINTCLIENT,
                    snapshot.dc as usize,
                    PRF_CLIENT as isize,
                );
                checks[2] &= GetPixel(snapshot.dc, bounds.right / 2, 0) == s.textures.color(TEAL);
                checks[3] &= SendMessageW(list, LB_GETTOPINDEX, 0, 0) == top
                    && SendMessageW(list, LB_GETCURSEL, 0, 0) == selected;
                observations.push(json!({"dpi":dpi,"id":id,"effects":effects,"rounding":rounding,"frame":frame,"region":has_shape}));
                if rounded {
                    screenshot(s.hwnd, &s.root.join(format!("native-list-{id}-{dpi}.bmp")))?;
                }
            }
        }
        s.page = 1;
        s.scroll = 0;
        for rounding in [true, false] {
            s.locale.design.effects = true;
            s.locale.design.rounding = rounding;
            s.refresh_design();
            let combo = s.controls[&3101];
            SendMessageW(combo, CB_SHOWDROPDOWN, 1, 0);
            let mut info = COMBOBOXINFO::default();
            info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
            let shape = CreateRectRgn(0, 0, 0, 0);
            checks[5] &= GetComboBoxInfo(combo, &mut info) != 0
                && (GetWindowRgn(info.hwndList, shape) != 0) == rounding
                && (!rounding || PtInRegion(shape, 0, 0) == 0);
            DeleteObject(shape);
            SendMessageW(combo, CB_SHOWDROPDOWN, 0, 0);
        }
    }
    s.locale.design = crate::design::Design::default();
    s.set_dpi_fonts(96);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    s.page = 2;
    s.draft["presets"] = json!([]);
    s.selected_preset = None;
    s.populate_presets();
    s.update_buttons();
    s.refresh_design();
    let list = s.controls[&3301];
    let mut bounds = RECT::default();
    GetClientRect(list, &mut bounds);
    let mut snapshot = PaintBuffer::default();
    snapshot.ensure(s.buffer_dc, (bounds.right, bounds.bottom));
    SendMessageW(
        list,
        WM_PRINTCLIENT,
        snapshot.dc as usize,
        PRF_CLIENT as isize,
    );
    let mut hint_pixels = 0;
    for y in 12..74.min(bounds.bottom) {
        for x in 12..(bounds.right - 12) {
            hint_pixels += (GetPixel(snapshot.dc, x, y) == s.textures.color(MUTED)) as usize;
        }
    }
    checks[4] &= hint_pixels > 40
        && IsWindowEnabled(s.controls[&3314]) == 0
        && IsWindowEnabled(s.controls[&3106]) == 0;
    screenshot(s.hwnd, &s.root.join("native-list-empty.bmp"))?;
    std::fs::write(
        s.root.join("native-list-observations.json"),
        serde_json::to_vec_pretty(&observations).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        s.root.join("native-list-checks.json"),
        serde_json::to_vec_pretty(&json!({
            "lists_have_stepped_frames_at_all_dpi":checks[0],
            "rounding_and_effects_switches_restore_square_fields":checks[1],
            "keyboard_focus_is_visible":checks[2],
            "painting_preserves_selection_and_scroll":checks[3],
            "empty_list_hint_is_visible_and_actions_disabled":checks[4],
            "dropdown_corners_follow_preferences_at_all_dpi":checks[5]
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    (
        s.model,
        s.draft,
        s.selected_preset,
        s.page,
        s.scroll,
        _,
        s.locale.design,
    ) = original.clone();
    s.set_dpi_fonts(original.5);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    SetFocus(s.hwnd);
    s.populate_profiles();
    s.populate_presets();
    s.update_buttons();
    s.refresh_design();
    Ok(checks)
}

// Observe messages, not the (already validated) update region after a frame.
// This catches synchronous repaints that GetUpdateRect cannot detect.
#[derive(Default)]
struct AnimationPaintProbe {
    paints: Cell<u32>,
    erases: Cell<u32>,
}
unsafe extern "system" fn animation_paint_probe(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    data: usize,
) -> isize {
    let probe = &*(data as *const AnimationPaintProbe);
    if message == WM_PAINT {
        probe.paints.set(probe.paints.get() + 1);
    } else if message == WM_ERASEBKGND {
        probe.erases.set(probe.erases.get() + 1);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn smoke_animation_isolation(s: &mut State) -> Result<bool, String> {
    let mut reports = Vec::new();
    let mut passed = true;
    let original_dpi = s.dpi;
    let mut original_bounds = RECT::default();
    GetWindowRect(s.hwnd, &mut original_bounds);
    // Settings, editors, lists, and the hero all share the animated canvas.
    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        for page in 0..4 {
            s.page = page;
            s.scroll = 0;
            s.layout();
            SetFocus(s.hwnd);
            s.sync_background();
            RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
            let probes: Vec<_> = s
                .controls
                .iter()
                .filter(|(_, hwnd)| IsWindowVisible(**hwnd) != 0)
                .map(|(&id, &hwnd)| (id, hwnd, Box::<AnimationPaintProbe>::default()))
                .collect();
            for (_, hwnd, probe) in &probes {
                passed &= SetWindowSubclass(
                    *hwnd,
                    Some(animation_paint_probe),
                    90,
                    &**probe as *const AnimationPaintProbe as usize,
                ) != 0;
            }
            let layouts = s.layout_passes.get();
            let moves = s.geometry_moves.get();
            let paints = s.parent_paints.get();
            let frames = s.textures.background_frames;
            for _ in 0..12 {
                // Windows discards damage outside the desktop. Internal paints
                // exercise the real handlers there without invalidating opaque
                // controls or making a test window appear on the user's screen.
                RedrawWindow(
                    s.hwnd,
                    null(),
                    null_mut(),
                    RDW_INTERNALPAINT | RDW_NOCHILDREN,
                );
                for (id, hwnd, _) in &probes {
                    if [
                        200, 201, 202, 203, 204, 205, 1007, 1011, 1010, 1024, 2110, 3010, 3307,
                    ]
                    .contains(id)
                    {
                        RedrawWindow(
                            *hwnd,
                            null(),
                            null_mut(),
                            RDW_INTERNALPAINT | RDW_NOCHILDREN,
                        );
                    }
                }
                s.background_since = Instant::now() - std::time::Duration::from_millis(50);
                SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
                // Include paints deferred until the composited DC was released,
                // just as the normal message loop would. Bound a paint storm.
                for pending in 0..256 {
                    let mut message = MSG::default();
                    if PeekMessageW(&mut message, null_mut(), WM_PAINT, WM_PAINT, PM_REMOVE) == 0 {
                        break;
                    }
                    passed &= pending < 255;
                    DispatchMessageW(&message);
                }
            }
            for (id, hwnd, probe) in probes {
                RemoveWindowSubclass(hwnd, Some(animation_paint_probe), 90);
                let animated = [
                    200, 201, 202, 203, 204, 205, 1007, 1011, 1010, 1024, 2110, 3010, 3307,
                ]
                .contains(&id);
                passed &=
                    probe.erases.get() == 0 && probe.paints.get() == if animated { 12 } else { 0 };
                reports.push(
                    json!({"dpi":dpi, "page":page, "control":id, "animated":animated,
                "paints":probe.paints.get(), "erases":probe.erases.get()}),
                );
            }
            passed &= s.layout_passes.get() == layouts
                && s.geometry_moves.get() == moves
                && s.parent_paints.get() == paints + 12
                && s.textures.background_frames == frames + 12;
        }
    }
    s.set_dpi_fonts(original_dpi);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        original_bounds.left,
        original_bounds.top,
        original_bounds.right - original_bounds.left,
        original_bounds.bottom - original_bounds.top,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    std::fs::write(
        s.root.join("native-animation-isolation.json"),
        serde_json::to_vec_pretty(&reports).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(passed)
}

unsafe fn smoke_render_stability(s: &mut State) -> Result<[bool; 11], String> {
    let original_appearance = s.appearance_open;
    s.appearance_open = false;
    SetFocus(s.hwnd);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_RESUME_TIMER, 0);
    let original = (
        s.page,
        s.scroll,
        s.dpi,
        s.app_active,
        s.background_enabled,
        s.locale.design,
        s.background_millis,
    );
    let mut checks = [true; 11];
    s.locale.set_design(crate::design::Design::default())?;
    s.background_enabled = true;
    s.app_active = true;
    s.page = 3;
    s.scroll = 0;
    s.refresh_design();
    s.populate_profiles();
    s.populate_preset_library();
    checks[0] &= IsWindowVisible(s.controls[&1001]) == 0 && IsWindowVisible(s.controls[&3301]) == 0;

    let millis = s.background_millis;
    let frames = s.textures.background_frames;
    SendMessageW(s.hwnd, WM_ACTIVATEAPP, 0, 0);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
    checks[1] &= !s.background_running
        && s.background_millis == millis
        && s.textures.background_frames == frames;
    SendMessageW(s.hwnd, WM_ACTIVATEAPP, 1, 0);
    checks[1] &= s.background_running;
    SendMessageW(s.hwnd, WM_ENTERSIZEMOVE, 0, 0);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
    checks[1] &= !s.background_running && s.background_millis == millis;
    SendMessageW(s.hwnd, WM_EXITSIZEMOVE, 0, 0);
    checks[1] &= s.background_running;

    s.page = 1;
    s.scroll = 0;
    s.layout();
    let phase = s.background_millis;
    SetFocus(s.controls[&2102]);
    SendMessageW(s.hwnd, BACKGROUND_ACTIVITY, 0, 0);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
    checks[6] &= !s.background_running && s.background_millis == phase;
    SetFocus(s.hwnd);
    SendMessageW(s.hwnd, BACKGROUND_ACTIVITY, 0, 0);
    checks[6] &= s.background_running && s.background_millis == phase;
    SendMessageW(s.hwnd, WM_MOUSEWHEEL, (120u32 << 16) as usize, 0);
    checks[6] &= !s.background_running;
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_RESUME_TIMER, 0);
    checks[6] &= s.background_running && s.background_millis == phase;

    s.page = 3;
    s.appearance_open = true;
    s.scroll = 588 + APPEARANCE_EXTRA_HEIGHT;
    s.layout();
    let slider = s.controls[&4024];
    let draft = s.draft.clone();
    checks[7] &= IsWindowVisible(slider) != 0 && SendMessageW(slider, TBM_GETPOS, 0, 0) == 100;
    for speed in [25, 100, 200] {
        SendMessageW(slider, TBM_SETPOS, 1, speed);
        SendMessageW(s.hwnd, WM_HSCROLL, TB_THUMBTRACK as usize, slider as isize);
        checks[7] &= s.background_speed() == speed as u32;
        let phase = s.background_millis;
        s.background_since = Instant::now() - std::time::Duration::from_millis(100);
        SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
        let delta = (s.background_millis + crate::texture::FLOW_PERIOD_MS - phase)
            % crate::texture::FLOW_PERIOD_MS;
        checks[7] &= (speed as u32..=speed as u32 + 20).contains(&delta);
        SendMessageW(s.hwnd, WM_HSCROLL, TB_ENDTRACK as usize, slider as isize);
        checks[7] &= Locale::new(&s.root, true)?.design.animation_speed == speed as u32;
    }
    // Native keyboard behavior and drag preview must use the same saved value.
    SetFocus(slider);
    SendMessageW(slider, WM_KEYDOWN, VK_HOME as usize, 0);
    SendMessageW(slider, WM_KEYUP, VK_HOME as usize, 0);
    checks[7] &= s.background_speed() == 25;
    SendMessageW(slider, WM_KEYDOWN, VK_RIGHT as usize, 0);
    SendMessageW(slider, WM_KEYUP, VK_RIGHT as usize, 0);
    SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_SPEED_SAVE_TIMER, 0);
    checks[7] &=
        s.background_speed() == 30 && Locale::new(&s.root, true)?.design.animation_speed == 30;
    let (mut thumb, mut slider_rect) = (RECT::default(), RECT::default());
    GetClientRect(slider, &mut slider_rect);
    SendMessageW(slider, TBM_GETTHUMBRECT, 0, &mut thumb as *mut _ as isize);
    let mouse_point =
        |x: i32| ((((thumb.top + thumb.bottom) / 2) as u32) << 16 | x as u32) as isize;
    SendMessageW(
        slider,
        WM_LBUTTONDOWN,
        1,
        mouse_point((thumb.left + thumb.right) / 2),
    );
    SendMessageW(slider, WM_MOUSEMOVE, 1, mouse_point(slider_rect.right - 2));
    SendMessageW(slider, WM_LBUTTONUP, 0, mouse_point(slider_rect.right - 2));
    checks[7] &=
        s.background_speed() == 200 && Locale::new(&s.root, true)?.design.animation_speed == 200;
    SendMessageW(s.hwnd, WM_COMMAND, 4021, 0);
    checks[7] &= IsWindowEnabled(slider) == 0 && !s.background_running;
    SendMessageW(s.hwnd, WM_COMMAND, 4021, 0);
    checks[7] &= IsWindowEnabled(slider) != 0 && s.background_speed() == 200 && s.draft == draft;
    s.locale.set_design(crate::design::Design::default())?;
    SetFocus(s.hwnd);
    s.refresh_design();

    for dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(dpi);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        s.page = 3;
        s.appearance_open = true;
        s.scroll = 588 + APPEARANCE_EXTRA_HEIGHT;
        s.layout();
        let (mut thumb, mut rect) = (RECT::default(), RECT::default());
        GetClientRect(slider, &mut rect);
        SendMessageW(slider, TBM_GETTHUMBRECT, 0, &mut thumb as *mut _ as isize);
        let source = GetDC(slider);
        let dc = CreateCompatibleDC(source);
        let bitmap = CreateCompatibleBitmap(source, rect.right, rect.bottom);
        let old = SelectObject(dc, bitmap);
        SendMessageW(slider, WM_PRINT, dc as usize, PRF_CLIENT as isize);
        checks[7] &= thumb.right > thumb.left
            && thumb.left > rect.right / 4
            && thumb.bottom <= rect.bottom
            && GetPixel(
                dc,
                (thumb.left + thumb.right) / 2,
                (thumb.top + thumb.bottom) / 2,
            ) == TEAL;
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ReleaseDC(slider, source);
        screenshot(
            s.hwnd,
            &s.root.join(format!("native-design-speed-{dpi}.bmp")),
        )?;
        let captured = std::fs::read(s.root.join(format!("native-design-speed-{dpi}.bmp")))
            .map_err(|e| e.to_string())?;
        let bounds = s.control_bounds.borrow()[&4024];
        let x = bounds.0 + (thumb.left + thumb.right) / 2;
        let y = bounds.1 + (thumb.top + thumb.bottom) / 2;
        let offset = 54 + ((y * s.d(HOME_WIDTH) + x) * 4) as usize;
        let captured_color = captured[offset + 2] as u32
            | (captured[offset + 1] as u32) << 8
            | (captured[offset] as u32) << 16;
        checks[7] &= captured_color == TEAL;
        s.appearance_open = false;
        s.scroll = 130; // first settings row straddles the fixed header
        s.layout();
        let control = s.controls[&4002];
        let mut rect = RECT::default();
        GetClientRect(control, &mut rect);
        let shape = CreateRectRgn(0, 0, 0, 0);
        let mut clip = RECT::default();
        checks[2] &= IsWindowVisible(control) != 0 && GetWindowRgn(control, shape) != 0;
        GetRgnBox(shape, &mut clip);
        checks[2] &= rect.bottom == s.d(42)
            && clip.top == s.d(88) - s.d(76)
            && PtInRegion(shape, rect.right / 2, clip.top) != 0
            && PtInRegion(shape, rect.right / 2, clip.top - 1) == 0;
        DeleteObject(shape);
        // Increasing viewport height must also update unchanged control bounds.
        s.appearance_open = true;
        s.scroll = 330 + APPEARANCE_EXTRA_HEIGHT;
        s.layout();
        let bottom_row = s.controls[&4023];
        let region = CreateRectRgn(0, 0, 0, 0);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(900),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        GetWindowRgn(bottom_row, region);
        checks[2] &= IsWindowVisible(bottom_row) != 0 && PtInRegion(region, s.d(100), s.d(40)) != 0;
        DeleteObject(region);
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(HOME_HEIGHT),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        s.appearance_open = false;
        s.scroll = 130;
        s.layout();
        RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
        s.background_since = Instant::now() - std::time::Duration::from_millis(50);
        SendMessageW(s.hwnd, WM_TIMER, BACKGROUND_TIMER, 0);
        let mut dirty = RECT::default();
        let retained = GetUpdateRect(s.hwnd, &mut dirty, 0) != 0;
        let mut window = RECT::default();
        GetWindowRect(s.hwnd, &mut window);
        checks[3] &= if retained {
            dirty.top == 0 && dirty.bottom == s.d(HOME_HEIGHT)
        } else {
            // As in the hero invalidation test, Win32 can discard partial
            // damage for a shaped window entirely outside the desktop.
            window.right < 0 && s.background_running
        };
        checks[3] &= GetUpdateRect(s.controls[&4002], null_mut(), 0) == 0;
        UpdateWindow(s.hwnd);
        screenshot(
            s.hwnd,
            &s.root.join(format!("native-scroll-clipped-{dpi}.bmp")),
        )?;
        s.page = 1;
        s.scroll = 0;
        s.layout();
        let editor = s.controls[&2102];
        let value = s.text(2102);
        SendMessageW(editor, EM_SETSEL, 1, 3);
        let mut before = RECT::default();
        GetClientRect(editor, &mut before);
        s.scroll = 24;
        s.layout();
        let mut after = RECT::default();
        let (mut start, mut end) = (0u32, 0u32);
        GetClientRect(editor, &mut after);
        SendMessageW(
            editor,
            EM_GETSEL,
            &mut start as *mut _ as usize,
            &mut end as *mut _ as isize,
        );
        checks[4] &= before.bottom == after.bottom
            && s.text(2102) == value
            && start == (value.encode_utf16().count().min(1) as u32)
            && end == (value.encode_utf16().count().min(3) as u32);
        // Inspect native EDIT output as well as its actual HWND shape. A
        // rectangular stock paint must not erase the stepped outline.
        s.scroll = 0;
        s.layout();
        s.prepare_background();
        for id in [2102, 2105] {
            let input = s.controls[&id];
            let mut bounds = RECT::default();
            GetClientRect(input, &mut bounds);
            let mut snapshot = PaintBuffer::default();
            checks[9] &= snapshot.ensure(s.buffer_dc, (bounds.right, bounds.bottom));
            SendMessageW(input, WM_PRINT, snapshot.dc as usize, PRF_CLIENT as isize);
            let u = crate::design::corner_unit(dpi).min(bounds.bottom / 12);
            let shape = CreateRectRgn(0, 0, 0, 0);
            checks[9] &= GetWindowRgn(input, shape) != 0
                && PtInRegion(shape, 0, 0) == 0
                && PtInRegion(shape, 3 * u, 0) != 0
                && GetPixel(snapshot.dc, 3 * u, 0) == LINE
                && GetPixel(snapshot.dc, 2 * u, u) == LINE
                && GetPixel(snapshot.dc, u, 2 * u) == LINE
                && GetPixel(snapshot.dc, 0, 3 * u) == LINE
                && SendMessageW(input, WM_ERASEBKGND, snapshot.dc as usize, 0) == 1;
            DeleteObject(shape);
        }
        s.page = 3;
        s.scroll = 200;
        s.layout();
        let (width, visible, content) = s.dimensions();
        let track = page_scroll_track(visible, content, s.scroll);
        InvalidateRect(s.hwnd, null(), 0);
        UpdateWindow(s.hwnd);
        let mut reference = PaintBuffer::default();
        checks[8] &= reference.ensure(s.buffer_dc, (s.d(width), s.d(visible)));
        surface_fill(
            s,
            reference.dc,
            &RECT {
                left: 0,
                top: 0,
                right: s.d(width),
                bottom: s.d(visible),
            },
            BG,
        );
        for (x, y) in [
            (8, 50),
            (width - 20, 80),
            (8, visible - 30),
            (width - 20, visible - 3),
        ] {
            checks[8] &=
                GetPixel(s.buffer_dc, s.d(x), s.d(y)) == GetPixel(reference.dc, s.d(x), s.d(y));
        }
        checks[10] &= track.top >= 88 && track.top + track.height <= visible - NAV_HEIGHT - NAV_GAP;
        for x in s.d(width - 9)..s.d(width - 4) {
            let y = s.d(track.position + track.thumb / 2);
            checks[10] &= GetPixel(s.buffer_dc, x, y) == crate::texture::sample(INDIGO, dpi, x, y);
        }
        let point = |y: i32| ((s.d(y) as u32) << 16 | s.d(width - 6) as u32) as isize;
        SendMessageW(
            s.hwnd,
            WM_LBUTTONDOWN,
            1,
            point(track.position + track.thumb / 2),
        );
        checks[5] &= s.scroll == 200 && s.scroll_dragging;
        SendMessageW(
            s.hwnd,
            WM_MOUSEMOVE,
            1,
            point(track.position + track.thumb / 2 + 20),
        );
        checks[5] &= s.scroll > 200;
        SendMessageW(s.hwnd, WM_MOUSEMOVE, 1, point(visible));
        checks[5] &= s.scroll == track.range;
        SendMessageW(s.hwnd, WM_LBUTTONUP, 0, point(visible));
        checks[5] &= !s.scroll_dragging;
    }
    s.set_dpi_fonts(original.2);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    checks[3] &= smoke_animation_isolation(s)?;
    s.page = original.0;
    s.scroll = original.1;
    s.app_active = original.3;
    s.background_enabled = original.4;
    s.locale.set_design(original.5)?;
    s.appearance_open = original_appearance;
    s.background_millis = original.6;
    s.refresh_design();
    Ok(checks)
}

// Smoke-only bitmap assertions keep expensive pixel sampling out of normal
// rendering. Caption glyphs must stay block based at all supported DPI scales.
unsafe fn smoke_pixel_surfaces(s: &mut State) -> (bool, bool) {
    let dc = CreateCompatibleDC(s.buffer_dc);
    if dc.is_null() {
        return (false, false);
    }
    let bitmap = CreateCompatibleBitmap(s.buffer_dc, 160, 160);
    if bitmap.is_null() {
        DeleteDC(dc);
        return (false, false);
    }
    let old = SelectObject(dc, bitmap);
    let solid = CreateSolidBrush(BG);
    let mut caption_blocks = true;
    for dpi in [96, 120, 144, 192] {
        let side = 28 * dpi / 96;
        let rect = RECT {
            left: 0,
            top: 0,
            right: side,
            bottom: side,
        };
        let mut sizes = Vec::new();
        for close in [true, false] {
            FillRect(dc, &rect, solid);
            caption_glyph(dc, &rect, dpi, InterfaceSize::Medium, close, MUTED);
            let (mut left, mut top, mut right, mut bottom) = (side, side, -1, -1);
            let mut ink = 0;
            for y in 0..side {
                for x in 0..side {
                    let color = GetPixel(dc, x, y);
                    caption_blocks &= color == BG || color == MUTED;
                    if color == MUTED {
                        left = left.min(x);
                        top = top.min(y);
                        right = right.max(x);
                        bottom = bottom.max(y);
                        ink += 1;
                    }
                }
            }
            let (width, height) = (right - left + 1, bottom - top + 1);
            caption_blocks &= width >= 16
                && width % 8 == 0
                && left >= 0
                && top >= 0
                && right < side
                && bottom < side;
            if close {
                caption_blocks &= width == height && ink > width * width / 4 && ink < width * width;
            }
            sizes.push((width, height));
        }
        caption_blocks &= sizes[0].0 == sizes[1].0 && sizes[1].1 * 4 == sizes[0].1;
    }
    DeleteObject(solid);
    let rect = RECT {
        left: 0,
        top: 0,
        right: 160,
        bottom: 160,
    };
    let mut texture_cache = true;
    let mut handles = HashMap::new();
    // Warm actual fills first: Windows lazily initializes native brush caches.
    for dpi in [96, 120, 144, 192] {
        s.textures.set_dpi(dpi);
        for color in [BG, PANEL, PANEL_ALT, LINE, INDIGO] {
            s.textures.fill(dc, &rect, color);
        }
    }
    GdiFlush();
    let resources = GetGuiResources(GetCurrentProcess(), 0);
    for _ in 0..12 {
        for dpi in [96, 120, 144, 192] {
            s.textures.set_dpi(dpi);
            for color in [BG, PANEL, PANEL_ALT, LINE, INDIGO] {
                let brush = s.textures.brush(color);
                if let Some(previous) = handles.insert((dpi, color), brush) {
                    texture_cache &= previous == brush;
                }
                SetBrushOrgEx(dc, 0, 0, null_mut());
                s.textures.fill(dc, &rect, color);
                for (x, y) in [(0, 0), (7, 9), (63, 63), (110, 139)] {
                    texture_cache &= GetPixel(dc, x, y) == crate::texture::sample(color, dpi, x, y);
                }
            }
        }
    }
    GdiFlush();
    texture_cache &= GetGuiResources(GetCurrentProcess(), 0) == resources;
    s.textures.set_dpi(s.dpi);
    surface_fill(s, dc, &rect, BG);
    let mut tones = std::collections::HashSet::new();
    for y in (0..160).step_by(4) {
        for x in (0..160).step_by(4) {
            let pixel = GetPixel(dc, x, y);
            tones.insert(pixel);
            texture_cache &= pixel == crate::texture::sample(BG, s.dpi, x, y);
        }
    }
    texture_cache &= tones.len() > 1
        && s.paper == s.textures.brush(BG)
        && s.white == s.textures.brush(PANEL_ALT);
    SelectObject(dc, old);
    DeleteObject(bitmap);
    DeleteDC(dc);
    (caption_blocks, texture_cache)
}
unsafe extern "system" fn profile_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    if message == WM_MOUSEWHEEL {
        SendMessageW(
            GetParent(hwnd),
            LIST_SCROLL,
            wparam,
            GetDlgCtrlID(hwnd) as isize,
        );
        return 0;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(profile_procedure), 1);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if matches!(message, WM_KEYDOWN | WM_VSCROLL | WM_LBUTTONDOWN) {
        let bar = GetDlgItem(GetParent(hwnd), scrollbar_id(GetDlgCtrlID(hwnd)));
        if !bar.is_null() {
            InvalidateRect(bar, null(), 0);
        }
    }
    result
}
unsafe extern "system" fn scrollbar_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    let id = GetDlgCtrlID(hwnd) as usize;
    let y = (lparam as u32 >> 16) as i16 as i32;
    if message == WM_LBUTTONDOWN {
        SetCapture(hwnd);
        SetFocus(GetDlgItem(GetParent(hwnd), scrollbar_target(id as i32)));
        SendMessageW(GetParent(hwnd), SCROLL_DRAG, id, y as isize);
        return 0;
    }
    if message == WM_MOUSEMOVE && GetCapture() == hwnd {
        SendMessageW(GetParent(hwnd), SCROLL_DRAG, id | (1 << 16), y as isize);
        return 0;
    }
    if message == WM_LBUTTONUP && GetCapture() == hwnd {
        ReleaseCapture();
        SendMessageW(GetParent(hwnd), SCROLL_DRAG, id | (2 << 16), y as isize);
        return 0;
    }
    if message == WM_MOUSEWHEEL {
        SendMessageW(
            GetParent(hwnd),
            LIST_SCROLL,
            wparam,
            scrollbar_target(id as i32) as isize,
        );
        return 0;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(scrollbar_procedure), 2);
    }
    if message == WM_CAPTURECHANGED {
        SendMessageW(GetParent(hwnd), SCROLL_DRAG, id | (2 << 16), 0);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
unsafe extern "system" fn navigation_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    data: usize,
) -> isize {
    let s = &mut *(data as *mut State);
    let id = GetDlgCtrlID(hwnd);
    if message == WM_MOUSEMOVE && s.nav_hover != Some(id) {
        if let Some(previous) = s.nav_hover.replace(id) {
            InvalidateRect(s.controls[&previous], null(), 0);
        }
        InvalidateRect(hwnd, null(), 0);
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut tracking);
    } else if message == WM_MOUSELEAVE && s.nav_hover == Some(id) {
        s.nav_hover = None;
        InvalidateRect(hwnd, null(), 0);
    } else if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(navigation_procedure), 3);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
unsafe fn paint_speed(hwnd: HWND, dc: HDC) {
    let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return;
    }
    let s = &*pointer;
    let (mut rect, mut channel, mut thumb) = (RECT::default(), RECT::default(), RECT::default());
    GetClientRect(hwnd, &mut rect);
    SendMessageW(hwnd, TBM_GETCHANNELRECT, 0, &mut channel as *mut _ as isize);
    SendMessageW(hwnd, TBM_GETTHUMBRECT, 0, &mut thumb as *mut _ as isize);
    FillRect(dc, &rect, s.textures.brush(PANEL));
    let enabled = IsWindowEnabled(hwnd) != 0;
    let center = (thumb.top + thumb.bottom) / 2;
    channel.top = center - s.d(2);
    channel.bottom = center + s.d(2);
    FillRect(dc, &channel, s.textures.brush(LINE));
    channel.right = (thumb.left + thumb.right) / 2;
    FillRect(
        dc,
        &channel,
        s.textures.brush(if enabled { TEAL } else { MUTED }),
    );
    // Use the native thumb rectangle for both painting and pointer hit testing.
    // Solid integer blocks also work when PrintWindow changes the DC origin.
    FillRect(
        dc,
        &thumb,
        s.textures
            .brush(if GetFocus() == hwnd { INK } else { LINE }),
    );
    InflateRect(&mut thumb, -s.d(2), -s.d(2));
    FillRect(
        dc,
        &thumb,
        s.textures.brush(if enabled { TEAL } else { MUTED }),
    );
}
unsafe extern "system" fn speed_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let dc = if wparam == 0 {
            BeginPaint(hwnd, &mut paint)
        } else {
            wparam as HDC
        };
        let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        if !pointer.is_null()
            && (*pointer)
                .speed_buffer
                .ensure(dc, (rect.right, rect.bottom))
        {
            let target = (*pointer).speed_buffer.dc;
            paint_speed(hwnd, target);
            BitBlt(dc, 0, 0, rect.right, rect.bottom, target, 0, 0, SRCCOPY);
        } else {
            paint_speed(hwnd, dc);
        }
        if wparam == 0 {
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    if matches!(message, WM_PRINT | WM_PRINTCLIENT) {
        paint_speed(hwnd, wparam as HDC);
        return 0;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(speed_procedure), 7);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
unsafe extern "system" fn buffered_control_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    // WM_DRAWITEM presents a complete buffer, including its background.
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(buffered_control_procedure), 8);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
unsafe extern "system" fn hero_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    if message == WM_ERASEBKGND {
        return 1;
    }
    if matches!(message, WM_MOUSEMOVE | WM_MOUSELEAVE) {
        let state = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
        if !state.is_null() {
            let s = &mut *state;
            let hovered = message == WM_MOUSEMOVE;
            if s.hero_hover != hovered {
                s.hero_hover = hovered;
                InvalidateRect(hwnd, null(), 0);
                if hovered {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut track);
                }
            }
        }
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(hero_procedure), 4);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
unsafe fn edit_font_metrics(hwnd: HWND) -> TEXTMETRICW {
    let mut metrics: TEXTMETRICW = std::mem::zeroed();
    let dc = GetDC(hwnd);
    let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
    let original = SelectObject(dc, font);
    GetTextMetricsW(dc, &mut metrics);
    SelectObject(dc, original);
    ReleaseDC(hwnd, dc);
    metrics
}
unsafe fn edit_cap_metrics(hwnd: HWND) -> (TEXTMETRICW, i32, i32) {
    let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
    let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
    if !pointer.is_null()
        && let Some(result) = (*pointer).edit_ink_cache.borrow().get(&(font as usize))
    {
        return *result;
    }
    let mut metrics: TEXTMETRICW = std::mem::zeroed();
    let mut glyph: GLYPHMETRICS = std::mem::zeroed();
    let mut matrix: MAT2 = std::mem::zeroed();
    matrix.eM11.value = 1;
    matrix.eM22.value = 1;
    let dc = GetDC(hwnd);
    let original = SelectObject(dc, font);
    GetTextMetricsW(dc, &mut metrics);
    let result = GetGlyphOutlineW(
        dc,
        b'i' as u32,
        GGO_BITMAP,
        &mut glyph,
        0,
        null_mut(),
        &matrix,
    );
    let mut cap = (
        metrics.tmInternalLeading,
        metrics.tmHeight - metrics.tmInternalLeading,
    );
    if result != GDI_ERROR as u32 && result > 0 && result <= 4096 && glyph.gmBlackBoxY > 0 {
        let mut pixels = vec![0u8; result as usize];
        if GetGlyphOutlineW(
            dc,
            b'i' as u32,
            GGO_BITMAP,
            &mut glyph,
            result,
            pixels.as_mut_ptr().cast(),
            &matrix,
        ) != GDI_ERROR as u32
        {
            let stride = (glyph.gmBlackBoxX as usize).div_ceil(32) * 4;
            let rows: Vec<_> = pixels
                .chunks(stride.max(1))
                .enumerate()
                .filter(|(_, row)| {
                    (0..glyph.gmBlackBoxX as usize).any(|x| row[x / 8] & (0x80 >> (x % 8)) != 0)
                })
                .map(|(row, _)| row as i32)
                .collect();
            if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
                cap = (
                    metrics.tmAscent - glyph.gmptGlyphOrigin.y + first,
                    last - first + 1,
                );
            }
        }
    }
    SelectObject(dc, original);
    ReleaseDC(hwnd, dc);
    let result = (metrics, cap.0, cap.1);
    if !pointer.is_null() {
        (*pointer)
            .edit_ink_cache
            .borrow_mut()
            .insert(font as usize, result);
    }
    result
}
unsafe fn format_edit(hwnd: HWND, dpi: i32, _frame: bool) {
    let margin = (10 * dpi / 96).max(1);
    let margins = margin | margin << 16;
    if SendMessageW(hwnd, EM_GETMARGINS, 0, 0) != margins as isize {
        SendMessageW(
            hwnd,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            margins as isize,
        );
    }
    let mut client = RECT::default();
    let mut previous = RECT::default();
    GetClientRect(hwnd, &mut client);
    SendMessageW(hwnd, EM_GETRECT, 0, &mut previous as *mut _ as isize);
    let multiline = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & ES_WANTRETURN as u32 != 0;
    let (metrics, cap_top, cap_height) = edit_cap_metrics(hwnd);
    let top = if multiline {
        margin.min(client.bottom / 2)
    } else {
        ((client.bottom - cap_height) / 2 - cap_top).max(0)
    };
    let rect = RECT {
        left: margin.min(client.right / 2),
        top,
        right: (client.right - margin).max(margin + 1),
        bottom: if multiline {
            (client.bottom - margin).max(top + 1)
        } else {
            (top + metrics.tmHeight).min(client.bottom)
        },
    };
    if (previous.left, previous.top, previous.right, previous.bottom)
        != (rect.left, rect.top, rect.right, rect.bottom)
    {
        let line = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        SendMessageW(hwnd, EM_SETRECTNP, 0, &rect as *const _ as isize);
        let after = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        if line != after {
            SendMessageW(hwnd, EM_LINESCROLL, 0, line - after);
        }
    }
}
unsafe extern "system" fn combo_list_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    combo: usize,
) -> isize {
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(combo_list_procedure), 10);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if message == WM_WINDOWPOSCHANGED {
        let pointer = GetWindowLongPtrW(GetParent(combo as HWND), GWLP_USERDATA) as *mut State;
        if !pointer.is_null() {
            let s = &*pointer;
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect);
            let rounded = s.locale.design.effects && s.locale.design.rounding;
            let current = CreateRectRgn(0, 0, 0, 0);
            let has_shape = GetWindowRgn(hwnd, current) != 0;
            let desired = if rounded {
                crate::design::region(
                    RECT {
                        left: 0,
                        top: 0,
                        right: rect.right - rect.left,
                        bottom: rect.bottom - rect.top,
                    },
                    crate::design::corner_unit(s.dpi),
                )
            } else {
                null_mut()
            };
            // SetWindowRgn itself sends WM_WINDOWPOSCHANGED. Compare first to
            // avoid recursive region changes and reapply at each popup size.
            let changed = if rounded {
                !has_shape || EqualRgn(current, desired) == 0
            } else {
                has_shape
            };
            DeleteObject(current);
            if changed {
                if SetWindowRgn(hwnd, desired, 1) == 0 && !desired.is_null() {
                    DeleteObject(desired);
                }
            } else if !desired.is_null() {
                DeleteObject(desired);
            }
        }
    }
    result
}

unsafe extern "system" fn combo_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefSubclassProc(hwnd, message, wparam, lparam);
    }
    let s = &*pointer;
    if matches!(message, WM_NCCALCSIZE | WM_NCPAINT) {
        return 0;
    }
    if message == WM_ERASEBKGND {
        return 1;
    }
    if matches!(message, WM_PAINT | WM_PRINT | WM_PRINTCLIENT) {
        let mut paint = std::mem::zeroed();
        let dc = if message == WM_PAINT {
            BeginPaint(hwnd, &mut paint)
        } else {
            wparam as HDC
        };
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r);
        let id = GetDlgCtrlID(hwnd);
        let origin = s
            .control_bounds
            .borrow()
            .get(&id)
            .copied()
            .unwrap_or_default();
        let saved = SaveDC(dc);
        SetBrushOrgEx(dc, -origin.0, -origin.1, null_mut());
        let focused = GetFocus() == hwnd;
        surface_fill(s, dc, &r, BG);
        pixel_panel(s, dc, r, PANEL_ALT, if focused { TEAL } else { PANEL_ALT });
        let (w, h) = (r.right * 96 / s.dpi, r.bottom * 96 / s.dpi);
        let mut bounds = RECT {
            left: s.d(10),
            top: s.d((h - 12) / 2),
            right: s.d(w - 32),
            bottom: r.bottom,
        };
        let old = SelectObject(dc, s.fonts[0]);
        let mut metrics = std::mem::zeroed();
        GetTextMetricsW(dc, &mut metrics);
        bounds.top -= metrics.tmInternalLeading;
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            s.textures.color(if IsWindowEnabled(hwnd) != 0 {
                INK
            } else {
                MUTED
            }),
        );
        DrawTextW(
            dc,
            wide(&s.text(id)).as_ptr(),
            -1,
            &mut bounds,
            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(dc, old);
        let unit = ((2 * s.dpi + 48) / 96).max(1);
        let x = r.right - s.d(24);
        let y = (r.bottom - 4 * unit) / 2;
        for (row, line) in ["XX....XX", ".XX..XX.", "..XXXX..", "...XX..."]
            .iter()
            .enumerate()
        {
            for (col, bit) in line.bytes().enumerate() {
                if bit == b'X' {
                    FillRect(
                        dc,
                        &RECT {
                            left: x + col as i32 * unit,
                            top: y + row as i32 * unit,
                            right: x + (col as i32 + 1) * unit,
                            bottom: y + (row as i32 + 1) * unit,
                        },
                        s.textures.brush(MUTED),
                    );
                }
            }
        }
        RestoreDC(dc, saved);
        if message == WM_PAINT {
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(combo_procedure), 6);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if matches!(
        message,
        CB_SETCURSEL | WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE
    ) {
        InvalidateRect(hwnd, null(), 0);
    }
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS) {
        PostMessageW(GetParent(hwnd), BACKGROUND_ACTIVITY, 0, 0);
    }
    result
}
/// EDIT and LISTBOX share one complete, stepped surface. A window region alone
/// does not constrain native WM_PRINTCLIENT or erase/selection painting.
unsafe fn paint_native_field(hwnd: HWND, message: u32, wparam: usize, list: bool) -> Option<isize> {
    if !matches!(
        message,
        WM_ERASEBKGND | WM_PAINT | WM_PRINT | WM_PRINTCLIENT
    ) {
        return None;
    }
    let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return None;
    }
    if message == WM_ERASEBKGND {
        return Some(1);
    }
    let mut paint = PAINTSTRUCT::default();
    let dc = if message == WM_PAINT {
        BeginPaint(hwnd, &mut paint)
    } else {
        wparam as HDC
    };
    let s = &mut *pointer;
    let id = GetDlgCtrlID(hwnd);
    let mut rect = RECT::default();
    GetClientRect(hwnd, &mut rect);
    let mut buffer = s.control_buffers.remove(&id).unwrap_or_default();
    let buffered = buffer.ensure(dc, (rect.right, rect.bottom));
    let target = if buffered { buffer.dc } else { dc };
    let saved = SaveDC(target);
    let origin = s
        .control_bounds
        .borrow()
        .get(&id)
        .copied()
        .unwrap_or_default();
    SetBrushOrgEx(target, -origin.0, -origin.1, null_mut());
    surface_fill(s, target, &rect, BG);
    let shape = if s.locale.design.effects && s.locale.design.rounding {
        crate::design::region(rect, crate::design::corner_unit(s.dpi))
    } else {
        CreateRectRgn(0, 0, rect.right, rect.bottom)
    };
    let clip = SaveDC(target);
    ExtSelectClipRgn(target, shape, RGN_AND);
    FillRect(
        target,
        &rect,
        s.textures.brush(if list { PANEL } else { PANEL_ALT }),
    );
    // Native controls retain text, selection, caret and keyboard navigation.
    DefSubclassProc(hwnd, WM_PRINTCLIENT, target as usize, PRF_CLIENT as isize);
    if list && id == 3301 && SendMessageW(hwnd, LB_GETCOUNT, 0, 0) == 0 {
        text(
            s,
            target,
            "Пока нет наборов. Создайте свой или добавьте готовый.",
            12,
            14,
            rect.right * 96 / s.dpi - 24,
            60,
            3,
            MUTED,
        );
    }
    RestoreDC(target, clip);
    FrameRgn(
        target,
        shape,
        s.textures
            .brush(if GetFocus() == hwnd { TEAL } else { LINE }),
        s.d(1).max(1),
        s.d(1).max(1),
    );
    DeleteObject(shape);
    RestoreDC(target, saved);
    if buffered {
        // PrintWindow may omit the child's viewport clip. Do not publish rows
        // over the fixed header/footer when the page is partially scrolled.
        let published = SaveDC(dc);
        if list {
            IntersectClipRect(
                dc,
                0,
                (s.d(88) - origin.1).max(0),
                rect.right,
                (s.d(s.dimensions().1 - NAV_HEIGHT - NAV_GAP) - origin.1).min(rect.bottom),
            );
        }
        BitBlt(dc, 0, 0, rect.right, rect.bottom, target, 0, 0, SRCCOPY);
        RestoreDC(dc, published);
    }
    s.control_buffers.insert(id, buffer);
    if message == WM_PAINT {
        EndPaint(hwnd, &paint);
    }
    Some(0)
}

unsafe extern "system" fn list_frame_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    if let Some(result) = paint_native_field(hwnd, message, wparam, true) {
        return result;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(list_frame_procedure), 9);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if matches!(
        message,
        WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_ENABLE
            | WM_KEYDOWN
            | WM_LBUTTONDOWN
            | WM_VSCROLL
            | LB_SETCURSEL
            | LB_SETTOPINDEX
            | LB_RESETCONTENT
            | LB_ADDSTRING
            | LB_DELETESTRING
    ) {
        // Stock list updates can paint individual rows directly. Recompose the
        // complete surface so selection/scrolling cannot erase its outline.
        InvalidateRect(hwnd, null(), 0);
    }
    result
}

unsafe extern "system" fn edit_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    let pointer = GetWindowLongPtrW(GetParent(hwnd), GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefSubclassProc(hwnd, message, wparam, lparam);
    }
    if let Some(result) = paint_native_field(hwnd, message, wparam, false) {
        return result;
    }
    let s = &*pointer;
    let single = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & ES_WANTRETURN as u32 == 0;
    if single && message == WM_CHAR && matches!(wparam, 10 | 13) {
        return 0;
    }
    if matches!(GetDlgCtrlID(hwnd), 2105 | 3310) {
        if message == WM_GETDLGCODE
            && lparam != 0
            && (*(lparam as *const MSG)).wParam == VK_RETURN as usize
        {
            return DefSubclassProc(hwnd, message, wparam, lparam) | DLGC_WANTALLKEYS as isize;
        }
        if message == WM_KEYDOWN && wparam == VK_RETURN as usize {
            PostMessageW(
                GetParent(hwnd),
                WM_COMMAND,
                if GetDlgCtrlID(hwnd) == 2105 {
                    2106
                } else {
                    3312
                },
                0,
            );
            return 0;
        }
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(edit_procedure), 5);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if matches!(message, WM_SIZE | WM_SETFONT) {
        format_edit(hwnd, s.dpi, false);
    }
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS) {
        InvalidateRect(hwnd, null(), 0);
        PostMessageW(GetParent(hwnd), BACKGROUND_ACTIVITY, 0, 0);
    }
    if single && message == WM_PASTE {
        let id = GetDlgCtrlID(hwnd);
        let raw = s.text(id);
        let clean = raw.replace("\r\n", " ").replace(['\r', '\n'], " ");
        if clean != raw {
            s.set(id, &clean);
            SendMessageW(
                hwnd,
                EM_SETSEL,
                clean.encode_utf16().count(),
                clean.encode_utf16().count() as isize,
            );
        }
    }
    result
}
pub fn run(base: PathBuf, root: PathBuf, smoke: bool, started: Instant) -> Result<(), String> {
    unsafe {
        let startup_probe = !smoke
            && root.join("qa.marker").is_file()
            && std::env::args().any(|a| a == "--qa-startup");
        let startup = !smoke && std::env::args().any(|a| a == "--startup");
        let common = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        InitCommonControlsEx(&common);
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let identity = window_identity(&root)?;
        let _instance_guard = if smoke {
            None
        } else {
            match single_instance(&identity, !startup)? {
                Some(guard) => Some(guard),
                None => return Ok(()),
            }
        };
        let class = wide(&identity);
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(procedure);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hIcon = product_icon();
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let monitor_dpi = GetDpiForSystem() as i32;
        let locale = Locale::new(&root, smoke)?;
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            &mut work as *mut _ as *mut std::ffi::c_void,
            0,
        );
        let requested_dpi = locale.design.interface_size.dpi(monitor_dpi);
        let dpi = if smoke {
            requested_dpi
        } else {
            crate::appearance::fit_dpi(requested_dpi, work.right - work.left)
        };
        let _embedded_font = crate::font::EmbeddedFont::register()?;
        let fonts = make_fonts(dpi);
        let draft: Value = serde_json::from_slice(
            &std::fs::read(base.join("assets/settings.default.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let autostart_executable = if smoke || startup_probe {
            None
        } else {
            Some(crate::setup::installed(&root)?)
        };
        let autostart_enabled = autostart_executable
            .as_ref()
            .is_some_and(|exe| crate::autostart::enabled(exe).unwrap_or(false));
        let theme = locale.design.theme;
        let mut textures = crate::texture::Textures::with_theme(theme)?;
        textures.set_dpi(dpi);
        textures.set_design(locale.design);
        let paper = textures.brush(BG);
        let white = textures.brush(PANEL_ALT);
        let mut animations: i32 = 1;
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            &mut animations as *mut _ as *mut std::ffi::c_void,
            0,
        );
        let mut s = Box::new(State {
            base,
            root,
            hwnd: null_mut(),
            controls: HashMap::new(),
            captions: HashMap::new(),
            locale,
            autostart_executable,
            autostart_enabled,
            startup_pending: startup,
            startup_since: Instant::now(),
            page: 0,
            category: 0,
            selected_preset: None,
            preset_manager_open: false,
            dns_settings_open: false,
            appearance_open: false,
            model: json!({"profiles":[],"selected":"","running":false}),
            draft,
            loaded: false,
            busy: false,
            rename_open: false,
            import_open: false,
            import_error: None,
            imported_count: None,
            pending_action: String::new(),
            generation: 0,
            status_pending: false,
            status_revision: None,
            startup_probe: startup_probe.then_some(StartupProbe {
                started,
                window_ready_ms: 0.0,
            }),
            status_attempts: Cell::new(0),
            pending_id: None,
            pending_since: None,
            request_nonce: format!(
                "{:x}-{:x}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            checks: HashMap::new(),
            notice: String::new(),
            next: None,
            dpi,
            monitor_dpi,
            fonts,
            mushroom_brushes: make_mushroom_brushes(theme),
            product_icons: ProductIcons::new(theme, monitor_dpi)?,
            pulse_frame: 0,
            pulse_running: false,
            pulse_enabled: animations != 0,
            pulse_ticks: Cell::new(0),
            hero_motion: HeroMotion::default(),
            hero_hover: false,
            background_running: false,
            background_enabled: !smoke,
            app_active: smoke,
            resizing: false,
            background_input_pause: false,
            background_millis: 0,
            background_fraction: 0,
            background_speed_preview: None,
            background_since: Instant::now(),
            hero_draws: Cell::new(0),
            nav_hover: None,
            smoke_min_height: None,
            textures,
            regions: crate::design::Regions::default(),
            paper,
            white,
            smoke,
            hidden: false,
            minimized: false,
            scroll: 0,
            scroll_dragging: false,
            scroll_drag_offset: 0,
            profile_wheel: 0,
            scrollbar_drag: None,
            exit_pending: false,
            layout_key: Cell::new(None),
            control_bounds: RefCell::new(HashMap::new()),
            layout_passes: Cell::new(0),
            geometry_moves: Cell::new(0),
            parent_paints: Cell::new(0),
            buffer_dc: null_mut(),
            buffer_bitmap: null_mut(),
            buffer_old: null_mut(),
            buffer_size: (0, 0),
            hero_buffer: PaintBuffer::default(),
            speed_buffer: PaintBuffer::default(),
            control_buffers: HashMap::new(),
            hero_presentations: Cell::new(0),
            edit_ink_cache: RefCell::new(HashMap::new()),
        });
        let pointer = &mut *s as *mut State;
        let hwnd = CreateWindowExW(
            WS_EX_COMPOSITED
                | if smoke {
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                } else {
                    0
                },
            class.as_ptr(),
            wide(crate::APP_NAME).as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            if smoke { -5000 } else { CW_USEDEFAULT },
            if smoke { 0 } else { CW_USEDEFAULT },
            HOME_WIDTH * dpi / 96,
            if smoke {
                HOME_HEIGHT * dpi / 96
            } else {
                (HOME_HEIGHT * dpi / 96).min(work.bottom - work.top - 16 * dpi / 96)
            },
            null_mut(),
            null_mut(),
            instance,
            pointer.cast(),
        );
        if hwnd.is_null() {
            return Err("Не удалось создать окно приложения".into());
        }
        s.hwnd = hwnd;
        s.product_icons.apply(hwnd);
        configure_custom_chrome(hwnd);
        // Activation restores a window only; it cannot invoke privileged operations.
        ChangeWindowMessageFilterEx(hwnd, ACTIVATE, MSGFLT_ALLOW, null_mut());
        ChangeWindowMessageFilterEx(hwnd, RETIRE, MSGFLT_ALLOW, null_mut());
        for (n, label) in PAGES.iter().enumerate() {
            s.ctrl(
                200 + n as i32,
                "BUTTON",
                label,
                BS_OWNERDRAW as u32 | WS_TABSTOP,
            );
            SetWindowSubclass(
                s.controls[&(200 + n as i32)],
                Some(navigation_procedure),
                3,
                pointer as usize,
            );
        }
        s.ctrl(
            204,
            "BUTTON",
            "Свернуть окно",
            BS_OWNERDRAW as u32 | WS_TABSTOP,
        );
        s.ctrl(
            205,
            "BUTTON",
            "Закрыть окно",
            BS_OWNERDRAW as u32 | WS_TABSTOP,
        );
        s.ctrl(
            1001,
            "LISTBOX",
            "",
            LBS_OWNERDRAWFIXED as u32
                | LBS_HASSTRINGS as u32
                | LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32
                | WS_TABSTOP,
        );
        SendMessageW(s.controls[&1001], LB_SETITEMHEIGHT, 0, s.d(44) as isize);
        SetWindowSubclass(s.controls[&1001], Some(profile_procedure), 1, 0);
        s.ctrl(
            3301,
            "LISTBOX",
            "",
            LBS_OWNERDRAWFIXED as u32
                | LBS_HASSTRINGS as u32
                | LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32
                | WS_TABSTOP,
        );
        SendMessageW(s.controls[&3301], LB_SETITEMHEIGHT, 0, s.d(44) as isize);
        SetWindowSubclass(s.controls[&3301], Some(preset_list_procedure), 6, 0);
        for id in [2105, 3310] {
            s.ctrl(id, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP);
            SendMessageW(
                s.controls[&id],
                EM_SETLIMITTEXT,
                if id == 3310 { 120 } else { 1024 },
                0,
            );
        }
        for id in [1010, 1024, 2110, 3010, 3307] {
            s.ctrl(id, "STATIC", "Прокрутка", PIXEL_SCROLLBAR_STYLE);
            SetWindowSubclass(s.controls[&id], Some(scrollbar_procedure), 2, 0);
        }
        for (id, label) in [
            (1002, "+ Добавить конфигурацию"),
            (1003, "Выбрать"),
            (1004, "···"),
            (1009, "ОК"),
            (1021, "Импортировать"),
            (1022, "Отмена"),
            (1005, "Удалить"),
            (1007, "Подключить VPN"),
            (1011, "Подробности ошибки"),
            (2103, "+ .exe"),
            (2106, "Добавить"),
            (2107, "Убрать"),
            (3312, "Создать"),
            (3313, "Импорт"),
            (3314, "Экспорт"),
            (3315, "Правила"),
            (4015, "Настройки DNS"),
            (4016, "К настройкам"),
            (2104, "Сохранить"),
            (3011, "Обновить списки и IP"),
            (3012, "Сохранить настройки DNS"),
            (4003, "Диагностика"),
            (4004, "Открыть журналы"),
            (4005, "Свернуть в трей"),
            (4008, "Сохранить маршрутизацию"),
            (4009, "Сбросить правила и пресеты"),
            (2109, "Проверить"),
            (2311, "Параметры"),
            (3100, "К спискам"),
            (3102, "+ Готовый"),
            (3106, "Удалить"),
            (3107, "Восстановить правила"),
        ] {
            s.ctrl(id, "BUTTON", label, BS_OWNERDRAW as u32 | WS_TABSTOP);
        }
        SetWindowSubclass(s.controls[&1007], Some(hero_procedure), 4, 0);
        s.ctrl(1006, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP);
        for (n, category) in CATEGORIES.iter().enumerate() {
            s.ctrl(
                2200 + n as i32,
                "BUTTON",
                category,
                BS_OWNERDRAW as u32 | WS_TABSTOP,
            );
        }
        s.ctrl(
            3101,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32
                | CBS_OWNERDRAWFIXED as u32
                | CBS_HASSTRINGS as u32
                | WS_VSCROLL
                | WS_TABSTOP,
        );
        for id in [3103, 3104] {
            s.ctrl(id, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP);
            SendMessageW(
                s.controls[&id],
                EM_SETLIMITTEXT,
                if id == 3103 { 120 } else { 1000 },
                0,
            );
        }
        for id in [1020, 2102, 3006] {
            s.ctrl(
                id,
                "EDIT",
                "",
                ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32 | WS_TABSTOP,
            );
            SendMessageW(
                s.controls[&id],
                EM_SETLIMITTEXT,
                if id == 1020 { IMPORT_LIMIT + 1 } else { 500000 },
                0,
            );
            SetWindowSubclass(s.controls[&id], Some(profile_procedure), 1, 0);
        }
        for (id, label) in [
            (3004, "Обновлять IP доменов"),
            (3005, "Учитывать замеченные поддомены"),
            (4001, "Автоподключение VPN"),
            (4002, "Запуск с Windows"),
            (4006, "Весь трафик через VPN"),
            (4007, "Разрешить локальную сеть"),
            (4020, "Визуальные улучшения"),
            (4021, "Анимация фона"),
            (4023, "Пиксельные скругления"),
            (3105, "Набор включён"),
            (3108, "Россия"),
            (3109, "Steam"),
            (3110, "Ozon"),
            (3112, "Игры"),
            (2302, "Список включён"),
        ] {
            s.ctrl(id, "BUTTON", label, BS_OWNERDRAW as u32 | WS_TABSTOP);
            s.checks.insert(id, false);
        }
        for id in [3007, 3008, 3009, 3013, 3014] {
            s.ctrl(id, "EDIT", "", ES_NUMBER as u32 | WS_TABSTOP);
        }
        for (id, label) in [(4010, "Авто"), (4011, "Русский"), (4012, "English")] {
            s.ctrl(id, "BUTTON", label, BS_OWNERDRAW as u32 | WS_TABSTOP);
        }
        s.ctrl(4025, "STATIC", "Скорость фона", 0x0000_000d);
        s.ctrl(
            4024,
            "msctls_trackbar32",
            "Скорость фона",
            TBS_NOTICKS | TBS_FIXEDLENGTH | WS_TABSTOP,
        );
        let slider = s.controls[&4024];
        SetWindowTheme(slider, wide("").as_ptr(), wide("").as_ptr());
        SetWindowSubclass(slider, Some(speed_procedure), 7, 0);
        SendMessageW(slider, TBM_SETRANGEMIN, 0, 25);
        SendMessageW(slider, TBM_SETRANGEMAX, 0, 200);
        SendMessageW(slider, TBM_SETLINESIZE, 0, 5);
        SendMessageW(slider, TBM_SETPAGESIZE, 0, 25);
        SendMessageW(slider, TBM_SETTHUMBLENGTH, s.d(24) as usize, 0);
        s.populate();
        s.create_appearance_controls();
        s.refresh_design();
        if startup {
            s.hidden = true;
            ShowWindow(hwnd, SW_HIDE);
        } else {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        UpdateWindow(hwnd);
        s.app_active = smoke || GetForegroundWindow() == hwnd;
        if let Some(probe) = &mut s.startup_probe {
            probe.window_ready_ms = probe.started.elapsed().as_secs_f64() * 1000.0;
        }
        s.sync_background();
        if smoke {
            let original_language = s.locale.language();
            s.change_language(Language::Russian);
            s.model = json!({"profiles":[{"id":"demo-fr","name":"France · Paris"},{"id":"demo-nl","name":"Netherlands · Amsterdam"},{"id":"demo-de","name":"Germany · Frankfurt"},{"id":"demo-fi","name":"Finland · Helsinki"},{"id":"demo-se","name":"Sweden · Stockholm"}],"selected":"demo-fr","running":true,"phase":"connected","service_ready":true,"autoconnect":true,"settings":s.draft});
            for n in 5..12 {
                s.model["profiles"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":format!("demo-{n}"),"name":format!("Server {:02}",n+1)}));
            }
            let library = s.model["profiles"].clone();
            s.populate();
            if std::env::args().any(|arg| arg == "--smoke-animation-only") {
                s.background_enabled = true;
                s.app_active = true;
                s.refresh_design();
                let passed = smoke_animation_isolation(&mut s)?;
                DestroyWindow(hwnd);
                return if passed {
                    Ok(())
                } else {
                    Err("Animation isolation checks failed".into())
                };
            }
            if std::env::args().any(|arg| arg == "--smoke-hero-only") {
                let checks = smoke_hero_feedback(&mut s)?;
                std::fs::write(
                    s.root.join("native-hero-checks.json"),
                    serde_json::to_vec_pretty(&checks).unwrap(),
                )
                .map_err(|e| e.to_string())?;
                DestroyWindow(hwnd);
                return if checks.into_iter().all(|ok| ok) {
                    Ok(())
                } else {
                    Err("Hero animation checks failed".into())
                };
            }
            if std::env::args().any(|arg| arg == "--smoke-appearance-only") {
                let checks = smoke_appearance(&mut s)?;
                DestroyWindow(hwnd);
                return if checks.into_iter().all(|ok| ok) {
                    Ok(())
                } else {
                    Err("Appearance checks failed".into())
                };
            }
            if std::env::args().any(|arg| arg == "--smoke-render-only") {
                let lists = smoke_list_surfaces(&mut s)?;
                let appearance = smoke_appearance(&mut s)?;
                s.apply_appearance(crate::design::Design::default())?;
                let stability = smoke_render_stability(&mut s)?;
                let design = smoke_design_preferences(&mut s)?;
                let chrome = smoke_window_chrome(&mut s)?;
                let home = smoke_home_design(&mut s)?;
                let edits = smoke_hero_and_edit_padding(&mut s)?;
                let feedback = smoke_hero_feedback(&mut s)?;
                for page in 0..4 {
                    s.page = page;
                    s.scroll = 0;
                    s.layout();
                    UpdateWindow(hwnd);
                    screenshot(hwnd, &s.root.join(format!("native-page-{page}.bmp")))?;
                }
                std::fs::write(s.root.join("native-render-checks.json"), serde_json::to_vec_pretty(&json!({
                    "hidden_lists_stay_hidden":stability[0],
                    "inactive_and_resizing_pause_animation":stability[1],
                    "partial_rows_clip_at_all_dpi":stability[2],
                    "full_window_animation_keeps_opaque_controls_clean":stability[3],
                    "scroll_preserves_editor_height_text_and_caret":stability[4],
                    "scrollbar_grab_does_not_jump_and_drag_reaches_bottom":stability[5],
                    "editing_and_scrolling_pause_without_losing_phase":stability[6],
                    "background_speed_slider_previews_scales_and_persists":stability[7],
                    "animated_field_covers_header_and_footer":stability[8],
                    "native_inputs_have_stepped_frames_at_all_dpi":stability[9],
                    "five_pixel_main_scrollbar_stays_in_content":stability[10],
                    "appearance":appearance,"design":design,"chrome":[chrome.0,chrome.1],"home":home,"edit_padding":edits,
                    "hero_feedback":feedback,"list_surfaces":lists
                })).unwrap()).map_err(|e|e.to_string())?;
                DestroyWindow(hwnd);
                return if stability
                    .into_iter()
                    .chain(design)
                    .chain(appearance)
                    .chain([chrome.0, chrome.1])
                    .chain(home)
                    .chain(edits)
                    .chain(feedback)
                    .chain(lists)
                    .all(|ok| ok)
                {
                    Ok(())
                } else {
                    Err("Native render stability checks failed".into())
                };
            }
            if std::env::args().any(|arg| arg == "--smoke-lists-only") {
                let checks = smoke_list_surfaces(&mut s)?;
                DestroyWindow(hwnd);
                return if checks.into_iter().all(|ok| ok) {
                    Ok(())
                } else {
                    Err("Native list surface checks failed".into())
                };
            }
            if std::env::args().any(|arg| arg == "--smoke-presets-only") {
                let checks = smoke_preset_workflow(&mut s)?;
                std::fs::write(
                    s.root.join("preset-checks.json"),
                    serde_json::to_vec_pretty(&checks).unwrap(),
                )
                .map_err(|e| e.to_string())?;
                DestroyWindow(hwnd);
                return if checks.into_iter().all(|ok| ok) {
                    Ok(())
                } else {
                    Err("Preset UI checks failed".into())
                };
            }
            if !smoke_list_surfaces(&mut s)?.into_iter().all(|ok| ok) {
                return Err("Native list surface checks failed".into());
            }
            let render_stability = smoke_render_stability(&mut s)?;
            let window_chrome = smoke_window_chrome(&mut s)?;
            let home_design = smoke_home_design(&mut s)?;
            let hero_edits = smoke_hero_and_edit_padding(&mut s)?;
            let imports = smoke_import_flow(&mut s)?;
            let preset_checks = smoke_preset_workflow(&mut s)?;
            for page in 0..4 {
                s.page = page;
                s.layout();
                InvalidateRect(hwnd, null(), 1);
                UpdateWindow(hwnd);
                screenshot(hwnd, &s.root.join(format!("native-page-{page}.bmp")))?;
            }
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            let original_notice = s.notice.clone();
            let original_model = s.model.clone();
            let unknown_error =
                "Синтетическая неизвестная ошибка Windows · fixture.example · 0x80070057";
            let mut data = s.model.clone();
            data["last_error"] = json!(unknown_error);
            data["phase"] = json!("idle");
            data["running"] = json!(false);
            let response = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":false,"error":unknown_error,"data":data})),
                action: "Connect".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, response as isize);
            let unknown_error_retained = s.locale.error(&s.notice) == unknown_error
                && s.connection_error() == Some(unknown_error)
                && s.notice == unknown_error;
            s.page = 0;
            s.scroll = 0;
            s.layout();
            let unknown_error_retained =
                unknown_error_retained && IsWindowVisible(s.controls[&1011]) != 0;
            InvalidateRect(hwnd, null(), 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-error.bmp"))?;
            let actual_windows_error =
                "Приложение содержит недопустимые знаки или имеет недопустимую длину.";
            s.notice = actual_windows_error.into();
            s.model["last_error"] = json!(actual_windows_error);
            s.layout();
            InvalidateRect(hwnd, null(), 0);
            UpdateWindow(hwnd);
            let translated_windows_error = s.locale.error(actual_windows_error);
            let known_error_translated = translated_windows_error
                == "The application path contains invalid characters or exceeds the Windows length limit."
                && s.connection_error() == Some(actual_windows_error)
                && s.model["last_error"] == actual_windows_error;
            let (width, _, _) = s.dimensions();
            let left = ((width - 520) / 2).max(24);
            let area = width - left * 2 - 36;
            let dc = GetDC(hwnd);
            let old_font = SelectObject(dc, s.fonts[3]);
            let lines = wrap_lines(&translated_windows_error, s.d(area), 2, |value| {
                let value = wide(value);
                let mut size = std::mem::zeroed();
                GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
                size.cx
            });
            SelectObject(dc, old_font);
            ReleaseDC(hwnd, dc);
            let second_line_painted =
                (s.d(s.home_layout().detail_y + 20)..s.d(s.home_layout().detail_y + 36)).any(|y| {
                    (s.d(left)..s.d(left + area)).any(|x| GetPixel(s.buffer_dc, x, y) == DANGER)
                });
            let wrapped_error_readable = lines.len() == 2
                && lines.join(" ") == translated_windows_error
                && second_line_painted
                && IsWindowVisible(s.controls[&1011]) != 0;
            screenshot(hwnd, &s.root.join("native-en-firewall-error.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            let unknown_error_retained =
                unknown_error_retained && s.locale.error(unknown_error) == unknown_error;
            InvalidateRect(hwnd, null(), 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-ru-firewall-error.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            s.model = original_model;
            s.notice = original_notice;
            s.update_buttons();
            for page in 0..4 {
                s.page = page;
                s.scroll = 0;
                s.layout();
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
                screenshot(hwnd, &s.root.join(format!("native-en-page-{page}.bmp")))?;
            }
            s.scroll = 120;
            s.layout();
            InvalidateRect(hwnd, null(), 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-settings-scrolled.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            s.page = 0;
            s.scroll = 0;
            s.layout();
            UpdateWindow(hwnd);
            smoke_idle_hold(&mut s)?;
            SendMessageW(s.controls[&1001], LB_SETTOPINDEX, 0, 0);
            let wheel_param = ((-120i16) as u16 as usize) << 16;
            SendMessageW(s.controls[&1001], WM_MOUSEWHEEL, wheel_param, 0);
            let child_wheel = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
            let mut list_screen = std::mem::zeroed();
            GetWindowRect(s.controls[&1001], &mut list_screen);
            let wheel_point = (((list_screen.top + 20) as u16 as usize) << 16)
                | ((list_screen.left + 20) as u16 as usize);
            let page_scroll = s.scroll;
            SendMessageW(hwnd, WM_MOUSEWHEEL, wheel_param, wheel_point as isize);
            let parent_wheel = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
            let mouse_wheel =
                child_wheel > 0 && parent_wheel > child_wheel && s.scroll == page_scroll;
            // Drag the visible pixel thumb, not just an internal list API.
            s.scroll_to(1001, 0);
            let bar_start = (8usize << 16) | 4;
            let mut bar_rect = std::mem::zeroed();
            GetClientRect(s.controls[&1010], &mut bar_rect);
            let bar_end = (((bar_rect.bottom - 1) as usize) << 16) | 4;
            SendMessageW(s.controls[&1010], WM_LBUTTONDOWN, 1, bar_start as isize);
            SendMessageW(s.controls[&1010], WM_MOUSEMOVE, 1, bar_end as isize);
            SendMessageW(s.controls[&1010], WM_LBUTTONUP, 0, bar_end as isize);
            let (_, profile_count, visible_profiles, _) = s.scroll_info(1001);
            let scrollbar_drag = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0)
                == (profile_count - visible_profiles) as isize
                && s.scrollbar_drag.is_none();
            for _ in 0..4 {
                SendMessageW(s.controls[&1001], WM_MOUSEWHEEL, wheel_param, 0);
            }
            let row_height = SendMessageW(s.controls[&1001], LB_GETITEMHEIGHT, 0, 0) as i32;
            let row_point = (((row_height * (visible_profiles - 1) + row_height / 2) as usize)
                << 16)
                | s.d(20) as usize;
            SendMessageW(
                s.controls[&1001],
                WM_LBUTTONDOWN,
                1, // MK_LBUTTON
                row_point as isize,
            );
            SendMessageW(s.controls[&1001], WM_LBUTTONUP, 0, row_point as isize);
            let last_by_mouse = s.model["selected"] == "demo-11";
            SetFocus(s.controls[&1001]);
            SendMessageW(s.controls[&1001], WM_KEYDOWN, VK_END as usize, 0);
            let keyboard_scroll = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) > 0;
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-list-scrolled.bmp"))?;
            s.page = 2;
            SetWindowPos(
                hwnd,
                null_mut(),
                -5000,
                0,
                s.d(500),
                s.d(720),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            s.page = 0;
            s.scroll = 0;
            s.layout();
            s.scroll_to(1001, 0);
            SendMessageW(s.controls[&1001], WM_MOUSEWHEEL, wheel_param, 0);
            let mut small_list = std::mem::zeroed();
            GetClientRect(s.controls[&1001], &mut small_list);
            let small_wheel = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) > 0
                && small_list.bottom
                    / SendMessageW(s.controls[&1001], LB_GETITEMHEIGHT, 0, 0) as i32
                    >= 4
                && IsWindowVisible(s.controls[&1010]) != 0;
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-small-list.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-small-list.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            s.page = 3;
            s.dns_settings_open = true;
            s.scroll = 520;
            s.layout();
            SetViewportOrgEx(s.buffer_dc, 0, 0, null_mut());
            SetPixel(s.buffer_dc, s.d(10), s.d(500), 0x00ff00ff);
            InvalidateRect(hwnd, null(), 1);
            let mut clear_damage = RECT::default();
            GetUpdateRect(hwnd, &mut clear_damage, 0);
            UpdateWindow(hwnd);
            let scrolled_buffer_clear = GetPixel(s.buffer_dc, s.d(10), s.d(500))
                == crate::texture::sample(BG, s.dpi, s.d(10), s.d(500));
            std::fs::write(s.root.join("native-scroll-paint.json"), serde_json::to_vec_pretty(&json!({
                "actual":GetPixel(s.buffer_dc,s.d(10),s.d(500)),
                "expected":crate::texture::sample(BG,s.dpi,s.d(10),s.d(500)),
                "dpi":s.dpi,"phase":s.background_millis,
                "damage":[clear_damage.left,clear_damage.top,clear_damage.right,clear_damage.bottom]
            })).unwrap()).map_err(|e|e.to_string())?;
            screenshot(hwnd, &s.root.join("native-small-page.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-small-dns.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            s.page = 0;
            s.dns_settings_open = false;
            s.scroll = 0;
            s.model["profiles"] = json!([]);
            s.model["running"] = json!(false);
            s.model["phase"] = json!("idle");
            s.populate();
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-empty.bmp"))?;
            s.model["profiles"] = json!([{"id":"demo-fr","name":"France · Paris"}]);
            s.model["phase"] = json!("connecting");
            s.model["can_disconnect"] = json!(true);
            s.busy = true;
            s.populate();
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-connecting.bmp"))?;
            let cancel_enabled = IsWindowEnabled(s.controls[&1007]) != 0;
            SendMessageW(hwnd, WM_COMMAND, 1007, 0);
            let cancel_click = s.phase() == "idle" && !s.busy;
            s.busy = false;
            s.model["phase"] = json!("recovery");
            s.populate();
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-recovery.bmp"))?;
            s.model["phase"] = json!("idle");
            s.model["can_disconnect"] = json!(false);
            s.populate();
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-idle.bmp"))?;
            s.set_dpi_fonts(192);
            SetWindowPos(
                hwnd,
                null_mut(),
                -5000,
                0,
                s.d(560),
                s.d(820),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            s.populate();
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-dpi-200.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-dpi-200.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            s.model["profiles"] = library.clone();
            s.populate_profiles();
            SendMessageW(s.controls[&1001], LB_SETTOPINDEX, 0, 0);
            SendMessageW(s.controls[&1001], WM_MOUSEWHEEL, wheel_param, 0);
            let dpi_wheel = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) > 0;
            let mut fractional_dpi = true;
            for (dpi, label) in [(120, "125"), (144, "150")] {
                s.set_dpi_fonts(dpi);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    -5000,
                    0,
                    s.d(560),
                    s.d(820),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
                s.layout();
                s.scroll_to(1001, 0);
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
                let mut list = std::mem::zeroed();
                GetClientRect(s.controls[&1001], &mut list);
                let mut add = std::mem::zeroed();
                GetWindowRect(s.controls[&1002], &mut add);
                let mut window = std::mem::zeroed();
                GetWindowRect(hwnd, &mut window);
                fractional_dpi &= list.bottom
                    / SendMessageW(s.controls[&1001], LB_GETITEMHEIGHT, 0, 0) as i32
                    >= 4
                    && IsWindowVisible(s.controls[&1010]) != 0
                    && add.bottom - window.top < s.d(740);
                SendMessageW(s.controls[&1001], WM_MOUSEWHEEL, wheel_param, 0);
                fractional_dpi &= SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) > 0;
                screenshot(hwnd, &s.root.join(format!("native-dpi-{label}.bmp")))?;
            }
            s.set_dpi_fonts(192);
            SetWindowPos(
                hwnd,
                null_mut(),
                -5000,
                0,
                s.d(560),
                s.d(820),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            s.layout();
            // Real REPLY dispatch with synthetic snapshots: a timer or unrelated
            // action must preserve raw edits, including temporarily empty numbers.
            s.loaded = true;
            s.model["selected"] = json!("demo-fr");
            s.rename_open = true;
            let edits = [
                (2102, "unsaved.example\r\nsecond.example"),
                (3006, "seed.example\r\nsub.seed.example"),
                (3007, ""),
                (3008, "001234"),
                (3013, "0512"),
                (1006, "Моё несохранённое имя"),
            ];
            for (id, value) in edits {
                s.set(id, value);
            }
            s.check(3004, true);
            s.check(3005, false);
            let mut data = s.model.clone();
            data["phase"] = json!("connecting");
            data["dns_count"] = json!(3);
            data["profiles"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"demo-next","name":"Another profile"}));
            let response = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":data})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, response as isize);
            let status_preserves_edits = edits.iter().all(|(id, value)| s.text(*id) == *value)
                && s.checked(3004)
                && !s.checked(3005);
            s.status_revision = Some("qa-current".into());
            let unchanged_model = s.model.clone();
            let unchanged_layout = s.layout_passes.get();
            let response = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"not_modified":true,"revision":"qa-current"})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, response as isize);
            let compact_status_preserves_edits = s.model == unchanged_model
                && s.layout_passes.get() == unchanged_layout
                && edits.iter().all(|(id, value)| s.text(*id) == *value);
            let attempts = s.status_attempts.get();
            let response = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"not_modified":true,"revision":"qa-obsolete"})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, response as isize);
            let stale_compact_status_refetches = s.status_revision.is_none()
                && s.status_attempts.get() == attempts + 1
                && s.model == unchanged_model;
            let response = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":s.model})),
                action: "Update".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, response as isize);
            let action_preserves_edits = edits.iter().all(|(id, value)| s.text(*id) == *value)
                && s.checked(3004)
                && !s.checked(3005);
            let locale_model = s.model.clone();
            let locale_draft = s.draft.clone();
            let locale_selected = s.profile_id();
            let locale_top = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
            let locale_scroll = s.scroll;
            let locale_generation = s.generation;
            SendMessageW(s.controls[&2102], EM_SETSEL, 3, 13);
            let edit_selection = SendMessageW(s.controls[&2102], EM_GETSEL, 0, 0);
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            let locale_preserves_edits = edits.iter().all(|(id, value)| s.text(*id) == *value)
                && s.model == locale_model
                && s.draft == locale_draft
                && s.profile_id() == locale_selected
                && SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) == locale_top
                && SendMessageW(s.controls[&2102], EM_GETSEL, 0, 0) == edit_selection
                && s.scroll == locale_scroll
                && s.generation == locale_generation
                && s.checked(3004)
                && !s.checked(3005);
            let english_captions = s.locale.language() == Language::English
                && s.captions.keys().all(|id| {
                    *id == 4011
                        || !s
                            .text(*id)
                            .chars()
                            .any(|c| ('\u{0400}'..='\u{052f}').contains(&c))
                });
            let locale_persistence = Locale::new(&s.root, true)?.language() == Language::English;
            SendMessageW(hwnd, WM_COMMAND, 4010, 0);
            let auto_restores_system = s.locale.language() == Language::Auto
                && Locale::new(&s.root, true)?.language() == Language::Auto
                && s.locale.text("Подключение") == Locale::new(&s.root, true)?.text("Подключение");
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            let russian_roundtrip = s.text(200) == PAGES[0]
                && s.text(2104) == "Сохранить"
                && edits.iter().all(|(id, value)| s.text(*id) == *value);
            let profile_name = s.model["profiles"][0]["name"].clone();
            s.model["profiles"][0]["name"] = json!("Настройки");
            s.populate_profiles();
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            let mut raw_profile_name = vec![0u16; 256];
            let profile_length = SendMessageW(
                s.controls[&1001],
                LB_GETTEXT,
                0,
                raw_profile_name.as_mut_ptr() as isize,
            )
            .max(0) as usize;
            let profile_names_untranslated =
                String::from_utf16_lossy(&raw_profile_name[..profile_length]) == "Настройки"
                    && s.model["profiles"][0]["name"] == "Настройки";
            s.model["profiles"][0]["name"] = profile_name;
            s.populate_profiles();
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            UpdateWindow(hwnd);
            let before_layout = s.layout_passes.get();
            let before_moves = s.geometry_moves.get();
            let before_paints = s.parent_paints.get();
            let before_top = SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0);
            for n in 0..100 {
                let mut data = s.model.clone();
                data["uptime_seconds"] = json!(n);
                data["dns_count"] = json!(n);
                let reply = Box::into_raw(Box::new(Answer {
                    response: Ok(json!({"ok":true,"data":data})),
                    action: "Status".into(),
                    generation: s.generation,
                    request_id: None,
                }));
                SendMessageW(hwnd, REPLY, 0, reply as isize);
                UpdateWindow(hwnd);
            }
            let quiet_status = s.layout_passes.get() == before_layout
                && s.geometry_moves.get() == before_moves
                && s.parent_paints.get() == before_paints
                && SendMessageW(s.controls[&1001], LB_GETTOPINDEX, 0, 0) == before_top;
            // Let lazy Windows font/control caches settle before measuring
            // resource growth from repeated rendering of an unchanged frame.
            for _ in 0..16 {
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
            }
            GdiFlush();
            let gdi_objects = GetGuiResources(GetCurrentProcess(), 0);
            let buffer_dc = s.buffer_dc;
            let buffer_bitmap = s.buffer_bitmap;
            let mut resource_samples = vec![gdi_objects];
            for _ in 0..200 {
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
            }
            GdiFlush();
            let warmed_gdi_objects = GetGuiResources(GetCurrentProcess(), 0);
            resource_samples.push(warmed_gdi_objects);
            for n in 0..200 {
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
                if n % 25 == 24 {
                    GdiFlush();
                    resource_samples.push(GetGuiResources(GetCurrentProcess(), 0));
                }
            }
            GdiFlush();
            let after_gdi_objects = GetGuiResources(GetCurrentProcess(), 0);
            let stable_render_resources = after_gdi_objects <= warmed_gdi_objects
                && s.buffer_dc == buffer_dc
                && s.buffer_bitmap == buffer_bitmap;
            std::fs::write(s.root.join("native-render-resources.json"),serde_json::to_vec_pretty(&json!({
                "gdi_objects_before":gdi_objects,"gdi_objects_after":after_gdi_objects,
                "gdi_objects_warmed":warmed_gdi_objects,"samples":resource_samples,
                "buffer_dc_reused":s.buffer_dc==buffer_dc,"buffer_bitmap_reused":s.buffer_bitmap==buffer_bitmap
            })).unwrap()).map_err(|e|e.to_string())?;
            s.busy = true;
            s.pending_action = "Connect".into();
            s.pending_id = Some("failed-connect".into());
            s.pending_since = Some(Instant::now());
            let mut data = s.model.clone();
            data["phase"] = json!("idle");
            data["busy"] = json!(false);
            data["last_operation"] =
                json!({"request_id":"failed-connect","ok":false,"error":"Connection failed"});
            let reply = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":data})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let terminal_failure = !s.busy && s.pending_id.is_none() && s.phase() == "idle";
            s.busy = true;
            s.pending_action = "Disconnect".into();
            s.pending_id = Some("new-disconnect".into());
            let mut data = s.model.clone();
            data["phase"] = json!("connected");
            let reply = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":data})),
                action: "Connect".into(),
                generation: s.generation,
                request_id: Some("old-connect".into()),
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let stale_connect = s.busy
                && s.pending_id.as_deref() == Some("new-disconnect")
                && s.phase() == "disconnecting";
            // Cleanup may validate and restart an installed controller before
            // its RPC. Its 180-second watchdog must retain this same request
            // beyond the normal 115-second deadline and through 180 seconds,
            // then expose recovery rather
            // than infer that persistent protection has been removed.
            let mut cleanup_wait_retained = true;
            for age in [116, 180] {
                s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(age));
                SendMessageW(hwnd, WM_TIMER, 1, 0);
                cleanup_wait_retained &= s.busy
                    && s.pending_id.as_deref() == Some("new-disconnect")
                    && s.phase() == "disconnecting";
            }
            s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(181));
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            let bounded_wait = cleanup_wait_retained
                && !s.busy
                && s.pending_id.is_none()
                && s.phase() == "recovery";
            let (caption_blocks, texture_cache) = smoke_pixel_surfaces(&mut s);
            let design_checks = smoke_design_preferences(&mut s)?;
            SendMessageW(hwnd, WM_COMMAND, 204, 0);
            let caption_minimizes = IsIconic(hwnd) != 0 && s.minimized;
            let before_poll = s.status_attempts.get();
            // Exercise the production gate with no pending operation. The
            // blocked timer cannot reach RPC even though smoke=false here.
            s.smoke = false;
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            s.smoke = true;
            let minimized_idle_quiet = s.status_attempts.get() == before_poll;
            s.pending_id = Some("smoke-minimized-pending".into());
            s.pending_since = Some(Instant::now());
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            let minimized_pending_checked = s.status_attempts.get() == before_poll + 1;
            s.pending_id = None;
            s.pending_since = None;
            SendMessageW(hwnd, ACTIVATE, 0, 0);
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            let minimized_restore_resumes =
                IsIconic(hwnd) == 0 && !s.minimized && s.status_attempts.get() == before_poll + 2;
            SendMessageW(hwnd, WM_COMMAND, 204, 0);
            SendMessageW(hwnd, WM_COMMAND, 205, 0);
            let hidden_iconic = s.hidden && IsIconic(hwnd) != 0;
            SendMessageW(hwnd, TRAY, 0, WM_LBUTTONUP as isize);
            let tray_restores_minimized = hidden_iconic
                && !s.hidden
                && !s.minimized
                && IsIconic(hwnd) == 0
                && IsWindowVisible(hwnd) != 0;
            let mut watchdog_stops_timer = [false; 2];
            for (case, hidden) in [true, false].into_iter().enumerate() {
                s.busy = true;
                s.pending_action = "Connect".into();
                s.pending_id = Some("smoke-unacknowledged-connect".into());
                s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(116));
                if hidden {
                    SendMessageW(hwnd, WM_COMMAND, 205, 0);
                } else {
                    SendMessageW(hwnd, WM_COMMAND, 204, 0);
                }
                // A real short native timer would produce WM_TIMER after the
                // wait if the watchdog failed to remove it. No service exists
                // in this fixture and the operation is entirely synthetic.
                SetTimer(hwnd, 1, 10, None);
                let before = s.status_attempts.get();
                SendMessageW(hwnd, WM_TIMER, 1, 0);
                thread::sleep(std::time::Duration::from_millis(40));
                let mut timer_message = std::mem::zeroed();
                let timer_remaining =
                    PeekMessageW(&mut timer_message, hwnd, WM_TIMER, WM_TIMER, PM_NOREMOVE) != 0;
                watchdog_stops_timer[case] = !timer_remaining
                    && s.status_attempts.get() == before
                    && !s.busy
                    && s.pending_id.is_none()
                    && s.phase() == "recovery";
                KillTimer(hwnd, 1);
                SendMessageW(hwnd, ACTIVATE, 0, 0);
                watchdog_stops_timer[case] &= s.phase() == "recovery"
                    && !s.hidden
                    && IsIconic(hwnd) == 0
                    && IsWindowVisible(hwnd) != 0;
            }
            let _qa_instance =
                single_instance(&identity, true)?.ok_or("Smoke instance unexpectedly existed")?;
            let peer_identity = identity.clone();
            let duplicate_reused = thread::spawn(move || {
                single_instance(&peer_identity, true).map(|guard| guard.is_none())
            })
            .join()
            .map_err(|_| "Smoke activation thread failed")??;
            let mut normal_activation: MSG = std::mem::zeroed();
            while PeekMessageW(&mut normal_activation, hwnd, ACTIVATE, ACTIVATE, PM_REMOVE) != 0 {
                DispatchMessageW(&normal_activation);
            }
            s.hidden = true;
            ShowWindow(hwnd, SW_HIDE);
            let peer_identity = identity.clone();
            let startup_duplicate_quiet = thread::spawn(move || {
                single_instance(&peer_identity, false).map(|guard| guard.is_none())
            })
            .join()
            .map_err(|_| "Smoke startup thread failed")??;
            let mut activation_message: MSG = std::mem::zeroed();
            let startup_duplicate_quiet = startup_duplicate_quiet
                && PeekMessageW(
                    &mut activation_message,
                    hwnd,
                    ACTIVATE,
                    ACTIVATE,
                    PM_NOREMOVE,
                ) == 0
                && s.hidden
                && IsWindowVisible(hwnd) == 0;
            let old_model = s.model.clone();
            let old_draft = s.draft.clone();
            s.model["autoconnect"] = json!(false);
            SendMessageW(hwnd, WM_COMMAND, 4002, 0);
            let startup_preference_independent = s.autostart_enabled
                && s.checked(4002)
                && s.model["autoconnect"] == false
                && s.draft == old_draft;
            SendMessageW(hwnd, WM_COMMAND, 4002, 0);
            s.startup_pending = true;
            s.startup_since = Instant::now();
            s.model["service_ready"] = json!(false);
            let status_before = s.status_attempts.get();
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            SendMessageW(hwnd, STARTUP_CONNECT, 0, 0);
            let startup_waits_hidden =
                s.startup_pending && s.status_attempts.get() > status_before && s.hidden;
            s.model["service_ready"] = json!(true);
            s.model["phase"] = json!("idle");
            SendMessageW(hwnd, STARTUP_CONNECT, 0, 0);
            let startup_ready_consumes_once = !s.startup_pending && s.model["autoconnect"] == false;
            s.startup_pending = true;
            s.startup_since = Instant::now() - std::time::Duration::from_secs(181);
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            let startup_wait_bounded = !s.startup_pending;
            s.model = old_model;
            s.populate();
            SendMessageW(hwnd, ACTIVATE, 0, 0);
            let tray_restored = !s.hidden && IsWindowVisible(hwnd) != 0;
            s.model["phase"] = json!("connected");
            s.model["running"] = json!(true);
            SendMessageW(hwnd, WM_COMMAND, 205, 0);
            let close_hides = s.hidden
                && IsWindow(hwnd) != 0
                && IsWindowVisible(hwnd) == 0
                && s.model["running"] == true;
            SendMessageW(hwnd, ACTIVATE, 0, 0);
            // Exit supersedes a connect request even when hidden. A failed
            // cleanup must bring the recovery interface back for a retry.
            s.busy = true;
            s.pending_action = "Connect".into();
            s.next = Some(json!({"action":"Connect"}));
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            s.request(json!({"action":"Exit"}));
            let exit_cancels_connect = s.busy
                && s.pending_action == "Exit"
                && s.exit_pending
                && s.pending_id.is_some()
                && s.next.is_none()
                && s.phase() == "disconnecting";
            let mut data = s.model.clone();
            data["phase"] = json!("recovery");
            data["busy"] = json!(false);
            data["dns_recovery_pending"] = json!(true);
            data["last_operation"] = json!({"request_id":s.pending_id,"action":"Exit","ok":false,"error":"Synthetic DNS cleanup failure"});
            let reply = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":data})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let exit_failure_reopens = !s.busy
                && !s.exit_pending
                && !s.hidden
                && s.phase() == "recovery"
                && IsWindowVisible(hwnd) != 0;
            // Losing a receipt alone is not permission to exit: without scoped
            // controller cleanup proof, continue awaiting its terminal state.
            s.request(json!({"action":"Exit"}));
            let reply = Box::into_raw(Box::new(Answer {
                response: Err("Synthetic lost Exit receipt".into()),
                action: "Exit".into(),
                generation: s.generation,
                request_id: s.pending_id.clone(),
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let lost_exit_without_proof_waits =
                IsWindow(hwnd) != 0 && s.busy && s.exit_pending && s.pending_id.is_some();
            // Missing controller/runtime metadata is not a terminal receipt.
            // Even an apparently idle model must remain open on RPC loss;
            // the watchdog exposes recovery and allows an explicit retry.
            let mut unconfirmed_exit_cases = [false; 2];
            for (case, watchdog) in [false, true].into_iter().enumerate() {
                s.model["running"] = json!(false);
                s.model["dns_recovery_pending"] = json!(false);
                s.model["traffic_guard"] = json!({"configured":false,"protected":false});
                s.model["last_operation"] = Value::Null;
                s.request(json!({"action":"Exit"}));
                if watchdog {
                    s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(181));
                    SendMessageW(hwnd, WM_TIMER, 1, 0);
                    unconfirmed_exit_cases[case] = IsWindow(hwnd) != 0
                        && !s.busy
                        && !s.exit_pending
                        && !s.hidden
                        && s.pending_id.is_none()
                        && s.phase() == "recovery"
                        && IsWindowVisible(hwnd) != 0;
                } else {
                    let reply = Box::into_raw(Box::new(Answer {
                        response: Err("Synthetic Status transport failure".into()),
                        action: "Status".into(),
                        generation: s.generation,
                        request_id: None,
                    }));
                    SendMessageW(hwnd, REPLY, 0, reply as isize);
                    unconfirmed_exit_cases[case] =
                        IsWindow(hwnd) != 0 && s.busy && s.exit_pending && s.pending_id.is_some();
                }
            }
            // The same window procedure used by upgrade retirement must really
            // destroy a window while preserving the VPN state; it has no RPC.
            let retire = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                wide("Smoke retirement").as_ptr(),
                WS_POPUP,
                -5000,
                0,
                10,
                10,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if retire.is_null() {
                return Err("Could not create retirement smoke window".into());
            }
            SetWindowLongPtrW(retire, GWLP_USERDATA, pointer as isize);
            let retired_model = s.model.clone();
            let retired_generation = s.generation;
            SendMessageW(retire, RETIRE, 0, 0);
            let retire_preserves_vpn = IsWindow(retire) == 0
                && s.model == retired_model
                && s.generation == retired_generation;
            s.hwnd = hwnd;
            // A cached terminal success must destroy the real smoke interface.
            s.request(json!({"action":"Exit"}));
            let mut data = s.model.clone();
            data["phase"] = json!("idle");
            data["running"] = json!(false);
            data["busy"] = json!(false);
            data["dns_recovery_pending"] = json!(false);
            data["last_operation"] = json!({"request_id":s.pending_id,"action":"Exit","ok":true});
            let reply = Box::into_raw(Box::new(Answer {
                response: Ok(json!({"ok":true,"data":data})),
                action: "Status".into(),
                generation: s.generation,
                request_id: None,
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let exit_stops_then_destroys =
                IsWindow(hwnd) == 0 && s.model["running"] == false && !s.busy && !s.exit_pending;
            s.locale.set_language(original_language)?;
            let distinct_roots = window_identity(&s.root.join("other-owner-scope"))? != identity;
            let checks = [
                ("hidden_lists_stay_hidden_on_refresh", render_stability[0]),
                ("inactive_and_resizing_pause_animation", render_stability[1]),
                ("partial_rows_clip_at_all_dpi", render_stability[2]),
                (
                    "editing_and_scrolling_pause_without_losing_phase",
                    render_stability[6],
                ),
                (
                    "background_speed_slider_previews_scales_and_persists",
                    render_stability[7],
                ),
                (
                    "full_window_animation_keeps_opaque_controls_clean",
                    render_stability[3],
                ),
                (
                    "animated_field_covers_header_and_footer",
                    render_stability[8],
                ),
                (
                    "native_inputs_have_stepped_frames_at_all_dpi",
                    render_stability[9],
                ),
                (
                    "five_pixel_main_scrollbar_stays_in_content",
                    render_stability[10],
                ),
                (
                    "scroll_preserves_editor_height_text_and_caret",
                    render_stability[4],
                ),
                (
                    "scrollbar_grab_does_not_jump_and_drag_reaches_bottom",
                    render_stability[5],
                ),
                (
                    "design_settings_are_visible_and_enabled_by_default",
                    design_checks[0],
                ),
                (
                    "design_master_switch_persists_disables_effects_and_stops_calculations",
                    design_checks[1],
                ),
                (
                    "design_individual_switches_persist_without_changing_vpn_draft_or_caret",
                    design_checks[2],
                ),
                (
                    "background_animation_stops_in_tray_and_when_minimized",
                    design_checks[3],
                ),
                (
                    "combo_row_corners_and_pixel_arrows_scale_at_every_dpi",
                    design_checks[4],
                ),
                (
                    "scrolled_backbuffer_uses_screen_coordinates_and_composited_presentation",
                    design_checks[5],
                ),
                (
                    "animation_publishes_frame_before_children_and_obeys_its_own_switch",
                    design_checks[6],
                ),
                (
                    "sixty_animation_frames_paint_once_without_layout_moves_or_gdi_growth",
                    design_checks[7],
                ),
                (
                    "startup_duplicate_does_not_activate_existing_window",
                    startup_duplicate_quiet,
                ),
                (
                    "startup_preference_is_independent_from_vpn_and_rule_draft",
                    startup_preference_independent,
                ),
                (
                    "startup_waits_for_controller_even_while_hidden",
                    startup_waits_hidden,
                ),
                (
                    "startup_ready_consumes_login_attempt_once",
                    startup_ready_consumes_once,
                ),
                ("startup_readiness_wait_is_bounded", startup_wait_bounded),
                (
                    "presets_create_edit_rename_sources_and_all_six_sections",
                    preset_checks[0],
                ),
                (
                    "preset_draft_survives_rules_category_switch_with_invalid_raw_text",
                    preset_checks[1],
                ),
                (
                    "presets_disable_delete_restore_preserve_personal_rules",
                    preset_checks[2],
                ),
                (
                    "separate_preset_library_and_rules_fit_all_dpi_and_dns_stays_in_settings",
                    preset_checks[3],
                ),
                (
                    "rule_diagnostics_scroll_full_large_text_and_close_without_mutating_draft",
                    preset_checks[4],
                ),
                (
                    "preset_export_import_roundtrip_is_separate_disabled_and_unicode_safe",
                    preset_checks[5],
                ),
                (
                    "import_empty_and_oversized_input_are_rejected_and_file_reads_are_bounded_utf8",
                    imports[0],
                ),
                (
                    "paste_import_keeps_raw_text_and_caret_on_status_error_and_language_switch",
                    imports[1],
                ),
                (
                    "import_cancel_success_and_lost_receipt_clear_secret_draft_and_show_count",
                    imports[2],
                ),
                (
                    "batch_import_keeps_connected_selection_scroll_and_factual_protocols",
                    imports[3],
                ),
                (
                    "paste_form_fits_short_workarea_and_version_is_visible_without_overlap_at_all_dpi",
                    imports[4],
                ),
                (
                    "hero_presents_one_buffered_frame_reuses_resources_preserves_clip_and_never_erases",
                    hero_edits[0],
                ),
                (
                    "native_edit_padding_and_single_line_ink_center_at_all_dpi",
                    hero_edits[1],
                ),
                (
                    "native_edit_padding_preserves_selection_scroll_single_line_semantics_and_full_hitbox",
                    hero_edits[2],
                ),
                (
                    "custom_window_disables_native_nonclient_frame_during_activation",
                    window_chrome.0,
                ),
                (
                    "navigation_click_and_dialog_tab_preserve_owner_draw_buttons",
                    window_chrome.1,
                ),
                (
                    "window_width_is_fixed_height_can_grow_and_maximize_is_blocked",
                    home_design[0],
                ),
                (
                    "short_469_logical_workarea_keeps_connect_add_profile_and_navigation_visible",
                    home_design[1],
                ),
                (
                    "navigation_selected_hover_and_focus_fill_the_entire_equal_cell",
                    home_design[2],
                ),
                (
                    "connection_feedback_repaints_only_button_and_stops_offpage_hidden_minimized",
                    home_design[3],
                ),
                (
                    "all_home_phases_have_correct_russian_and_english_actions_at_all_dpi",
                    home_design[4],
                ),
                (
                    "mushroom_cells_are_square_physical_pixels_at_all_dpi",
                    home_design[5],
                ),
                (
                    "navigation_icons_have_distinct_silhouettes_and_square_cells_at_all_dpi",
                    home_design[6],
                ),
                ("five_profiles_keyboard_scroll", keyboard_scroll),
                ("twelve_profiles_mouse_wheel_child_and_parent", mouse_wheel),
                ("last_profile_selected_by_mouse", last_by_mouse),
                ("mouse_wheel_at_dpi_200", dpi_wheel),
                ("pixel_scrollbar_mouse_drag", scrollbar_drag),
                ("small_window_four_profiles_and_wheel", small_wheel),
                ("close_hides_to_tray_and_keeps_vpn", close_hides),
                ("exit_supersedes_pending_connect", exit_cancels_connect),
                (
                    "exit_cleanup_failure_reopens_recovery",
                    exit_failure_reopens,
                ),
                (
                    "exit_terminal_success_destroys_window",
                    exit_stops_then_destroys,
                ),
                (
                    "lost_exit_without_cleanup_proof_remains_pending",
                    lost_exit_without_proof_waits,
                ),
                (
                    "lost_exit_status_error_with_idle_metadata_keeps_window",
                    unconfirmed_exit_cases[0],
                ),
                (
                    "lost_exit_watchdog_with_idle_metadata_reopens_recovery",
                    unconfirmed_exit_cases[1],
                ),
                ("upgrade_retire_preserves_vpn", retire_preserves_vpn),
                (
                    "volatile_status_does_not_redraw_relayout_or_scroll",
                    quiet_status,
                ),
                (
                    "two_hundred_paints_reuse_buffer_without_gdi_leak",
                    stable_render_resources,
                ),
                (
                    "scrolled_page_clears_entire_backbuffer",
                    scrolled_buffer_clear,
                ),
                (
                    "fractional_dpi_125_150_four_profiles_and_wheel",
                    fractional_dpi,
                ),
                ("terminal_failure_clears_busy", terminal_failure),
                ("late_connect_cannot_override_disconnect", stale_connect),
                ("lost_controller_wait_is_bounded", bounded_wait),
                ("cancel_enabled_while_busy", cancel_enabled),
                ("cancel_click_returns_idle", cancel_click),
                ("dpi_200_captured", true),
                ("status_preserves_unsaved_editors", status_preserves_edits),
                (
                    "compact_status_preserves_model_editors_and_layout",
                    compact_status_preserves_edits,
                ),
                (
                    "stale_compact_status_fetches_full_snapshot",
                    stale_compact_status_refetches,
                ),
                (
                    "background_action_preserves_unsaved_editors",
                    action_preserves_edits,
                ),
                ("duplicate_controller_reuses_window", duplicate_reused),
                ("tray_activation_restores_window", tray_restored),
                (
                    "distinct_roots_have_distinct_window_identity",
                    distinct_roots,
                ),
                (
                    "language_switch_preserves_editors_model_caret_and_scroll",
                    locale_preserves_edits,
                ),
                ("english_control_captions_are_complete", english_captions),
                (
                    "language_preference_persists_in_smoke_root",
                    locale_persistence,
                ),
                (
                    "automatic_language_restores_system_choice",
                    auto_restores_system,
                ),
                (
                    "russian_english_roundtrip_preserves_raw_edits",
                    russian_roundtrip,
                ),
                (
                    "profile_names_are_never_translated",
                    profile_names_untranslated,
                ),
                (
                    "unknown_backend_error_retains_actual_details_in_both_languages",
                    unknown_error_retained,
                ),
                (
                    "known_windows_error_translates_without_mutating_source",
                    known_error_translated,
                ),
                (
                    "connection_error_wraps_two_visible_lines_with_full_details_access",
                    wrapped_error_readable,
                ),
                (
                    "caption_glyphs_use_square_blocks_at_100_125_150_200",
                    caption_blocks,
                ),
                (
                    "baked_texture_fills_and_48_dpi_cycles_reuse_cache",
                    texture_cache,
                ),
                ("caption_button_minimizes_window", caption_minimizes),
                (
                    "minimized_production_idle_timer_skips_status",
                    minimized_idle_quiet,
                ),
                (
                    "minimized_pending_operation_still_reconciles",
                    minimized_pending_checked,
                ),
                ("restored_window_resumes_status", minimized_restore_resumes),
                (
                    "tray_restores_window_after_minimize_then_close",
                    tray_restores_minimized,
                ),
                (
                    "hidden_watchdog_kills_idle_native_timer",
                    watchdog_stops_timer[0],
                ),
                (
                    "minimized_watchdog_kills_idle_native_timer",
                    watchdog_stops_timer[1],
                ),
            ];
            let report: serde_json::Map<String, Value> = checks
                .iter()
                .map(|(name, passed)| ((*name).to_owned(), Value::Bool(*passed)))
                .collect();
            std::fs::write(
                s.root.join("native-ui-checks.json"),
                serde_json::to_vec_pretty(&Value::Object(report)).unwrap(),
            )
            .map_err(|e| e.to_string())?;
            if checks.iter().any(|(_, passed)| !passed) {
                return Err("Native UI smoke interaction check failed".into());
            }
            DestroyWindow(hwnd);
        } else {
            tray(&s, true);
            s.status();
            SetTimer(hwnd, 1, 3000, None);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for font in &s.fonts {
            DeleteObject(*font);
        }
        for brushes in &s.mushroom_brushes {
            for brush in brushes {
                DeleteObject(*brush);
            }
        }
        if !s.buffer_dc.is_null() {
            SelectObject(s.buffer_dc, s.buffer_old);
            DeleteObject(s.buffer_bitmap);
            DeleteDC(s.buffer_dc);
        }
        Ok(())
    }
}
unsafe fn screenshot(hwnd: HWND, path: &std::path::Path) -> Result<(), String> {
    let mut r = std::mem::zeroed();
    GetWindowRect(hwnd, &mut r);
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    let source = GetDC(hwnd);
    let dc = CreateCompatibleDC(source);
    let bmp = CreateCompatibleBitmap(source, w, h);
    let old = SelectObject(dc, bmp);
    PrintWindow(hwnd, dc, 0);
    // Offscreen composited PrintWindow does not dispatch print messages to
    // the subclassed trackbar. Ask that native control to paint its snapshot,
    // preserving the real HWND region (including viewport/corner clipping).
    let slider = GetDlgItem(hwnd, 4024);
    if !slider.is_null() && IsWindowVisible(slider) != 0 {
        let mut bounds = RECT::default();
        GetWindowRect(slider, &mut bounds);
        MapWindowPoints(null_mut(), hwnd, &mut bounds as *mut _ as *mut POINT, 2);
        let saved = SaveDC(dc);
        let region = CreateRectRgn(0, 0, 0, 0);
        if GetWindowRgn(slider, region) != 0 {
            OffsetRgn(region, bounds.left, bounds.top);
            ExtSelectClipRgn(dc, region, RGN_AND);
        } else {
            IntersectClipRect(dc, bounds.left, bounds.top, bounds.right, bounds.bottom);
        }
        SetViewportOrgEx(dc, bounds.left, bounds.top, null_mut());
        SendMessageW(slider, WM_PRINT, dc as usize, PRF_CLIENT as isize);
        RestoreDC(dc, saved);
        DeleteObject(region);
    }
    SelectObject(dc, old);
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = 40;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    GetDIBits(
        source,
        bmp,
        0,
        h as u32,
        pixels.as_mut_ptr().cast(),
        &mut info,
        DIB_RGB_COLORS,
    );
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(std::slice::from_raw_parts(
        &info.bmiHeader as *const _ as *const u8,
        40,
    ));
    bytes.extend_from_slice(&pixels);
    DeleteObject(bmp);
    DeleteDC(dc);
    ReleaseDC(hwnd, source);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod cleanup_request_tests {
    use super::*;

    #[test]
    fn authenticated_ready_controller_cleanup_preserves_request_and_skips_restart() {
        for action in ["Disconnect", "Exit"] {
            let request =
                json!({"action":action,"request_id":"exact-terminal-receipt","generation":7});
            let mut seen = Vec::new();
            let reply = controller_request(
                &request,
                |value| {
                    seen.push(value.clone());
                    if value["action"] == "Status" {
                        Ok(json!({"ok":true,"data":{"service_ready":true}}))
                    } else {
                        Ok(json!({"ok":true,"request_id":"exact-terminal-receipt"}))
                    }
                },
                || panic!("A ready QA/portable controller must not use installed recovery"),
            )
            .unwrap();
            assert_eq!(seen, vec![json!({"action":"Status"}), request]);
            assert_eq!(reply["request_id"], "exact-terminal-receipt");
        }
    }

    #[test]
    fn unavailable_or_unready_status_requires_checked_recovery_before_single_cleanup() {
        for status in [
            Err("pipe unavailable".into()),
            Ok(json!({"ok":false,"data":{"service_ready":true}})),
            Ok(json!({"ok":true,"data":{"service_ready":false}})),
            Ok(json!({"ok":true,"data":{"service_ready":"true"}})),
        ] {
            let order = RefCell::new(Vec::new());
            controller_request(
                &json!({"action":"Disconnect"}),
                |value| {
                    let action = value["action"].as_str().unwrap();
                    order.borrow_mut().push(action.to_owned());
                    if action == "Status" {
                        status.clone()
                    } else {
                        Ok(json!({"ok":true}))
                    }
                },
                || {
                    order.borrow_mut().push("checked-recovery".into());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                *order.borrow(),
                ["Status", "checked-recovery", "Disconnect"]
            );
        }
    }

    #[test]
    fn failed_recovery_does_not_mutate_and_lost_cleanup_receipt_is_never_retried() {
        let mut calls = 0;
        assert_eq!(
            controller_request(
                &json!({"action":"Exit"}),
                |_| {
                    calls += 1;
                    Err("unavailable".into())
                },
                || Err("untrusted installation".into()),
            ),
            Err("untrusted installation".into())
        );
        assert_eq!(calls, 1);
        let mut actions = Vec::new();
        assert_eq!(
            controller_request(
                &json!({"action":"Exit","request_id":"lost-receipt"}),
                |value| {
                    actions.push(value["action"].as_str().unwrap().to_owned());
                    if value["action"] == "Status" {
                        Ok(json!({"ok":true,"data":{"service_ready":true}}))
                    } else {
                        Err("lost receipt".into())
                    }
                },
                || panic!("An ambiguous cleanup must not trigger restart"),
            ),
            Err("lost receipt".into())
        );
        assert_eq!(actions, ["Status", "Exit"]);
    }

    #[test]
    fn ordinary_requests_have_no_status_probe_or_recovery() {
        let mut calls = 0;
        let request = json!({"action":"Connect"});
        controller_request(
            &request,
            |value| {
                calls += 1;
                assert_eq!(value, &request);
                Ok(json!({"ok":true}))
            },
            || panic!("Ordinary actions cannot start recovery"),
        )
        .unwrap();
        assert_eq!(calls, 1);
    }
}
