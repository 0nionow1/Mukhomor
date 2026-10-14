// Included in ui.rs: native controls share the same paint, scrolling and DPI model.
const THEME_CONTROL_START: i32 = 4040;
const THEME_CONTROL_END: i32 = THEME_CONTROL_START + Theme::ALL.len() as i32;
const _: () = assert!(
    THEME_CONTROL_END <= 4060,
    "Theme controls overlap interface sizes"
);
// The first three gallery rows occupy the original layout; grow the page with the catalog.
const APPEARANCE_EXTRA_HEIGHT: i32 = (Theme::ALL.len().div_ceil(2) as i32 - 3) * 116;

fn is_theme_control(id: i32) -> bool {
    (THEME_CONTROL_START..THEME_CONTROL_END).contains(&id)
}

impl State {
    unsafe fn create_appearance_controls(&mut self) {
        for (id, name) in [
            (4030, "Оформление"),
            (4031, "К настройкам"),
            (4032, "Сбросить оформление"),
        ] {
            self.ctrl(id, "BUTTON", name, BS_OWNERDRAW as u32 | WS_TABSTOP);
        }
        for (i, theme) in Theme::ALL.into_iter().enumerate() {
            self.ctrl(
                THEME_CONTROL_START + i as i32,
                "BUTTON",
                theme.name(),
                BS_OWNERDRAW as u32 | WS_TABSTOP,
            );
        }
        for (i, size) in InterfaceSize::ALL.into_iter().enumerate() {
            self.ctrl(
                4060 + i as i32,
                "BUTTON",
                size.name(),
                BS_OWNERDRAW as u32 | WS_TABSTOP,
            );
        }
    }

    unsafe fn paint_appearance(&self, dc: HDC, left: i32, area: i32) {
        text(self, dc, "РАЗМЕР ИНТЕРФЕЙСА", left, 250, area, 22, 3, MUTED);
        text(
            self,
            dc,
            "Меняются текст, иконки, кнопки и отступы.\nРазмер учитывает масштаб Windows.",
            left,
            342,
            area,
            38,
            3,
            MUTED,
        );
        text(self, dc, "ПРЕСЕТ ДИЗАЙНА", left, 386, area, 22, 3, MUTED);
        text(
            self,
            dc,
            "Неон — основной: розовый + неон-teal.\nЛюбой пресет работает с любым размером.",
            left,
            772 + APPEARANCE_EXTRA_HEIGHT,
            area,
            38,
            3,
            MUTED,
        );
        text(
            self,
            dc,
            "ЭФФЕКТЫ",
            left,
            834 + APPEARANCE_EXTRA_HEIGHT,
            area,
            22,
            3,
            MUTED,
        );
        rounded(
            self,
            dc,
            left,
            868 + APPEARANCE_EXTRA_HEIGHT,
            area,
            242,
            PANEL,
            LINE,
        );
        text(
            self,
            dc,
            "Выбор сохраняется автоматически.",
            left,
            1170 + APPEARANCE_EXTRA_HEIGHT,
            area,
            20,
            3,
            MUTED,
        );
    }

