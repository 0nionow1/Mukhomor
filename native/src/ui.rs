#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    backend,
    i18n::{Language, Locale},
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
const HOME_WIDTH: i32 = 560;
const HOME_HEIGHT: i32 = 740;
const PULSE_TIMER: usize = 2;
const IMPORT_LIMIT: usize = 512 * 1024;
const MUSHROOM_GRID: [&str; 12] = [
    "............",
    "....IIII....",
    "..IITTIIII..",
    ".ITTIIITTTI.",
    "ITTTIITTTTTI",
    "ITTTTTTTIITI",
    "IIIIIIIIIIII",
    "....WWWW....",
    "....WWWW....",
    "...WWWWWW...",
    "...WWWWWW...",
    "............",
];
// STATIC owner drawing/notifications; these styles are absent from the
// windows-sys Controls feature but have stable Win32 values.
const PIXEL_SCROLLBAR_STYLE: u32 = 0x0000_000d | 0x0000_0100;
type LayoutKey = (usize, usize, i32, i32, i32, i32, bool, usize, bool, bool);
include!(concat!(env!("OUT_DIR"), "/theme.rs"));
const PAGES: [&str; 4] = ["Подключение", "Исключения", "DNS и списки", "Настройки"];
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
    "Поддомены · TLD",
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
fn mix_color(first: u32, second: u32, strength: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |color, shift| {
        let first = (first >> shift) & 255;
        let second = (second >> shift) & 255;
        color | ((first * (255 - strength) + second * strength) / 255) << shift
    })
}
unsafe fn make_mushroom_brushes() -> Vec<[HBRUSH; 3]> {
    let mut colors = vec![[MUTED, LINE, MUTED], [INDIGO, TEAL, STEM]];
    for strength in [
        144, 148, 160, 179, 200, 224, 242, 252, 255, 252, 242, 224, 200, 179, 160, 148,
    ] {
        colors.push([
            mix_color(MUTED, INDIGO, strength),
            mix_color(MUTED, TEAL, strength),
            mix_color(MUTED, STEM, strength),
        ]);
    }
    colors
        .into_iter()
        .map(|row| row.map(|color| CreateSolidBrush(color)))
        .collect()
}
struct HomeLayout {
    hero_y: i32,
    hero_h: i32,
    hero_pixel: i32,
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
struct State {
    base: PathBuf,
    root: PathBuf,
    hwnd: HWND,
    controls: HashMap<i32, HWND>,
    captions: HashMap<i32, String>,
    locale: Locale,
    page: usize,
    category: usize,
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
    status_attempts: Cell<u64>,
    pending_id: Option<String>,
    pending_since: Option<Instant>,
    request_nonce: String,
    checks: HashMap<i32, bool>,
    notice: String,
    next: Option<Value>,
    dpi: i32,
    fonts: Vec<HFONT>,
    mushroom_brushes: Vec<[HBRUSH; 3]>,
    pulse_frame: u8,
    pulse_running: bool,
    pulse_enabled: bool,
    pulse_ticks: Cell<u64>,
    hero_draws: Cell<u64>,
    nav_hover: Option<i32>,
    smoke_min_height: Option<i32>,
    textures: crate::texture::Textures,
    paper: HBRUSH,
    white: HBRUSH,
    smoke: bool,
    hidden: bool,
    minimized: bool,
    scroll: i32,
    scroll_dragging: bool,
    profile_wheel: i32,
    scrollbar_drag: Option<(i32, i32)>,
    exit_pending: bool,
    smoke_stopped_clean: bool,
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
    hero_presentations: Cell<u64>,
    edit_ink_cache: RefCell<HashMap<usize, (TEXTMETRICW, i32, i32)>>,
}
struct Answer {
    response: Result<Value, String>,
    action: String,
    generation: u64,
    request_id: Option<String>,
}
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
        let actions_y = visible - if tiny { 122 } else { 134 };
        let header_y = if tiny {
            216
        } else if compact {
            252
        } else {
            324
        } + if error { 44 } else { 0 };
        let list_y = header_y + 28;
        HomeLayout {
            hero_y: if compact { 88 } else { 96 },
            hero_h: if tiny {
                124
            } else if compact {
                160
            } else {
                196
            },
            hero_pixel: if tiny {
                6
            } else if compact {
                8
            } else {
                10
            },
            detail_y: if tiny {
                208
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
        let active = self.pulse_enabled
            && self.phase() == "connecting"
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
            SetTimer(self.hwnd, PULSE_TIMER, 125, None);
        } else {
            KillTimer(self.hwnd, PULSE_TIMER);
            self.pulse_frame = 0;
        }
    }
    fn d(&self, n: i32) -> i32 {
        n * self.dpi / 96
    }
    unsafe fn set_dpi_fonts(&mut self, dpi: i32) {
        self.dpi = dpi;
        self.textures.set_dpi(dpi);
        self.paper = self.textures.brush(BG);
        self.white = self.textures.brush(PANEL_ALT);
        self.edit_ink_cache.borrow_mut().clear();
        let old = std::mem::replace(&mut self.fonts, make_fonts(dpi));
        for (&id, &control) in &self.controls {
            SendMessageW(
                control,
                WM_SETFONT,
                self.fonts[if [1006, 1020, 2102, 3006, 3007, 3008, 3009, 3013, 3014].contains(&id) {
                    4
                } else {
                    0
                }] as usize,
                1,
            );
        }
        if let Some(&list) = self.controls.get(&1001) {
            SendMessageW(list, LB_SETITEMHEIGHT, 0, self.d(44) as isize);
        }
        for font in old {
            DeleteObject(font);
        }
    }
    unsafe fn ctrl(&mut self, id: i32, class: &str, text: &str, style: u32) {
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
        if class == "EDIT" {
            SetWindowSubclass(h, Some(edit_procedure), 5, 0);
            format_edit(h, self.dpi, true);
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
            launch(
                self.hwnd,
                self.root.clone(),
                json!({"action":"Status"}),
                self.generation,
            );
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
    unsafe fn collect(&mut self) -> bool {
        if self.page == 2
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
        if self.page == 1 {
            self.draft["direct"][KEYS[self.category]] = self.lines(2102);
        }
        if self.page == 2 {
            let mut sets = vec![];
            for (id, name) in [(3001, "russia"), (3002, "steam"), (3003, "ozon")] {
                if self.checked(id) {
                    sets.push(name);
                }
            }
            self.draft["direct"]["domain_rule_sets"] = json!(sets);
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
        true
    }
    unsafe fn populate_profiles(&mut self) {
        let rename_draft = if self.rename_open {
            Some(self.text(1006))
        } else {
            None
        };
        let old_top = SendMessageW(self.controls[&1001], LB_GETTOPINDEX, 0, 0).max(0);
        SendMessageW(self.controls[&1001], WM_SETREDRAW, 0, 0);
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
        SendMessageW(self.controls[&1001], WM_SETREDRAW, 1, 0);
        InvalidateRect(self.controls[&1001], null(), 0);
        if let Some(&bar) = self.controls.get(&1010) {
            InvalidateRect(bar, null(), 0);
        }
    }
    unsafe fn populate_settings(&mut self) {
        self.set(2102, &joined(&self.draft["direct"][KEYS[self.category]]));
        for (id, name) in [(3001, "russia"), (3002, "steam"), (3003, "ozon")] {
            self.check(
                id,
                self.draft["direct"]["domain_rule_sets"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s == name)),
            );
        }
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
    unsafe fn sync_snapshot(&mut self, data: Value, action: &str) {
        let initial = !self.loaded;
        let profiles_changed = self.model["profiles"] != data["profiles"];
        let selection_changed = self.model["selected"] != data["selected"];
        self.model = data;
        if initial || action == "ApplySettings" {
            self.draft = self.model["settings"].clone();
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
        self.update_buttons();
        self.layout();
        InvalidateRect(self.hwnd, null(), 0);
    }
    unsafe fn update_buttons(&mut self) {
        let label = match self.phase() {
            "connecting" => "Отменить",
            "connected" => "Отключиться",
            "disconnecting" => "Отключаем…",
            "recovery" => "Вернуть сеть",
            _ => "Подключиться",
        };
        self.set(1007, label);
        for id in [1002, 1003, 1004, 1005, 2103, 2104, 3011, 3012, 4001, 4003] {
            let disabled = self.busy
                || self.model["busy"] == true
                || self.model["service_ready"] != true
                || ([1003, 1004, 1005].contains(&id) && self.profile_id().is_none());
            self.enable(id, !disabled);
        }
        let primary = self.model["service_ready"] == true
            && self.phase() != "disconnecting"
            && (self.can_disconnect() || (!self.busy && self.profile_id().is_some()));
        self.enable(1007, primary);
        self.enable(1001, !self.busy && self.model["busy"] != true);
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
    fn exit_is_confirmed_stopped(&self) -> bool {
        if !self.exit_pending || self.pending_action != "Exit" || self.pending_id.is_none() {
            return false;
        }
        if self.smoke {
            // Synthetic proof seam is inaccessible from the normal GUI and
            // never queries or changes an installed service in smoke tests.
            self.smoke_stopped_clean
        } else {
            backend::stopped_clean(&self.root)
        }
    }
    unsafe fn finish_confirmed_exit(&mut self) {
        self.busy = false;
        self.pending_id = None;
        self.pending_since = None;
        self.next = None;
        self.model["busy"] = json!(false);
        self.model["running"] = json!(false);
        self.model["dns_recovery_pending"] = json!(false);
        self.model["phase"] = json!("idle");
        self.finish_exit(true);
    }
    unsafe fn scroll_info(&self, id: i32) -> (i32, i32, i32, i32) {
        let handle = self.controls[&id];
        let mut rect = std::mem::zeroed();
        GetClientRect(handle, &mut rect);
        let (top, count, item) = if id == 1001 {
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
        if id == 1001 {
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
            0 => visible.max(460) - 80,
            1 => 700,
            2 => 1160,
            _ => 852,
        };
        (width, visible, content.max(visible - 80))
    }
    unsafe fn layout(&self) {
        if !self.controls.contains_key(&1001) {
            return;
        }
        let (width, visible, height) = self.dimensions();
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
        self.layout_key.set(Some(key));
        self.layout_passes.set(self.layout_passes.get() + 1);
        let left = ((width - 520) / 2).max(24);
        let area = width - left * 2;
        let mv = |id: i32, x: i32, y: i32, w: i32, h: i32| {
            if let Some(handle) = self.controls.get(&id) {
                let screen_y = y - if id >= 1000 { self.scroll } else { 0 };
                let drawn_height = if id >= 1000 {
                    (visible - 80 - screen_y).max(0).min(h)
                } else {
                    h
                };
                let page = if id >= 4000 {
                    3
                } else if id >= 3000 {
                    2
                } else if id >= 2000 {
                    1
                } else {
                    0
                };
                let mut show = id < 1000 || page == self.page;
                if id >= 1000 {
                    show &= screen_y >= if self.scroll > 0 { 88 } else { 0 }
                        && screen_y < visible - 80
                        && drawn_height > 0;
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
                        ShowWindow(*handle, SW_HIDE);
                    }
                    return;
                }
                let bounds = (self.d(x), self.d(screen_y), self.d(w), self.d(drawn_height));
                if self.control_bounds.borrow().get(&id) != Some(&bounds) {
                    MoveWindow(*handle, bounds.0, bounds.1, bounds.2, bounds.3, 0);
                    self.control_bounds.borrow_mut().insert(id, bounds);
                    self.geometry_moves.set(self.geometry_moves.get() + 1);
                    RedrawWindow(
                        *handle,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_NOERASE | RDW_FRAME,
                    );
                }
                if !was_visible {
                    ShowWindow(*handle, SW_SHOW);
                }
            }
        };
        let nav_width = width / 4;
        for n in 0..4 {
            mv(200 + n, n * nav_width, visible - 80, nav_width, 80);
        }
        mv(204, width - 90, 24, 28, 28);
        mv(205, width - 54, 24, 28, 28);
        let home = self.home_layout();
        mv(1007, (width - 240) / 2, home.hero_y, 240, home.hero_h);
        mv(1001, left, home.list_y, area - 16, home.list_h);
        mv(1010, width - left - 10, home.list_y, 10, home.list_h);
        mv(1011, width - left - 28, home.detail_y + 4, 28, 28);
        let import_y = if visible < 560 { 190 } else { 206 };
        let import_buttons_y = visible - 134;
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
        let category_width = (area - 8) / 2;
        for n in 0..6 {
            mv(
                2200 + n,
                left + (n % 2) * (category_width + 8),
                188 + (n / 2) * 42,
                category_width,
                34,
            );
        }
        mv(2102, left, 374, area - 16, height - 462);
        mv(2110, width - left - 10, 374, 10, height - 462);
        mv(2103, left, height - 68, 170, 42);
        mv(2104, width - left - 190, height - 68, 190, 42);
        for (id, y) in [
            (3001, 194),
            (3002, 254),
            (3003, 314),
            (3004, 474),
            (3005, 560),
        ] {
            mv(id, left + 14, y, area - 28, 42);
        }
        mv(3006, left, 658, area - 16, 104);
        mv(3010, width - left - 10, 658, 10, 104);
        let third = (area - 24) / 3;
        for (id, n) in [(3007, 0), (3008, 1), (3009, 2)] {
            mv(id, left + n * (third + 12), 808, third, 38);
        }
        for (id, n) in [(3013, 0), (3014, 1)] {
            mv(
                id,
                left + n * ((area - 12) / 2 + 12),
                898,
                (area - 12) / 2,
                38,
            );
        }
        mv(3011, left, 1008, area, 42);
        mv(3012, left, 1072, area, 42);
        mv(4001, left + 14, 206, area - 28, 42);
        let language_width = (area - 16) / 3;
        for n in 0..3 {
            mv(
                4010 + n,
                left + n * (language_width + 8),
                400,
                language_width,
                42,
            );
        }
        mv(4005, left, 534, area, 44);
        mv(4003, left, 678, (area - 12) / 2, 44);
        mv(4004, left + (area - 12) / 2 + 12, 678, (area - 12) / 2, 44);
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
        mushroom(self, dc, left, 24, 3);
        text(self, dc, "Mukhomor", left + 48, 34, 180, 30, 2, INK);
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
        if self.page != 0 {
            text(self, dc, PAGES[self.page], left, 112, area, 42, 1, INK);
            text(
                self,
                dc,
                match self.page {
                    1 => "Трафик по этим правилам идёт напрямую",
                    2 => "Российские сервисы и актуальные адреса",
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
                        visible - 182,
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
            1 => {
                let hint = match self.category {
                    0 => {
                        "cs2.exe, qbittorrent.exe — по одному имени на строку. TCP и UDP программы идут напрямую."
                    }
                    1 => {
                        "Полный путь отличает конкретную программу. Выбери установленный .exe или добавь путь вручную."
                    }
                    2 => {
                        "api.example.com — только точное имя. Без https://, порта и пути страницы."
                    }
                    3 => "example.com — домен с поддоменами. ru — вся зона .ru. рф — вся зона .рф.",
                    4 => "IPv4, IPv6 и подсети: 1.2.3.4, 1.2.3.0/24, 2001:db8::/32.",
                    _ => {
                        "Эти домены пойдут через VPN, даже если присутствуют в готовом списке сервисов."
                    }
                };
                text(self, dc, hint, left, 320, area, 48, 3, MUTED);
            }
            2 => {
                rounded(self, dc, left, 182, area, 188, PANEL, LINE);
                text(
                    self,
                    dc,
                    "ГОТОВЫЕ ИСКЛЮЧЕНИЯ",
                    left + 14,
                    182,
                    area - 28,
                    17,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Домены и поддомены обновляются автоматически.
Ozon также входит в российский список.",
                    left,
                    385,
                    area,
                    45,
                    3,
                    MUTED,
                );
                rounded(self, dc, left, 458, area, 153, PANEL, LINE);
                text(
                    self,
                    dc,
                    "IP ПО ДОМЕНАМ",
                    left + 14,
                    444,
                    area - 28,
                    19,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Доменные правила работают и без IP-кэша.",
                    left + 14,
                    524,
                    area - 28,
                    28,
                    3,
                    MUTED,
                );
                text(
                    self,
                    dc,
                    "Cloudflare → Google · A / AAAA · CNAME · TTL",
                    left,
                    619,
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
                    641,
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
                        782,
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
                        872,
                        (area - 12) / 2,
                        25,
                        3,
                        MUTED,
                    );
                }
                text(
                    self,
                    dc,
                    "Учитывается TTL ответа. При сбое остаются последние
проверенные адреса в пределах заданного срока.",
                    left,
                    952,
                    area,
                    46,
                    3,
                    MUTED,
                );
            }
            _ => {
                rounded(self, dc, left, 192, area, 148, PANEL, LINE);
                text(
                    self,
                    dc,
                    "Сразу после запуска Windows",
                    left + 14,
                    260,
                    area - 28,
                    25,
                    2,
                    INK,
                );
                text(
                    self,
                    dc,
                    "Подключится последний выбранный сервер.
Можно отключить VPN в любой момент.",
                    left + 14,
                    294,
                    area - 28,
                    43,
                    3,
                    MUTED,
                );
                text(self, dc, "ЯЗЫК ИНТЕРФЕЙСА", left, 366, area, 20, 3, MUTED);
                text(self, dc, "ЛЕГКО В ФОНЕ", left, 478, area, 20, 3, MUTED);
                text(
                    self,
                    dc,
                    "Можно закрыть окно — подключение продолжит работать.
В трее интерфейс ждёт без фоновой перерисовки.",
                    left,
                    504,
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
                    618,
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
                    645,
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
                    756,
                    area,
                    47,
                    3,
                    MUTED,
                );
            }
        }
        SetViewportOrgEx(dc, 0, 0, null_mut());
        if self.scroll > 0 {
            let header = RECT {
                left: 0,
                top: 0,
                right: client.right,
                bottom: self.d(88),
            };
            surface_fill(self, dc, &header, BG);
            mushroom(self, dc, left, 24, 3);
            text(self, dc, "Mukhomor", left + 48, 34, 180, 30, 2, INK);
        }
        let (_, _, content) = self.dimensions();
        if content > visible - 80 {
            let travel = visible - 96;
            let thumb = (travel * (visible - 80) / content).max(42);
            let y = 8 + (travel - thumb) * self.scroll / (content - visible + 80).max(1);
            rounded(self, dc, width - 7, 8, 3, travel, LINE, LINE);
            rounded(self, dc, width - 7, y, 3, thumb, INDIGO, INDIGO);
        }
        let nav = RECT {
            left: 0,
            top: self.d(visible - 80),
            right: client.right,
            bottom: client.bottom,
        };
        surface_fill(self, dc, &nav, BG);
    }
}
fn scrollbar_id(target: i32) -> i32 {
    match target {
        1001 => 1010,
        1020 => 1024,
        2102 => 2110,
        _ => 3010,
    }
}
fn scrollbar_target(bar: i32) -> i32 {
    match bar {
        1010 => 1001,
        1024 => 1020,
        2110 => 2102,
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
fn launch(hwnd: HWND, root: PathBuf, request: Value, generation: u64) {
    let window = hwnd as isize;
    let action = request["action"].as_str().unwrap_or("Status").to_owned();
    let request_id = request["request_id"].as_str().map(str::to_owned);
    thread::spawn(move || {
        let pointer = Box::into_raw(Box::new(Answer {
            response: backend::rpc(&root, &request),
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
    surface_fill(s, dc, &rect, color);
    if border != color && border != LINE {
        FrameRect(dc, &rect, s.textures.brush(border));
    }
}
unsafe fn surface_fill(s: &State, dc: HDC, rect: &RECT, color: u32) {
    s.textures.fill(dc, rect, color);
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
    SetTextColor(dc, color);
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
    SetTextColor(dc, color);
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
    SetTextColor(dc, color);
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
unsafe fn mushroom(s: &State, dc: HDC, x: i32, y: i32, pixel: i32) {
    mushroom_palette(s, dc, x, y, pixel, 1);
}
fn mushroom_grid_bounds(s: &State, x: i32, y: i32, pixel: i32) -> (i32, i32, i32) {
    let cell = ((pixel * s.dpi + 48) / 96).max(1);
    let inset = (s.d(pixel * 12) - cell * 12) / 2;
    (s.d(x) + inset, s.d(y) + inset, cell)
}
unsafe fn mushroom_palette(s: &State, dc: HDC, x: i32, y: i32, pixel: i32, palette: usize) {
    let (origin_x, origin_y, cell) = mushroom_grid_bounds(s, x, y, pixel);
    let brushes = s.mushroom_brushes[palette];
    for (row, line) in MUSHROOM_GRID.iter().enumerate() {
        for (col, ch) in line.bytes().enumerate() {
            let brush = match ch {
                b'I' => brushes[0],
                b'T' => brushes[1],
                b'W' => brushes[2],
                _ => continue,
            };
            let r = RECT {
                left: origin_x + col as i32 * cell,
                top: origin_y + row as i32 * cell,
                right: origin_x + (col as i32 + 1) * cell,
                bottom: origin_y + (row as i32 + 1) * cell,
            };
            FillRect(dc, &r, brush);
        }
    }
}
fn icon_grid(kind: usize) -> &'static [&'static str; 12] {
    const GRIDS: [[&str; 12]; 8] = [
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
    ];
    &GRIDS[kind.min(GRIDS.len() - 1)]
}
fn icon_grid_bounds(s: &State, x: i32, y: i32, size: i32) -> (i32, i32, i32) {
    let cell = (((size / 12).max(1) * s.dpi + 48) / 96).max(1);
    let inset = (s.d(size) - cell * 12) / 2;
    (s.d(x) + inset, s.d(y) + inset, cell)
}
unsafe fn icon(s: &State, dc: HDC, kind: usize, x: i32, y: i32, size: i32, color: u32) {
    let (origin_x, origin_y, cell) = icon_grid_bounds(s, x, y, size);
    let brush = CreateSolidBrush(color);
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
    DeleteObject(brush);
}
// An integer physical pixel cell keeps these glyphs square at fractional DPI.
// Caption hitboxes and button semantics remain native, independent of the art.
unsafe fn caption_glyph(dc: HDC, rect: &RECT, dpi: i32, close: bool, color: u32) {
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
    let cell = ((3 * dpi + 48) / 96)
        .max(1)
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
    icon.hIcon = product_icon();
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
        ACTIVATE => {
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
            if x >= width - 16 && content > visible - 80 {
                s.scroll_dragging = true;
                SetCapture(hwnd);
                s.scroll = ((y - 8) * (content - visible + 80) / (visible - 96).max(1))
                    .clamp(0, content - visible + 80);
                s.layout();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_MOUSEMOVE if s.scroll_dragging => {
            let (_, visible, content) = s.dimensions();
            let y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32 * 96 / s.dpi;
            s.scroll = ((y - 8) * (content - visible + 80) / (visible - 96).max(1))
                .clamp(0, (content - visible + 80).max(0));
            s.layout();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONUP if s.scroll_dragging => {
            s.scroll_dragging = false;
            ReleaseCapture();
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
            let target = if s.ensure_buffer(dc, (rect.right, rect.bottom)) {
                s.buffer_dc
            } else {
                dc
            };
            // Clear in screen coordinates before applying the content scroll.
            // Otherwise a scrolled page leaves the lower backbuffer unchanged
            // and pixels from another page show through its empty spaces.
            SetViewportOrgEx(target, 0, 0, null_mut());
            SetBrushOrgEx(target, 0, 0, null_mut());
            surface_fill(s, target, &rect, BG);
            SetViewportOrgEx(target, 0, -s.d(s.scroll), null_mut());
            s.paint(target);
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
                if s.pending_id.is_none() {
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
            s.scroll = s
                .scroll
                .min((s.dimensions().2 - r.bottom * 96 / s.dpi + 80).max(0));
            s.layout();
            s.sync_pulse();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DPICHANGED => {
            s.set_dpi_fonts((wparam & 0xffff) as i32);
            let suggested = &*(lparam as *const RECT);
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
            0
        }
        LIST_SCROLL => {
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
            s.scroll_profiles((wparam >> 16) as i16 as i32);
            0
        }
        WM_VSCROLL | WM_MOUSEWHEEL => {
            let mut r = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let visible = r.bottom * 96 / s.dpi - 80;
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
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            SetTextColor(wparam as HDC, INK);
            SetBkMode(wparam as HDC, TRANSPARENT as i32);
            s.white as isize
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            SetTextColor(wparam as HDC, INK);
            SetBkColor(wparam as HDC, PANEL_ALT);
            SetBrushOrgEx(wparam as HDC, 0, 0, null_mut());
            s.white as isize
        }
        WM_DRAWITEM => {
            let draw = &*(lparam as *const DRAWITEMSTRUCT);
            let dc = draw.hDC;
            let r = draw.rcItem;
            let id = draw.CtlID as i32;
            let origin = s
                .control_bounds
                .borrow()
                .get(&id)
                .copied()
                .unwrap_or((0, 0, 0, 0));
            SetBrushOrgEx(dc, -origin.0, -origin.1, null_mut());
            if [1010, 1024, 2110, 3010].contains(&id) {
                surface_fill(s, dc, &r, BG);
                let (top, count, rows, height) = s.scroll_info(scrollbar_target(id));
                if count <= rows {
                    return 1;
                }
                let thumb = (height * rows / count).max(s.d(12)).min(height);
                let travel = (height - thumb).max(0);
                let pos = top * travel / (count - rows).max(1);
                let brush = CreateSolidBrush(if count > rows { INDIGO } else { LINE });
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
                let palette = match phase {
                    "connected" => 1,
                    "connecting" => 2 + s.pulse_frame as usize,
                    _ => 0,
                };
                let pixel = s.home_layout().hero_pixel;
                mushroom_palette(s, target, (w - pixel * 12) / 2, 8, pixel, palette);
                let color = if draw.itemState & ODS_DISABLED != 0 {
                    MUTED
                } else if phase == "recovery" {
                    DANGER
                } else if draw.itemState & (ODS_FOCUS | ODS_SELECTED) != 0 {
                    TEAL
                } else {
                    INK
                };
                centered(s, target, &s.text(id), 0, h - 40, w, 31, 2, color);
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
            if id == 1001 && draw.itemID == u32::MAX {
                surface_fill(s, dc, &r, PANEL);
                return 1;
            }
            if id == 1001 && draw.itemID != u32::MAX {
                let selected = draw.itemState & ODS_SELECTED != 0;
                surface_fill(s, dc, &r, if selected { PANEL_ALT } else { PANEL });
                if let Some(item) = s.model["profiles"]
                    .as_array()
                    .and_then(|a| a.get(draw.itemID as usize))
                {
                    let y = r.top * 96 / s.dpi;
                    let width = (r.right - r.left) * 96 / s.dpi;
                    let selected_color = if selected { TEAL } else { MUTED };
                    icon(s, dc, 0, 18, y + 12, 20, selected_color);
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
                        id == 205,
                        if id == 205 && pressed { DANGER } else { MUTED },
                    );
                } else if nav {
                    let hover = s.nav_hover == Some(id);
                    surface_fill(
                        s,
                        dc,
                        &r,
                        if pressed || focus {
                            LINE
                        } else if chosen || hover {
                            PANEL_ALT
                        } else {
                            BG
                        },
                    );
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
                    FillRect(dc, &r, s.white);
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
                        if on { c } else { BG },
                        if on { c } else { LINE },
                    );
                    if on {
                        icon(s, dc, 4, w - 31, 12, 18, BG);
                    }
                    text(
                        s,
                        dc,
                        &s.text(id),
                        0,
                        12,
                        w - 56,
                        27,
                        0,
                        if disabled { MUTED } else { INK },
                    );
                } else {
                    let category = (2200..2206).contains(&id);
                    let selected_category = (category && id - 2200 == s.category as i32)
                        || (id
                            == match s.locale.language() {
                                Language::Auto => 4010,
                                Language::Russian => 4011,
                                Language::English => 4012,
                            });
                    let accent = [1002, 1021, 2104, 3012].contains(&id);
                    let bg = if disabled {
                        PANEL
                    } else if pressed || selected_category {
                        LINE
                    } else if accent {
                        INDIGO
                    } else {
                        PANEL_ALT
                    };
                    rounded(s, dc, 0, 0, w, h, bg, if focus { TEAL } else { bg });
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
                        } else {
                            INK
                        },
                    );
                }
            }
            1
        }
        WM_TIMER => {
            if wparam == PULSE_TIMER {
                s.sync_pulse();
                if s.pulse_running {
                    s.pulse_frame = (s.pulse_frame + 1) % (s.mushroom_brushes.len() - 2) as u8;
                    s.pulse_ticks.set(s.pulse_ticks.get() + 1);
                    let pixel = s.home_layout().hero_pixel;
                    let (x, y, cell) = mushroom_grid_bounds(s, (240 - pixel * 12) / 2, 8, pixel);
                    let rect = RECT {
                        left: x,
                        top: y,
                        right: x + cell * 12,
                        bottom: y + cell * 12,
                    };
                    InvalidateRect(s.controls[&1007], &rect, 0);
                }
                return 0;
            }
            if wparam == 1 && (s.pending_id.is_some() || (!s.hidden && IsIconic(hwnd) == 0)) {
                if s.pending_since
                    .is_some_and(|started| started.elapsed().as_secs() > 115)
                {
                    if s.exit_is_confirmed_stopped() {
                        s.finish_confirmed_exit();
                        return 0;
                    }
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
                    if s.hidden || IsIconic(hwnd) != 0 {
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
            match answer.response {
                Ok(reply) if reply["ok"] == true => {
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
                    }
                    advance = terminal == Some(true);
                    completed = terminal;
                }
                Ok(reply) => {
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
                    if s.exit_is_confirmed_stopped() {
                        s.finish_confirmed_exit();
                        return 0;
                    }
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
            if (s.hidden || IsIconic(hwnd) != 0) && s.pending_id.is_none() {
                KillTimer(hwnd, 1);
            }
            s.update_buttons();
            s.layout();
            if before != s.visible_state() {
                InvalidateRect(hwnd, null(), 0);
                InvalidateRect(s.controls[&1007], null(), 0);
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xffff) as i32;
            let event = ((wparam >> 16) & 0xffff) as u32;
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
                InvalidateRect(s.controls[&(200 + old_page as i32)], null(), 0);
                InvalidateRect(s.controls[&id], null(), 0);
                s.scroll = 0;
                s.layout();
                s.sync_pulse();
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            match id {
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
                    let menu = CreatePopupMenu();
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        1008,
                        wide(&s.locale.text("Переименовать")).as_ptr(),
                    );
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        1005,
                        wide(&s.locale.text("Удалить профиль")).as_ptr(),
                    );
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
                    if s.page == 2
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
                3001..=3005 => {
                    s.check(id, !s.checked(id));
                }
                4001 => {
                    s.check(4001, !s.checked(4001));
                    s.request(json!({"action":"Autoconnect","enabled":s.checked(4001)}));
                }
                4003 => s.request(json!({"action":"Diagnose"})),
                4004 => shell_open(s, &s.root.join("runtime").to_string_lossy(), None, "open"),
                4005 => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
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
                let menu = CreatePopupMenu();
                AppendMenuW(
                    menu,
                    MF_STRING,
                    1,
                    wide(&s.locale.text("Открыть настройки")).as_ptr(),
                );
                AppendMenuW(
                    menu,
                    MF_STRING,
                    2,
                    wide(&s.locale.text("Отключить VPN")).as_ptr(),
                );
                AppendMenuW(
                    menu,
                    MF_STRING,
                    3,
                    wide(&s.locale.text("Выход из Mukhomor")).as_ptr(),
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
                if s.pending_id.is_none() {
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
            KillTimer(hwnd, 1);
            KillTimer(hwnd, PULSE_TIMER);
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
unsafe fn single_instance(class: &str) -> Result<Option<OwnedHandle>, String> {
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
            && add.bottom - rectangle.top <= s.d(s.dimensions().1 - 80)
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
    for _ in 0..16 {
        ValidateRect(s.controls[&1007], null());
        SendMessageW(hwnd, WM_TIMER, PULSE_TIMER, 0);
        let mut update: RECT = std::mem::zeroed();
        checks[3] &= GetUpdateRect(s.controls[&1007], &mut update, 0) != 0
            && update.right - update.left == 120
            && update.bottom - update.top == 120;
        UpdateWindow(s.controls[&1007]);
    }
    checks[3] &= s.pulse_ticks.get() == ticks + 16
        && s.pulse_frame == 0
        && s.parent_paints.get() == parent
        && s.layout_passes.get() == layouts
        && s.geometry_moves.get() == moves;
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
    }
    s.model["phase"] = json!("idle");
    s.busy = false;
    s.pending_action.clear();
    s.update_buttons();
    checks[3] &= !s.pulse_running;
    let dc = CreateCompatibleDC(s.buffer_dc);
    let bitmap = CreateCompatibleBitmap(s.buffer_dc, 400, 400);
    if dc.is_null() || bitmap.is_null() {
        return Err("Could not create design smoke bitmap".into());
    }
    let old = SelectObject(dc, bitmap);
    for (id, state, color) in [
        (200, 0, PANEL_ALT),
        (203, 0, PANEL_ALT),
        (200, ODS_FOCUS, LINE),
        (203, ODS_FOCUS, LINE),
        (200, ODS_SELECTED, LINE),
    ] {
        s.nav_hover = if id == 203 { Some(id) } else { None };
        let mut draw: DRAWITEMSTRUCT = std::mem::zeroed();
        draw.CtlID = id as u32;
        draw.itemState = state;
        draw.hDC = dc;
        draw.rcItem = RECT {
            left: 0,
            top: 0,
            right: 140,
            bottom: 80,
        };
        SendMessageW(hwnd, WM_DRAWITEM, id as usize, &draw as *const _ as isize);
        let bounds = s.control_bounds.borrow()[&id];
        for (x, y) in (0..140)
            .flat_map(|x| [(x, 0), (x, 79)])
            .chain((0..80).flat_map(|y| [(0, y), (139, y)]))
        {
            checks[2] &=
                GetPixel(dc, x, y) == crate::texture::sample(color, 96, bounds.0 + x, bounds.1 + y);
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
        for pixel in [3, 10] {
            let rect = RECT {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            };
            FillRect(dc, &rect, solid);
            mushroom_palette(s, dc, 20, 20, pixel, 1);
            let (x, y, cell) = mushroom_grid_bounds(s, 20, 20, pixel);
            for (row, line) in MUSHROOM_GRID.iter().enumerate() {
                for (column, value) in line.bytes().enumerate() {
                    let color = match value {
                        b'I' => INDIGO,
                        b'T' => TEAL,
                        b'W' => STEM,
                        _ => BG,
                    };
                    for (a, b) in [(0, 0), (cell - 1, 0), (0, cell - 1), (cell - 1, cell - 1)] {
                        checks[5] &=
                            GetPixel(dc, x + column as i32 * cell + a, y + row as i32 * cell + b)
                                == color;
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
            let brand_right = s.d(72 + measured_width(s, s.buffer_dc, "Mukhomor", 2));
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
    let original_scroll = s.scroll;
    let original_rename = s.rename_open;
    let original_language = s.locale.language();
    let original_model = s.model.clone();
    let original_busy = s.busy;
    let original_action = s.pending_action.clone();
    let original_frame = s.pulse_frame;
    let ids = [1006, 2102, 3006, 3007, 3008, 3009, 3013, 3014];
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
        for page in 0..3 {
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
                        inset > 0
                    }
                    && (GetWindowLongPtrW(control, GWL_STYLE) as u32 & ES_MULTILINE as u32 != 0)
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
    s.busy = true;
    s.pending_action = "Connect".into();
    s.model["phase"] = json!("connecting");
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
    IntersectClipRect(dc, 60, 8, 180, 128);
    for frame in 0..200 {
        s.pulse_frame = (frame % 16) as u8;
        draw.itemState = match frame % 4 {
            0 => 0,
            1 => ODS_FOCUS,
            2 => ODS_SELECTED,
            _ => ODS_FOCUS | ODS_SELECTED,
        };
        SendMessageW(s.hwnd, WM_DRAWITEM, 1007, &draw as *const _ as isize);
        let mut colors = [0; 3];
        for (color, brush) in colors.iter_mut().zip(s.mushroom_brushes[2 + frame % 16]) {
            let mut info: LOGBRUSH = std::mem::zeroed();
            GetObjectW(
                brush,
                std::mem::size_of_val(&info) as i32,
                &mut info as *mut _ as *mut std::ffi::c_void,
            );
            *color = info.lbColor;
        }
        let origin = s.control_bounds.borrow()[&1007];
        for (row, line) in MUSHROOM_GRID.iter().enumerate() {
            for (column, value) in line.bytes().enumerate() {
                let x = 60 + column as i32 * 10 + 5;
                let y = 8 + row as i32 * 10 + 5;
                let color = match value {
                    b'I' => colors[0],
                    b'T' => colors[1],
                    b'W' => colors[2],
                    _ => crate::texture::sample(BG, 96, origin.0 + x, origin.1 + y),
                };
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
    s.set_dpi_fonts(original_dpi);
    s.change_language(original_language);
    for (id, value) in original_texts {
        s.set(id, &value);
    }
    s.page = original_page;
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
            caption_glyph(dc, &rect, dpi, close, MUTED);
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
unsafe fn format_edit(hwnd: HWND, dpi: i32, frame: bool) {
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
    if GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & ES_MULTILINE as u32 != 0 {
        let mut client: RECT = std::mem::zeroed();
        let mut previous: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        SendMessageW(hwnd, EM_GETRECT, 0, &mut previous as *mut _ as isize);
        let padding = (10 * dpi / 96).max(1);
        let rect = RECT {
            left: margin.min(client.right / 2),
            top: padding.min(client.bottom / 2),
            right: (client.right - margin).max(margin + 1),
            bottom: (client.bottom - padding).max(padding + 1),
        };
        if (previous.left, previous.top, previous.right, previous.bottom)
            != (rect.left, rect.top, rect.right, rect.bottom)
        {
            let top = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
            SendMessageW(hwnd, EM_SETRECTNP, 0, &rect as *const _ as isize);
            let after = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
            if top != after {
                SendMessageW(hwnd, EM_LINESCROLL, 0, top - after);
            }
        }
    } else if frame {
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
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
    let s = &*pointer;
    let single = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & ES_MULTILINE as u32 == 0;
    match message {
        WM_NCCALCSIZE if single => {
            DefSubclassProc(hwnd, message, wparam, lparam);
            let rect = if wparam == 0 {
                &mut *(lparam as *mut RECT)
            } else {
                &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]
            };
            // Tiny5 has large internal leading; the dotted i captures its
            // normal letter ink, including dots that extend above capitals.
            // Centering the entire line box would put that ink too low.
            let (metrics, cap_top, cap_height) = edit_cap_metrics(hwnd);
            let height = rect.bottom - rect.top;
            let inset =
                ((height - cap_height) / 2 - cap_top).clamp(0, (height - metrics.tmHeight).max(0));
            rect.top += inset;
            rect.bottom = (rect.top + metrics.tmHeight).min(rect.bottom);
            0
        }
        WM_NCHITTEST if single => HTCLIENT as isize,
        WM_NCPAINT if single => {
            let dc = GetWindowDC(hwnd);
            let saved = SaveDC(dc);
            let mut window: RECT = std::mem::zeroed();
            let mut client: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut window);
            GetClientRect(hwnd, &mut client);
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut origin);
            ExcludeClipRect(
                dc,
                origin.x - window.left,
                origin.y - window.top,
                origin.x - window.left + client.right,
                origin.y - window.top + client.bottom,
            );
            let rect = RECT {
                left: 0,
                top: 0,
                right: window.right - window.left,
                bottom: window.bottom - window.top,
            };
            FillRect(dc, &rect, s.white);
            RestoreDC(dc, saved);
            ReleaseDC(hwnd, dc);
            0
        }
        WM_PRINT if single => {
            let dc = wparam as HDC;
            let saved = SaveDC(dc);
            let mut window: RECT = std::mem::zeroed();
            let mut client: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut window);
            GetClientRect(hwnd, &mut client);
            let mut origin = POINT { x: 0, y: 0 };
            let mut viewport: POINT = std::mem::zeroed();
            ClientToScreen(hwnd, &mut origin);
            GetViewportOrgEx(dc, &mut viewport);
            let rect = RECT {
                left: 0,
                top: 0,
                right: window.right - window.left,
                bottom: window.bottom - window.top,
            };
            FillRect(dc, &rect, s.white);
            SetViewportOrgEx(
                dc,
                viewport.x + origin.x - window.left,
                viewport.y + origin.y - window.top,
                null_mut(),
            );
            IntersectClipRect(dc, 0, 0, client.right, client.bottom);
            DefSubclassProc(hwnd, WM_PRINTCLIENT, wparam, lparam);
            RestoreDC(dc, saved);
            0
        }
        WM_SIZE | WM_SETFONT => {
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            format_edit(hwnd, s.dpi, message == WM_SETFONT);
            result
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(edit_procedure), 5);
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}
pub fn run(base: PathBuf, root: PathBuf, smoke: bool) -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let identity = window_identity(&root)?;
        let _instance_guard = if smoke {
            None
        } else {
            match single_instance(&identity)? {
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
        let dpi = GetDpiForSystem() as i32;
        let _embedded_font = crate::font::EmbeddedFont::register()?;
        let fonts = make_fonts(dpi);
        let draft: Value = serde_json::from_slice(
            &std::fs::read(base.join("assets/settings.default.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let locale = Locale::new(&root, smoke)?;
        let mut textures = crate::texture::Textures::new()?;
        textures.set_dpi(dpi);
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
            page: 0,
            category: 0,
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
            fonts,
            mushroom_brushes: make_mushroom_brushes(),
            pulse_frame: 0,
            pulse_running: false,
            pulse_enabled: animations != 0,
            pulse_ticks: Cell::new(0),
            hero_draws: Cell::new(0),
            nav_hover: None,
            smoke_min_height: None,
            textures,
            paper,
            white,
            smoke,
            hidden: false,
            minimized: false,
            scroll: 0,
            scroll_dragging: false,
            profile_wheel: 0,
            scrollbar_drag: None,
            exit_pending: false,
            smoke_stopped_clean: false,
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
            hero_presentations: Cell::new(0),
            edit_ink_cache: RefCell::new(HashMap::new()),
        });
        let pointer = &mut *s as *mut State;
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            &mut work as *mut _ as *mut std::ffi::c_void,
            0,
        );
        let hwnd = CreateWindowExW(
            if smoke {
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            } else {
                0
            },
            class.as_ptr(),
            wide("Mukhomor").as_ptr(),
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
            LBS_OWNERDRAWFIXED as u32 | LBS_HASSTRINGS as u32 | LBS_NOTIFY as u32 | WS_TABSTOP,
        );
        SendMessageW(s.controls[&1001], LB_SETITEMHEIGHT, 0, s.d(44) as isize);
        SetWindowSubclass(s.controls[&1001], Some(profile_procedure), 1, 0);
        for id in [1010, 1024, 2110, 3010] {
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
            (2103, "Добавить .exe"),
            (2104, "Сохранить"),
            (3011, "Обновить списки и IP"),
            (3012, "Сохранить настройки DNS"),
            (4003, "Диагностика"),
            (4004, "Открыть журналы"),
            (4005, "Свернуть в трей"),
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
            (3001, "Российские сервисы"),
            (3002, "Steam / Valve"),
            (3003, "Ozon"),
            (3004, "Обновлять IP доменов"),
            (3005, "Учитывать замеченные поддомены"),
            (4001, "Автоподключение"),
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
        s.populate();
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
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
            let window_chrome = smoke_window_chrome(&mut s)?;
            let home_design = smoke_home_design(&mut s)?;
            let hero_edits = smoke_hero_and_edit_padding(&mut s)?;
            let imports = smoke_import_flow(&mut s)?;
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
            s.page = 2;
            s.scroll = 520;
            s.layout();
            SetViewportOrgEx(s.buffer_dc, 0, 0, null_mut());
            SetPixel(s.buffer_dc, s.d(10), s.d(500), 0x00ff00ff);
            InvalidateRect(hwnd, null(), 1);
            UpdateWindow(hwnd);
            let scrolled_buffer_clear = GetPixel(s.buffer_dc, s.d(10), s.d(500))
                == crate::texture::sample(BG, s.dpi, s.d(10), s.d(500));
            screenshot(hwnd, &s.root.join("native-small-page.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4012, 0);
            UpdateWindow(hwnd);
            screenshot(hwnd, &s.root.join("native-en-small-dns.bmp"))?;
            SendMessageW(hwnd, WM_COMMAND, 4011, 0);
            s.page = 0;
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
            s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(116));
            SendMessageW(hwnd, WM_TIMER, 1, 0);
            let bounded_wait = !s.busy && s.pending_id.is_none();
            let (caption_blocks, texture_cache) = smoke_pixel_surfaces(&mut s);
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
                single_instance(&identity)?.ok_or("Smoke instance unexpectedly existed")?;
            let peer_identity = identity.clone();
            let duplicate_reused =
                thread::spawn(move || single_instance(&peer_identity).map(|guard| guard.is_none()))
                    .join()
                    .map_err(|_| "Smoke activation thread failed")??;
            s.hidden = true;
            ShowWindow(hwnd, SW_HIDE);
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
            // A clean stopped controller is irrelevant to Connect/Disconnect
            // receipts: the shortcut applies exclusively to the pending Exit.
            s.smoke_stopped_clean = true;
            s.pending_action = "Connect".into();
            s.exit_pending = false;
            let reply = Box::into_raw(Box::new(Answer {
                response: Err("Synthetic Connect transport failure".into()),
                action: "Connect".into(),
                generation: s.generation,
                request_id: s.pending_id.clone(),
            }));
            SendMessageW(hwnd, REPLY, 0, reply as isize);
            let cleanup_proof_only_settles_exit = IsWindow(hwnd) != 0
                && s.busy
                && s.pending_id.is_some()
                && s.phase() == "connecting";
            let mut confirmed_exit_cases = [false; 2];
            for (case, watchdog) in [false, true].into_iter().enumerate() {
                let fixture = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    wide("Smoke lost Exit").as_ptr(),
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
                if fixture.is_null() {
                    return Err("Could not create lost Exit smoke window".into());
                }
                SetWindowLongPtrW(fixture, GWLP_USERDATA, pointer as isize);
                s.request(json!({"action":"Exit"}));
                if watchdog {
                    s.pending_since = Some(Instant::now() - std::time::Duration::from_secs(116));
                    SendMessageW(fixture, WM_TIMER, 1, 0);
                } else {
                    let reply = Box::into_raw(Box::new(Answer {
                        response: Err("Synthetic Status transport failure".into()),
                        action: "Status".into(),
                        generation: s.generation,
                        request_id: None,
                    }));
                    SendMessageW(fixture, REPLY, 0, reply as isize);
                }
                confirmed_exit_cases[case] = IsWindow(fixture) == 0
                    && !s.busy
                    && s.pending_id.is_none()
                    && !s.exit_pending
                    && s.model["running"] == false;
                s.hwnd = hwnd;
            }
            s.smoke_stopped_clean = false;
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
                    "connecting_pulse_repaints_only_mushroom_and_stops_offpage_hidden_minimized",
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
                    "cleanup_proof_never_settles_connect",
                    cleanup_proof_only_settles_exit,
                ),
                (
                    "lost_exit_status_error_with_cleanup_proof_destroys_window",
                    confirmed_exit_cases[0],
                ),
                (
                    "lost_exit_watchdog_with_cleanup_proof_destroys_window",
                    confirmed_exit_cases[1],
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