    /// Prepare fallible resources before committing the atomic preference write.
    /// Changing appearance never re-populates editors or asks the backend to save.
    unsafe fn apply_appearance(&mut self, next: crate::design::Design) -> Result<(), String> {
        let previous = self.locale.design;
        let icons = if previous.theme != next.theme {
            Some(ProductIcons::new(next.theme, self.monitor_dpi)?)
        } else {
            None
        };
        let replacement = if self.textures.theme != next.theme {
            Some(crate::texture::Textures::with_theme(next.theme)?)
        } else {
            None
        };
        self.locale.set_design(next)?;
        if let Some(icons) = icons {
            icons.apply(self.hwnd);
            let old = std::mem::replace(&mut self.product_icons, icons);
            if !self.smoke {
                tray_update(self);
            }
            drop(old);
        }
        if let Some(mut textures) = replacement {
            textures.set_dpi(self.dpi);
            textures.set_design(next);
            // Keep the counters monotonic across theme changes for diagnostics.
            textures.background_frames = self.textures.background_frames;
            self.textures = textures;
            self.paper = self.textures.brush(BG);
            self.white = self.textures.brush(PANEL_ALT);
            for brushes in std::mem::replace(
                &mut self.mushroom_brushes,
                make_mushroom_brushes(next.theme),
            ) {
                for brush in brushes {
                    DeleteObject(brush);
                }
            }
        }
        if previous.interface_size != next.interface_size {
            let old_dpi = self.dpi;
            let mut bounds = RECT::default();
            GetWindowRect(self.hwnd, &mut bounds);
            self.set_dpi_fonts(self.monitor_dpi);
            self.regions.clear();
            let width = self.d(HOME_WIDTH);
            let height =
                ((bounds.bottom - bounds.top) * self.dpi / old_dpi).max(self.minimum_height());
            let work = self.work_area_for_rect(&bounds);
            let height = if self.smoke {
                height
            } else {
                height.min((work.bottom - work.top - self.d(16)).max(240))
            };
            let x = if self.smoke {
                bounds.left
            } else {
                bounds
                    .left
                    .clamp(work.left, (work.right - width).max(work.left))
            };
            let y = if self.smoke {
                bounds.top
            } else {
                bounds
                    .top
                    .clamp(work.top, (work.bottom - height).max(work.top))
            };
            SetWindowPos(
                self.hwnd,
                null_mut(),
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.refresh_design();
        // Surface brushes also belong to the new palette when the size is unchanged.
        self.paper = self.textures.brush(BG);
        self.white = self.textures.brush(PANEL_ALT);
        Ok(())
    }

    unsafe fn appearance_command(&mut self, id: i32, event: u32) -> bool {
        if event != BN_CLICKED {
            return false;
        }
        if id == 4030 || id == 4031 {
            if !self.collect() {
                return true;
            }
            self.save_background_speed();
            self.appearance_open = id == 4030;
            self.dns_settings_open = false;
            self.scroll = 0;
            self.layout_key.set(None);
            self.refresh_design();
            SetFocus(self.controls[&if self.appearance_open { 4031 } else { 4030 }]);
            return true;
        }
        if id != 4032 && !is_theme_control(id) && !(4060..=4062).contains(&id) {
            return false;
        }
        self.save_background_speed();
        let mut next = self.locale.design;
        if id == 4032 {
            next = Default::default();
        } else if is_theme_control(id) {
            next.theme = Theme::ALL[(id - THEME_CONTROL_START) as usize];
        } else {
            next.interface_size = InterfaceSize::ALL[(id - 4060) as usize];
        }
        if let Err(error) = self.apply_appearance(next) {
            self.notice = error;
            InvalidateRect(self.hwnd, null(), 0);
        }
        true
    }
}

unsafe fn render_theme_card(s: &State, draw: &DRAWITEMSTRUCT) -> isize {
    let theme = Theme::ALL[(draw.CtlID as i32 - THEME_CONTROL_START) as usize];
    let selected = s.locale.design.theme == theme;
    let focus = draw.itemState & (ODS_FOCUS | ODS_SELECTED) != 0;
    let r = draw.rcItem;
    let dc = draw.hDC;
    if let Some(origin) = s.control_bounds.borrow().get(&(draw.CtlID as i32)) {
        SetBrushOrgEx(dc, -origin.0, -origin.1, null_mut());
    }
    let w = (r.right - r.left) * 96 / s.dpi;
    let h = (r.bottom - r.top) * 96 / s.dpi;
    surface_fill(s, dc, &r, BG);
    rounded(
        s,
        dc,
        0,
        0,
        w,
        h,
        if selected { PANEL_ALT } else { PANEL },
        if selected || focus { TEAL } else { LINE },
    );
    text(
        s,
        dc,
        theme.name(),
        14,
        12,
        w - 48,
        27,
        0,
        if selected { TEAL } else { INK },
    );
    text(s, dc, theme.description(), 14, 43, w - 28, 20, 3, MUTED);
    if selected {
        icon(s, dc, 4, w - 30, 16, 16, TEAL);
    }
    // Actual preset swatches, deliberately independent of the current palette.
    let colors = theme.colors();
    let swatch = (w - 28) / 2;
    for (i, index) in [6, 5].into_iter().enumerate() {
        let brush = CreateSolidBrush(colors[index]);
        let rect = RECT {
            left: s.d(14 + i as i32 * swatch),
            top: s.d(76),
            right: s.d(14 + (i as i32 + 1) * swatch - 3),
            bottom: s.d(92),
        };
        FillRect(dc, &rect, brush);
        DeleteObject(brush);
    }
    1
}

unsafe fn smoke_appearance(s: &mut State) -> Result<[bool; 6], String> {
    let original = (
        s.locale.design,
        s.monitor_dpi,
        s.page,
        s.scroll,
        s.appearance_open,
        s.dns_settings_open,
        s.locale.language(),
    );
    let draft = s.draft.clone();
    let model = s.model.clone();
    let editor = s.text(2102);
    SendMessageW(s.controls[&2102], EM_SETSEL, 1, 3);
    let mut checks = [true; 6];
    checks[0] &=
        s.dpi == original.0.interface_size.dpi(original.1) && s.textures.theme == original.0.theme;
    let mut observations = Vec::new();
    s.set_dpi_fonts(96);
    s.change_language(Language::Russian);
    s.page = 3;
    s.dns_settings_open = false;
    s.appearance_open = false;
    s.scroll = 0;
    s.layout_key.set(None);
    s.layout();
    checks[0] &= IsWindowVisible(s.controls[&4030]) != 0;
    // Use the real button command path, including persistence, rather than
    // installing palettes directly into the renderer for these checks.
    SendMessageW(s.hwnd, WM_COMMAND, 4030, 0);
    checks[0] &= s.appearance_open && IsWindowVisible(s.controls[&4062]) != 0;
    for (ti, theme) in Theme::ALL.into_iter().enumerate() {
        SendMessageW(s.hwnd, WM_COMMAND, THEME_CONTROL_START as usize + ti, 0);
        for (si, size) in InterfaceSize::ALL.into_iter().enumerate() {
            SendMessageW(s.hwnd, WM_COMMAND, 4060 + si, 0);
            let mut font: LOGFONTW = std::mem::zeroed();
            GetObjectW(
                s.fonts[0],
                std::mem::size_of_val(&font) as i32,
                (&mut font as *mut LOGFONTW).cast(),
            );
            checks[1] &= s.dpi == size.dpi(96) && font.lfHeight == -24 * s.dpi / 96;
            checks[2] &=
                Locale::new(&s.root, true)?.design == s.locale.design && s.textures.theme == theme;
            checks[2] &= SendMessageW(s.hwnd, WM_GETICON, ICON_SMALL as usize, 0)
                == s.product_icons.small.0 as isize
                && SendMessageW(s.hwnd, WM_GETICON, ICON_BIG as usize, 0)
                    == s.product_icons.large.0 as isize;
            for (glyph, expected) in [
                (b'R', theme.color(INDIGO)),
                (b'W', theme.color(TEAL)),
                (b'S', theme.color(STEM)),
            ] {
                let mut brush = LOGBRUSH::default();
                GetObjectW(
                    s.mushroom_brushes[1][mushroom::palette_index(glyph).unwrap()],
                    std::mem::size_of_val(&brush) as i32,
                    (&mut brush as *mut LOGBRUSH).cast(),
                );
                checks[2] &= brush.lbColor == expected;
            }
            for page in 0..4 {
                s.page = page;
                s.scroll = 0;
                s.layout_key.set(None);
                s.refresh_design();
                RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
                let (width, height, _) = s.dimensions();
                checks[3] &= (559..=561).contains(&width) && height >= 738;
                if page == 0 {
                    let bounds = s.control_bounds.borrow();
                    let list = bounds[&1001];
                    let action = bounds[&1002];
                    let nav = bounds[&200];
                    checks[3] &= list.1 + list.3 + s.d(18) <= action.1
                        && action.1 + action.3 + s.d(20) <= nav.1;
                    let layout = s.home_layout();
                    let art = hero_mushroom_bounds(s, 240, layout.hero_art_height);
                    checks[3] &= art.left >= 0
                        && art.right <= s.d(240)
                        && art.bottom + s.d(4) <= s.d(layout.hero_h - 40);
                    if size == InterfaceSize::Medium {
                        screenshot(
                            s.hwnd,
                            &s.root.join(format!("appearance-{}.bmp", theme.id())),
                        )?;
                    }
                    if theme == Theme::default() {
                        screenshot(s.hwnd, &s.root.join(format!("size-{}.bmp", size.id())))?;
                    }
                }
            }
            observations.push(json!({"theme":theme.id(),"size":size.id(),"dpi":s.dpi,"font_height":font.lfHeight}));
        }
    }
    s.apply_appearance(crate::design::Design::default())?;
    for monitor_dpi in [96, 120, 144, 192] {
        s.set_dpi_fonts(monitor_dpi);
        for (si, size) in InterfaceSize::ALL.into_iter().enumerate() {
            SendMessageW(s.hwnd, WM_COMMAND, 4060 + si, 0);
            SetWindowPos(
                s.hwnd,
                null_mut(),
                -5000,
                0,
                s.d(HOME_WIDTH),
                s.d(HOME_HEIGHT),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            s.page = 3;
            s.appearance_open = true;
            s.scroll = 0;
            s.layout_key.set(None);
            s.refresh_design();
            let bounds = s.control_bounds.borrow();
            checks[1] &= s.dpi == size.dpi(monitor_dpi);
            for id in [4031, 4060, 4061, 4062, 4040, 4041] {
                let b = bounds[&id];
                checks[3] &= IsWindowVisible(s.controls[&id]) != 0
                    && b.0 >= 0
                    && b.0 + b.2 <= s.d(HOME_WIDTH);
            }
            drop(bounds);
            if monitor_dpi == 96 && size == InterfaceSize::Large {
                screenshot(s.hwnd, &s.root.join("appearance-settings-large.bmp"))?;
                s.scroll = 300;
                s.layout();
                screenshot(s.hwnd, &s.root.join("appearance-settings-cards.bmp"))?;
            }
            // Every row must be reachable, including the last new preset, at all scales.
            for row in 0..Theme::ALL.len().div_ceil(2) {
                s.scroll = 300 + row as i32 * 116;
                s.layout();
                let bounds = s.control_bounds.borrow();
                for column in 0..2 {
                    let index = row * 2 + column;
                    if index >= Theme::ALL.len() {
                        continue;
                    }
                    let id = THEME_CONTROL_START + index as i32;
                    let b = bounds[&id];
                    checks[3] &= IsWindowVisible(s.controls[&id]) != 0
                        && b.0 >= 0
                        && b.0 + b.2 <= s.d(HOME_WIDTH)
                        && b.1 >= s.d(88)
                        && b.1 + b.3 <= s.d(s.dimensions().1 - NAV_HEIGHT - NAV_GAP);
                }
                drop(bounds);
                if monitor_dpi == 96 && size == InterfaceSize::Medium && row % 4 == 0 {
                    screenshot(
                        s.hwnd,
                        &s.root.join(format!("appearance-gallery-{row}.bmp")),
                    )?;
                    s.change_language(Language::English);
                    screenshot(
                        s.hwnd,
                        &s.root.join(format!("appearance-gallery-en-{row}.bmp")),
                    )?;
                    s.change_language(Language::Russian);
                }
            }
        }
    }
    s.set_dpi_fonts(96);
    let minimum = s.smoke_min_height;
    for (i, size) in InterfaceSize::ALL.into_iter().enumerate() {
        SendMessageW(s.hwnd, WM_COMMAND, 4060 + i, 0);
        s.smoke_min_height = Some(s.d(460));
        SetWindowPos(
            s.hwnd,
            null_mut(),
            -5000,
            0,
            s.d(HOME_WIDTH),
            s.d(480),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        s.page = 0;
        s.scroll = 0;
        s.refresh_design();
        let bounds = s.control_bounds.borrow();
        let actions = bounds[&1002];
        let nav = bounds[&200];
        checks[3] &=
            IsWindowVisible(s.controls[&1002]) != 0 && actions.1 + actions.3 + s.d(20) <= nav.1;
        let layout = s.home_layout();
        let art = hero_mushroom_bounds(s, 240, layout.hero_art_height);
        checks[3] &= art.bottom + s.d(4) <= s.d(layout.hero_h - 40);
        drop(bounds);
        screenshot(s.hwnd, &s.root.join(format!("short-{}.bmp", size.id())))?;
        s.page = 3;
        s.appearance_open = true;
        s.scroll = (s.dimensions().2 - s.dimensions().1 + NAV_HEIGHT + NAV_GAP).max(0);
        s.layout();
        checks[3] &= IsWindowVisible(s.controls[&4032]) != 0;
    }
    s.smoke_min_height = minimum;
    // Repeated palette changes must release native brushes, DIBs and cached fonts.
    s.apply_appearance(crate::design::Design::default())?;
    s.page = 0;
    s.scroll = 0;
    s.refresh_design();
    RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
    GdiFlush();
    let resources = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
    let user_resources = GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS);
    for _ in 0..3 {
        for theme in Theme::ALL {
            s.apply_appearance(crate::design::Design {
                theme,
                ..Default::default()
            })?;
        }
    }
    s.apply_appearance(crate::design::Design::default())?;
    RedrawWindow(s.hwnd, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
    GdiFlush();
    checks[4] &= GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) <= resources;
    checks[4] &= GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) <= user_resources;
    let (mut start, mut end) = (0u32, 0u32);
    SendMessageW(
        s.controls[&2102],
        EM_GETSEL,
        &mut start as *mut _ as usize,
        &mut end as *mut _ as isize,
    );
    checks[5] &= s.draft == draft
        && s.model == model
        && s.text(2102) == editor
        && start == 1.min(editor.encode_utf16().count() as u32)
        && end == 3.min(editor.encode_utf16().count() as u32);
    std::fs::write(s.root.join("appearance-observations.json"),serde_json::to_vec_pretty(&json!({"checks":checks,"startup_theme":original.0.theme.id(),"startup_size":original.0.interface_size.id(),"variants":observations,"gdi_before":resources,"gdi_after":GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS)})).unwrap()).map_err(|e|e.to_string())?;
    s.apply_appearance(original.0)?;
    s.set_dpi_fonts(original.1);
    s.page = original.2;
    s.scroll = original.3;
    s.appearance_open = original.4;
    s.dns_settings_open = original.5;
    s.change_language(original.6);
    SetWindowPos(
        s.hwnd,
        null_mut(),
        -5000,
        0,
        s.d(HOME_WIDTH),
        s.d(HOME_HEIGHT),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    s.refresh_design();
    Ok(checks)
}
