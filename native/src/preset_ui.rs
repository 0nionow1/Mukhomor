// One owner and one editor for all six rule sections.
impl State {
    fn selected_preset_index(&self) -> Option<usize> {
        crate::presets::index(&self.draft, self.selected_preset.as_deref()?)
    }
    fn manager_visible(&self) -> bool {
        self.page == 1 && self.preset_manager_open
    }
    unsafe fn collect_presets(&mut self) {
        if self.page != 1 {
            return;
        }
        let selected = self.selected_preset_index();
        if !self.manager_visible() {
            let lines = self.lines(2102);
            if let Some(i) = selected {
                self.draft["presets"][i]["rules"][KEYS[self.category]] = lines;
            } else {
                self.draft["direct"][KEYS[self.category]] = lines;
            }
            return;
        }
        if let Some(i) = selected {
            self.draft["presets"][i]["name"] = json!(self.text(3103));
            self.draft["presets"][i]["description"] = json!(self.text(3104));
        }
        let owner = if let Some(i) = selected {
            &self.draft["presets"][i]
        } else {
            &self.draft["direct"]
        };
        // Keep unknown/custom sources when editing the four bundled sources.
        let mut sources: Vec<Value> = owner["domain_rule_sets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|v| !crate::presets::DOMAIN_SOURCES.iter().any(|(_, s)| v == s))
            .collect();
        sources.extend(
            crate::presets::DOMAIN_SOURCES
                .into_iter()
                .filter(|(id, _)| self.checked(*id))
                .map(|(_, name)| json!(name)),
        );
        if let Some(i) = selected {
            self.draft["presets"][i]["domain_rule_sets"] = json!(sources);
        } else {
            self.draft["direct"]["domain_rule_sets"] = json!(sources);
        }
    }
    unsafe fn populate_preset_choices(&self) {
        let combo = self.controls[&3101];
        SendMessageW(combo, CB_RESETCONTENT, 0, 0);
        SendMessageW(
            combo,
            CB_ADDSTRING,
            0,
            wide(&self.locale.text("Мои правила")).as_ptr() as isize,
        );
        SendMessageW(combo, CB_SETCURSEL, 0, 0);
        if let Some(presets) = self.draft["presets"].as_array() {
            for (i, p) in presets.iter().enumerate() {
                let count = p["rules"][KEYS[self.category]]
                    .as_array()
                    .map_or(0, Vec::len);
                let label = format!(
                    "{} · {}{}{}",
                    p["name"].as_str().unwrap_or(""),
                    count,
                    if matches!(self.category, 2 | 3)
                        && p["domain_rule_sets"]
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                    {
                        format!(" + {}", self.locale.text("источники"))
                    } else {
                        String::new()
                    },
                    if p["enabled"] != true {
                        format!(" · {}", self.locale.text("выключен"))
                    } else {
                        String::new()
                    }
                );
                SendMessageW(combo, CB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
                if self.selected_preset.as_deref() == p["id"].as_str() {
                    SendMessageW(combo, CB_SETCURSEL, i + 1, 0);
                }
            }
        }
    }
    unsafe fn populate_presets(&mut self) {
        if self.selected_preset_index().is_none() {
            self.selected_preset = None;
        }
        self.populate_preset_choices();
        self.populate_preset_library();
        let owner = if let Some(i) = self.selected_preset_index() {
            let p = self.draft["presets"][i].clone();
            self.set(2102, &joined(&p["rules"][KEYS[self.category]]));
            self.set(3103, p["name"].as_str().unwrap_or(""));
            self.set(3104, p["description"].as_str().unwrap_or(""));
            self.check(3105, p["enabled"] == true);
            self.check(
                2302,
                !crate::presets::contains(&p, "disabled_sections", KEYS[self.category]),
            );
            p
        } else {
            self.set(2102, &joined(&self.draft["direct"][KEYS[self.category]]));
            self.draft["direct"].clone()
        };
        for (id, name) in crate::presets::DOMAIN_SOURCES {
            self.check(
                id,
                crate::presets::contains(&owner, "domain_rule_sets", name),
            );
        }
        self.layout_key.set(None);
    }
    unsafe fn preset_buttons(&self) {
        let editable = !self.busy && self.model["busy"] != true;
        let selected = self.selected_preset_index();
        for id in [
            3101, 3100, 2311, 2102, 2105, 2106, 2107, 3103, 3104, 3108, 3109, 3110, 2302, 2109,
            4009, 3112, 3301, 3310, 3312, 3313, 3314, 3315, 3106,
        ] {
            self.enable(
                id,
                editable
                    && (!matches!(id, 3103 | 3104 | 2302 | 3106 | 3314 | 3315)
                        || selected.is_some()),
            );
        }
        let capacity = self.draft["presets"]
            .as_array()
            .is_some_and(|p| p.len() < 128);
        for id in [3102, 3312, 3313] {
            self.enable(id, editable && capacity);
        }
        self.enable(
            3107,
            editable
                && selected.is_some_and(|i| self.draft["presets"][i]["builtin_id"].is_string()),
        );
    }
    unsafe fn add_preset(&mut self, template: Option<&str>) -> bool {
        let id = format!(
            "p-{:x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let created = if let Some(builtin) = template {
            crate::presets::add_template(&mut self.draft, &id, builtin)
        } else {
            crate::presets::create(&mut self.draft, &id, &self.locale.text("Новый набор"))
        };
        if created {
            self.selected_preset = Some(id);
            self.preset_manager_open = false;
            self.scroll = 0;
        }
        created
    }
    unsafe fn populate_preset_library(&self) {
        let list = self.controls[&3301];
        let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0).max(0);
        let visible = GetWindowLongPtrW(list, GWL_STYLE) as u32 & WS_VISIBLE != 0;
        if visible {
            SendMessageW(list, WM_SETREDRAW, 0, 0);
        }
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        if let Some(presets) = self.draft["presets"].as_array() {
            for (i, p) in presets.iter().enumerate() {
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    0,
                    wide(p["name"].as_str().unwrap_or("")).as_ptr() as isize,
                );
                if self.selected_preset.as_deref() == p["id"].as_str() {
                    SendMessageW(list, LB_SETCURSEL, i, 0);
                }
            }
        }
        let mut bounds = RECT::default();
        GetClientRect(list, &mut bounds);
        let rows = (bounds.bottom / self.d(44).max(1)).max(1);
        let top = if let Some(i) = self.selected_preset_index() {
            top.min(i as isize).max(i as isize - rows as isize + 1)
        } else {
            top
        };
        SendMessageW(list, LB_SETTOPINDEX, top.max(0) as usize, 0);
        if visible {
            SendMessageW(list, WM_SETREDRAW, 1, 0);
        }
        InvalidateRect(list, null(), 0);
        InvalidateRect(self.controls[&3307], null(), 0);
    }
    unsafe fn preset_error(&mut self, error: &str) {
        self.notice = error.to_owned();
        if !self.smoke {
            MessageBoxW(
                self.hwnd,
                wide(&self.locale.error(error)).as_ptr(),
                wide(&self.locale.text("Наборы")).as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
    unsafe fn preset_command(&mut self, id: i32, event: u32) -> bool {
        if id == 3101 && event != CBN_SELCHANGE {
            return false;
        }
        if id == 3301 && ![LBN_SELCHANGE, LBN_DBLCLK].contains(&event) {
            return false;
        }
        if !matches!(id, 3100..=3110 | 3112 | 2311 | 2302 | 2106 | 2107 | 2109 | 3301 | 3312..=3315 | 4009)
            || matches!(id, 3103 | 3104)
        {
            return false;
        }
        if self.busy || self.model["busy"] == true {
            return true;
        }
        let choice = SendMessageW(self.controls[&3101], CB_GETCURSEL, 0, 0);
        let library_choice = SendMessageW(self.controls[&3301], LB_GETCURSEL, 0, 0);
        if !self.collect() {
            return true;
        }
        match id {
            3101 => { self.selected_preset = if choice > 0 { self.draft["presets"][(choice-1) as usize]["id"].as_str().map(str::to_owned) } else { None }; }
            3301 => {
                self.selected_preset = if library_choice>=0 { self.draft["presets"][library_choice as usize]["id"].as_str().map(str::to_owned) } else { None };
                if event==LBN_DBLCLK { self.page=1; self.preset_manager_open=false; self.scroll=0; }
            }
            3312 => {
                let name=self.text(3310).trim().to_owned();
                if name.is_empty() { SetFocus(self.controls[&3310]); return true; }
                if self.add_preset(None) {
                    let i=self.selected_preset_index().unwrap(); self.draft["presets"][i]["name"]=json!(name);
                    self.set(3310, "");
                }
            }
            3102 => {
                let defaults=crate::presets::defaults(); let templates=defaults["presets"].as_array().unwrap();
                let items:Vec<_>=templates.iter().enumerate().map(|(n,p)| (n+1,p["name"].as_str().unwrap_or(""))).collect();
                let (menu,_labels)=pixel_menu(self,&items); let mut r=RECT::default(); GetWindowRect(self.controls[&3102],&mut r);
                let action=TrackPopupMenu(menu,TPM_RETURNCMD|TPM_RIGHTBUTTON,r.left,r.bottom,0,self.hwnd,null()); DestroyMenu(menu);
                if action>0 { if let Some(p)=templates.get(action as usize-1) { self.add_preset(p["builtin_id"].as_str()); } }
            }
            3313 => {
                if let Some(path)=preset_file(self,false) {
                    use std::io::Read;
                    let result=(|| -> Result<(),String> {
                        let mut bytes=Vec::new();
                        std::fs::File::open(&path).map_err(|e|e.to_string())?.take((crate::presets::MAX_SETTINGS_BYTES+1) as u64).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
                        let id=format!("p-{:x}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
                        crate::presets::import(&mut self.draft,&id,&bytes)?;
                        self.selected_preset=Some(id); Ok(())
                    })();
                    if let Err(e)=result { self.preset_error(&e); }
                }
            }
            3314 => {
                if let Some(owner)=self.selected_preset.as_deref() {
                    match crate::presets::export(&self.draft,owner) {
                        Ok(bytes) => { if let Some(path)=preset_file(self,true) { if let Err(e)=std::fs::write(path,bytes) { self.preset_error(&e.to_string()); } } }
                        Err(e)=>self.preset_error(&e),
                    }
                }
            }
            3315 => { self.page=1; self.preset_manager_open=false; self.scroll=0; }
            3100 => { self.preset_manager_open=false; self.scroll=0; }
            2311 => { self.preset_manager_open=true; self.scroll=0; }
            2302 => { if let Some(i)=self.selected_preset_index() {
                let disabled=!crate::presets::contains(&self.draft["presets"][i],"disabled_sections",KEYS[self.category]);
                crate::presets::membership(&mut self.draft["presets"][i],"disabled_sections",KEYS[self.category],disabled);
            } }
            3105 => { if self.page==2 { if let Some(i)=self.selected_preset_index() { let enabled=self.draft["presets"][i]["enabled"]!=true; self.draft["presets"][i]["enabled"]=json!(enabled); } } }
            3108..=3110 | 3112 => { self.check(id,!self.checked(id)); self.collect_presets(); }
            3106 | 3107 => { if let Some(owner)=self.selected_preset.clone() {
                let message=if id==3106 { "Удалить набор и все его правила? Изменение применится после сохранения." } else { "Восстановить исходные правила и источники этого набора? Имя и настройки сохранятся." };
                if self.smoke || MessageBoxW(self.hwnd,wide(&self.locale.text(message)).as_ptr(),wide(&self.locale.text("Наборы")).as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2)==IDYES {
                    if id==3106 { crate::presets::remove(&mut self.draft,&owner); self.selected_preset=None; }
                    else { crate::presets::restore(&mut self.draft,&owner); }
                }
            } }
            2106 => {
                let value=self.text(2105).trim().to_owned();
                if !value.is_empty() {
                    let previous=self.text(2102);
                    let editor=self.controls[&2102];
                    let keep=if previous.trim().is_empty() { "" } else { previous.trim_end() };
                    let insertion=if keep.is_empty() { value } else { format!("\r\n{value}") };
                    // Append as a native edit operation: keep Undo available
                    // and reveal the new rule instead of jumping to the top.
                    SendMessageW(editor,EM_SETSEL,keep.encode_utf16().count(),GetWindowTextLengthW(editor) as isize);
                    SendMessageW(editor,EM_REPLACESEL,1,wide(&insertion).as_ptr() as isize);
                    SendMessageW(editor,EM_SCROLLCARET,0,0);
                    self.set(2105,""); self.collect_presets();
                }
                SetFocus(self.controls[&2105]);
            }
            2107 => {
                let editor=self.controls[&2102]; let mut start=0u32; let mut end=0u32;
                SendMessageW(editor,EM_GETSEL,&mut start as *mut _ as usize,&mut end as *mut _ as isize);
                // Win32's visual lines depend on wrapping and CRLF. Delete
                // logical rule lines, including text pasted with Unix LF.
                let value:Vec<_>=self.text(2102).encode_utf16().collect();
                let first=(start as usize).min(value.len());
                let last=(if end>start {end-1}else{end} as usize).min(value.len());
                let newline=|c:&u16| *c==b'\n' as u16 || *c==b'\r' as u16;
                let mut from=value[..first].iter().rposition(newline).map_or(0,|n|n+1);
                let to=if let Some(n)=value[last..].iter().position(newline) {
                    let n=last+n;
                    n+if value[n]==b'\r' as u16 && value.get(n+1)==Some(&(b'\n' as u16)) {2}else{1}
                }else{
                    if from>0 && value.get(from-1)==Some(&(b'\n' as u16)) {from-=1;}
                    if from>0 && value.get(from-1)==Some(&(b'\r' as u16)) {from-=1;}
                    value.len()
                };
                SendMessageW(editor,EM_SETSEL,from,to as isize);
                SendMessageW(editor,EM_REPLACESEL,1,wide("").as_ptr() as isize);
                self.collect_presets(); SetFocus(editor);
            }
            2109 => { self.request(json!({"action":"ValidateSettings","settings":self.draft})); return true; }
            4009 => {
                if !self.smoke && MessageBoxW(self.hwnd,wide(&self.locale.text("Сбросить правила и пресеты? Будут восстановлены базовые пресеты. Серверы и настройки подключения сохранятся.")).as_ptr(),wide(&self.locale.text("Наборы")).as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2)==IDYES {
                    let defaults=crate::presets::defaults(); self.draft["direct"]=defaults["direct"].clone(); self.draft["presets"]=defaults["presets"].clone(); self.draft["dns_update"]["seed_hosts"]=json!([]); self.selected_preset=None;
                    self.request(json!({"action":"ApplySettings","settings":self.draft}));
                }
            }
            _=>{}
        }
        self.populate_settings();
        self.update_buttons();
        self.layout_key.set(None);
        self.layout();
        InvalidateRect(self.hwnd, null(), 0);
        true
    }
    unsafe fn paint_preset_library(&self, dc: HDC, left: i32, area: i32) {
        text(
            self,
            dc,
            "Название нового набора",
            left,
            188,
            area,
            20,
            3,
            MUTED,
        );
        text(self, dc, "МОИ НАБОРЫ", left, 314, area, 20, 3, MUTED);
        let footer = self.rules_footer_y();
        text(
            self,
            dc,
            "Галочка включает набор. Правила редактируются отдельно.",
            left,
            footer - 32,
            area,
            24,
            3,
            MUTED,
        );
        text(
            self,
            dc,
            "Сохранить применит изменения во всех наборах.",
            left,
            footer + 46,
            area,
            20,
            3,
            MUTED,
        );
    }
    unsafe fn paint_rules(&self, dc: HDC, left: i32, area: i32) {
        if self.manager_visible() {
            if self.selected_preset_index().is_some() {
                text(self, dc, "Название набора", left, 244, area, 20, 3, MUTED);
                text(self, dc, "Описание", left, 314, area, 20, 3, MUTED);
            } else {
                text(
                    self,
                    dc,
                    "Мои правила хранятся отдельно от наборов.",
                    left,
                    252,
                    area,
                    44,
                    3,
                    MUTED,
                );
            }
            text(
                self,
                dc,
                "ОБНОВЛЯЕМЫЕ СПИСКИ ДОМЕНОВ",
                left,
                416,
                area,
                20,
                3,
                MUTED,
            );
        } else {
            let owner = self
                .selected_preset_index()
                .map(|i| &self.draft["presets"][i])
                .unwrap_or(&self.draft["direct"]);
            let hint = if owner["enabled"] == false {
                "Набор выключен. Включите его на странице «Наборы»."
            } else if crate::presets::contains(owner, "disabled_sections", KEYS[self.category]) {
                "Этот список выключен. Включите его в параметрах."
            } else {
                match self.category {
                    0 => "Имя программы: cs2.exe. По одному правилу на строку.",
                    1 => "Полный путь к .exe. По одному правилу на строку.",
                    2 => "Точный домен: api.example.com. Без https:// и пути.",
                    3 => "example.com — с поддоменами. ru — вся зона .ru.",
                    4 => "IP или подсеть: 1.2.3.4, 1.2.3.0/24, 2001:db8::/32.",
                    _ => "Домены, которые пойдут через VPN вопреки готовым спискам.",
                }
            };
            text(self, dc, hint, left, 334, area, 34, 3, MUTED);
            let footer = self.rules_footer_y();
            text(
                self,
                dc,
                "Новое правило",
                left,
                footer - 86,
                area,
                20,
                3,
                MUTED,
            );
        }
        let footer = self.rules_footer_y();
        text(
            self,
            dc,
            "Сохранить применит изменения во всех наборах.",
            left,
            footer + 46,
            area,
            20,
            3,
            MUTED,
        );
    }
    fn rule_issues_text(&self, issues: &Value) -> String {
        let Some(issues) = issues.as_array() else {
            return String::new();
        };
        let mut lines = vec![];
        for issue in issues {
            let owner = issue["preset_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .and_then(|id| crate::presets::index(&self.draft, id))
                .and_then(|i| self.draft["presets"][i]["name"].as_str())
                .unwrap_or_else(|| {
                    if self.locale.language() == Language::English {
                        "Standalone rules"
                    } else {
                        "Обычные правила"
                    }
                });
            let section = issue["section"].as_str().unwrap_or("");
            let section = KEYS
                .iter()
                .position(|k| *k == section)
                .map(|n| self.locale.text(CATEGORIES[n]).into_owned())
                .unwrap_or_else(|| section.to_owned());
            lines.push(format!(
                "{} / {} / {}: {}\n{}",
                owner,
                section,
                issue["line"],
                issue["value"].as_str().unwrap_or(""),
                self.locale.error(issue["message"].as_str().unwrap_or(""))
            ));
        }
        if lines.is_empty() {
            lines.push(
                self.locale
                    .text("Проблем в правилах не обнаружено.")
                    .into_owned(),
            );
        } else {
            lines.insert(0,self.locale.text("Введённые данные не изменены. При сохранении VPN использует корректные правила. Предупреждения не меняют переключатели.").into_owned());
        }
        let total = self.model["rule_issues_total"]
            .as_u64()
            .unwrap_or(issues.len() as u64);
        if total > 0 {
            lines.insert(
                1,
                format!(
                    "{}: {} / {}",
                    self.locale.text("Показано предупреждений"),
                    issues.len(),
                    total
                ),
            );
        }
        if total > issues.len() as u64 {
            lines.insert(2, self.locale.text("Исправь показанные записи и запусти проверку снова, чтобы увидеть следующие замечания.").into_owned());
        }
        lines
            .join("\n\n")
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
    }
    unsafe fn show_rule_issues(&self, issues: &Value) {
        if self.smoke {
            return;
        }
        let previous_focus = GetFocus();
        let dialog = create_rule_issues_dialog(self, &self.rule_issues_text(issues));
        if dialog.is_null() {
            MessageBoxW(
                self.hwnd,
                wide(&self.locale.text("Не удалось открыть результаты проверки.")).as_ptr(),
                wide(&self.locale.text("Проверка правил")).as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
            return;
        }
        EnableWindow(self.hwnd, 0);
        SetFocus(GetDlgItem(dialog, 1));
        let mut msg: MSG = std::mem::zeroed();
        while IsWindow(dialog) != 0 {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result <= 0 {
                if result == 0 {
                    PostQuitMessage(msg.wParam as i32);
                }
                break;
            }
            if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE as usize {
                DestroyWindow(dialog);
            } else if msg.message == WM_KEYDOWN
                && msg.wParam == b'A' as usize
                && GetKeyState(VK_CONTROL as i32) < 0
            {
                SendMessageW(GetDlgItem(dialog, 1), EM_SETSEL, 0, -1);
            } else if IsDialogMessageW(dialog, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if IsWindow(dialog) != 0 {
            DestroyWindow(dialog);
        }
        if IsWindow(self.hwnd) != 0 {
            EnableWindow(self.hwnd, 1);
            SetActiveWindow(self.hwnd);
            if IsWindow(previous_focus) != 0 {
                SetFocus(previous_focus);
            }
        }
    }
}

unsafe fn rule_scroll_info(editor: HWND) -> (i32, i32, i32) {
    let mut r = RECT::default();
    GetClientRect(editor, &mut r);
    let dc = GetDC(editor);
    let old = SelectObject(dc, SendMessageW(editor, WM_GETFONT, 0, 0) as HGDIOBJ);
    let mut metrics = std::mem::zeroed();
    GetTextMetricsW(dc, &mut metrics);
    SelectObject(dc, old);
    ReleaseDC(editor, dc);
    (
        SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) as i32,
        (SendMessageW(editor, EM_GETLINECOUNT, 0, 0) as i32).max(1),
        (r.bottom / metrics.tmHeight.max(1)).max(1),
    )
}
unsafe extern "system" fn rule_scrollbar_proc(
    hwnd: HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _id: usize,
    _data: usize,
) -> isize {
    if msg == WM_LBUTTONDOWN || (msg == WM_MOUSEMOVE && GetCapture() == hwnd) {
        if msg == WM_LBUTTONDOWN {
            SetCapture(hwnd);
        }
        let editor = GetDlgItem(GetParent(hwnd), 1);
        let (top, count, rows) = rule_scroll_info(editor);
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r);
        let y = ((lp as u32 >> 16) & 0xffff) as i16 as i32;
        let next = (y * (count - rows).max(0) / r.bottom.max(1)).clamp(0, (count - rows).max(0));
        SendMessageW(editor, EM_LINESCROLL, 0, (next - top) as isize);
        InvalidateRect(hwnd, null(), 0);
        return 0;
    }
    if msg == WM_LBUTTONUP && GetCapture() == hwnd {
        ReleaseCapture();
        return 0;
    }
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(rule_scrollbar_proc), 7);
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}
unsafe fn rule_dialog_font(hwnd: HWND) {
    let old = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HFONT;
    let font = CreateFontW(
        -16 * GetDpiForWindow(hwnd).max(96) as i32 / 96,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        NONANTIALIASED_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide(crate::font::family()).as_ptr(),
    );
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, font as isize);
    for id in [1, 2] {
        SendMessageW(GetDlgItem(hwnd, id), WM_SETFONT, font as usize, 1);
    }
    if !old.is_null() {
        DeleteObject(old);
    }
}

unsafe fn rule_dialog_layout(hwnd: HWND) {
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let unit = GetDpiForWindow(hwnd).max(96) as i32;
    let margin = 12 * unit / 96;
    let button_height = 32 * unit / 96;
    let button_width = 112 * unit / 96;
    MoveWindow(
        GetDlgItem(hwnd, 1),
        margin,
        margin + 44 * unit / 96,
        (client.right - 2 * margin - 12 * unit / 96).max(1),
        (client.bottom - button_height - 3 * margin - 44 * unit / 96).max(1),
        1,
    );
    MoveWindow(
        GetDlgItem(hwnd, 2),
        (client.right - margin - button_width).max(margin),
        (client.bottom - margin - button_height).max(margin),
        button_width,
        button_height,
        1,
    );
    MoveWindow(
        GetDlgItem(hwnd, 3),
        client.right - margin - 10 * unit / 96,
        margin + 44 * unit / 96,
        10 * unit / 96,
        (client.bottom - button_height - 3 * margin - 44 * unit / 96).max(1),
        1,
    );
    let pointer = GetWindowLongPtrW(GetWindow(hwnd, GW_OWNER), GWLP_USERDATA) as *mut State;
    if !pointer.is_null() {
        let rounded = (*pointer).locale.design.effects && (*pointer).locale.design.rounding;
        crate::design::window_shape(hwnd, client.right, client.bottom, unit, rounded);
        for id in [1, 2] {
            let child = GetDlgItem(hwnd, id);
            let mut r = RECT::default();
            GetWindowRect(child, &mut r);
            crate::design::window_shape(child, r.right - r.left, r.bottom - r.top, unit, rounded);
        }
    }
}

unsafe extern "system" fn rule_dialog_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(GetWindow(hwnd, GW_OWNER), GWLP_USERDATA) as *mut State;
    match msg {
        WM_NCCALCSIZE => 0,
        WM_NCHITTEST => {
            let hit = DefWindowProcW(hwnd, msg, wp, lp);
            if hit != HTCLIENT as isize {
                return hit;
            }
            let mut p = POINT {
                x: (lp as u32 & 0xffff) as i16 as i32,
                y: ((lp as u32 >> 16) & 0xffff) as i16 as i32,
            };
            ScreenToClient(hwnd, &mut p);
            if p.y < 44 * GetDpiForWindow(hwnd) as i32 / 96 {
                HTCAPTION as isize
            } else {
                hit
            }
        }
        WM_ERASEBKGND => 1,
        WM_PAINT if !pointer.is_null() => {
            let s = &*pointer;
            let mut paint = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r);
            FillRect(dc, &r, s.textures.brush(BG));
            text(
                s,
                dc,
                "Проверка правил",
                14,
                18,
                r.right * 96 / s.dpi - 28,
                24,
                0,
                INK,
            );
            EndPaint(hwnd, &paint);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT if !pointer.is_null() => {
            let s = &*pointer;
            SetTextColor(wp as HDC, s.textures.color(INK));
            SetBkColor(wp as HDC, s.textures.color(PANEL_ALT));
            (*pointer).white as isize
        }
        WM_DRAWITEM if !pointer.is_null() => {
            let s = &*pointer;
            let item = &*(lp as *const DRAWITEMSTRUCT);
            let r = item.rcItem;
            if item.CtlID == 3 {
                FillRect(item.hDC, &r, s.textures.brush(BG));
                let editor = GetDlgItem(hwnd, 1);
                let (top, count, rows) = rule_scroll_info(editor);
                if count > rows {
                    let height = r.bottom - r.top;
                    let thumb = (height * rows / count).max(s.d(12));
                    let y = top * (height - thumb) / (count - rows).max(1);
                    pixel_panel(
                        s,
                        item.hDC,
                        RECT {
                            left: r.left + s.d(3),
                            top: y,
                            right: r.right - s.d(2),
                            bottom: y + thumb,
                        },
                        INDIGO,
                        INDIGO,
                    );
                }
            } else {
                FillRect(item.hDC, &r, s.textures.brush(BG));
                pixel_panel(
                    s,
                    item.hDC,
                    r,
                    PANEL_ALT,
                    if item.itemState & ODS_FOCUS != 0 {
                        TEAL
                    } else {
                        PANEL_ALT
                    },
                );
                centered(
                    s,
                    item.hDC,
                    "Закрыть",
                    0,
                    0,
                    r.right * 96 / s.dpi,
                    r.bottom * 96 / s.dpi,
                    3,
                    INK,
                );
            }
            1
        }
        WM_COMMAND if wp as u32 & 0xffff == 1 => {
            InvalidateRect(GetDlgItem(hwnd, 3), null(), 0);
            0
        }
        WM_SIZE => {
            rule_dialog_layout(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            let bounds = &mut *(lp as *mut MINMAXINFO);
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            bounds.ptMinTrackSize.x = 320 * dpi / 96;
            bounds.ptMinTrackSize.y = 240 * dpi / 96;
            0
        }
        WM_DPICHANGED => {
            rule_dialog_font(hwnd);
            let r = &*(lp as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            0
        }
        WM_COMMAND if wp as u32 & 0xffff == 2 => {
            DestroyWindow(hwnd);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            let font = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HFONT;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !font.is_null() {
                DeleteObject(font);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn create_rule_issues_dialog(s: &State, text: &str) -> HWND {
    let class = wide("MukhomorRuleDiagnostics");
    let instance = GetModuleHandleW(null());
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(rule_dialog_proc);
    wc.hInstance = instance;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.hIcon = product_icon();
    wc.hbrBackground = null_mut();
    wc.lpszClassName = class.as_ptr();
    RegisterClassW(&wc);
    let dpi = GetDpiForWindow(s.hwnd).max(96) as i32;
    let mut monitor: MONITORINFO = std::mem::zeroed();
    monitor.cbSize = std::mem::size_of_val(&monitor) as u32;
    GetMonitorInfoW(
        MonitorFromWindow(s.hwnd, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    );
    let mut owner: RECT = std::mem::zeroed();
    GetWindowRect(s.hwnd, &mut owner);
    let width = (640 * dpi / 96)
        .min(monitor.rcWork.right - monitor.rcWork.left - 32)
        .max(200);
    let height = (580 * dpi / 96)
        .min(monitor.rcWork.bottom - monitor.rcWork.top - 32)
        .max(200);
    let x = ((owner.left + owner.right - width) / 2).clamp(
        monitor.rcWork.left,
        (monitor.rcWork.right - width).max(monitor.rcWork.left),
    );
    let y = ((owner.top + owner.bottom - height) / 2).clamp(
        monitor.rcWork.top,
        (monitor.rcWork.bottom - height).max(monitor.rcWork.top),
    );
    let dialog = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class.as_ptr(),
        wide(&s.locale.text("Проверка правил")).as_ptr(),
        WS_POPUP | WS_SYSMENU | WS_THICKFRAME,
        x,
        y,
        width,
        height,
        s.hwnd,
        null_mut(),
        instance,
        null(),
    );
    if dialog.is_null() {
        return dialog;
    }
    configure_custom_chrome(dialog);
    let editor = CreateWindowExW(
        0,
        wide("EDIT").as_ptr(),
        null(),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_READONLY as u32,
        0,
        0,
        1,
        1,
        dialog,
        1 as HMENU,
        instance,
        null(),
    );
    let close = CreateWindowExW(
        0,
        wide("BUTTON").as_ptr(),
        wide(&s.locale.text("Закрыть")).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
        0,
        0,
        1,
        1,
        dialog,
        2 as HMENU,
        instance,
        null(),
    );
    if editor.is_null() || close.is_null() {
        DestroyWindow(dialog);
        return null_mut();
    }
    let bar = CreateWindowExW(
        0,
        wide("STATIC").as_ptr(),
        null(),
        WS_CHILD | WS_VISIBLE | PIXEL_SCROLLBAR_STYLE,
        0,
        0,
        10,
        10,
        dialog,
        3 as HMENU,
        instance,
        null(),
    );
    SetWindowSubclass(bar, Some(rule_scrollbar_proc), 7, 0);
    SendMessageW(editor, EM_SETLIMITTEXT, 1024 * 1024, 0);
    SetWindowTextW(editor, wide(text).as_ptr());
    rule_dialog_font(dialog);
    rule_dialog_layout(dialog);
    ShowWindow(dialog, SW_SHOW);
    dialog
}

unsafe fn preset_file(s: &State, save: bool) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    if save {
        let name = wide("preset.mukhomor.json");
        buffer[..name.len()].copy_from_slice(&name);
    }
    let filter = wide("Mukhomor preset (*.json)\0*.json\0\0");
    let extension = wide("json");
    let title = wide(&s.locale.text(if save {
        "Экспорт набора"
    } else {
        "Импорт набора"
    }));
    let mut dialog: OPENFILENAMEW = std::mem::zeroed();
    dialog.lStructSize = std::mem::size_of_val(&dialog) as u32;
    dialog.hwndOwner = s.hwnd;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.Flags = OFN_PATHMUSTEXIST
        | OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let accepted = if save {
        GetSaveFileNameW(&mut dialog)
    } else {
        GetOpenFileNameW(&mut dialog)
    };
    if accepted == 0 {
        return None;
    }
    let end = buffer.iter().position(|c| *c == 0)?;
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..end])))
}
unsafe extern "system" fn preset_list_procedure(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    let parent = GetParent(hwnd);
    if message == WM_LBUTTONDOWN {
        let p = GetWindowLongPtrW(parent, GWLP_USERDATA) as *mut State;
        let hit = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, lparam) as u32;
        let checkbox =
            !p.is_null() && ((lparam as u32 & 0xffff) as i32) < (*p).d(44) && hit >> 16 == 0;
        if checkbox {
            SetFocus(hwnd);
            SendMessageW(hwnd, LB_SETCURSEL, (hit & 0xffff) as usize, 0);
            SendMessageW(
                parent,
                WM_COMMAND,
                3301 | ((LBN_SELCHANGE as usize) << 16),
                hwnd as isize,
            );
            SendMessageW(parent, WM_COMMAND, 3105, 0);
            return 0;
        }
    }
    if message == WM_KEYDOWN && wparam == VK_SPACE as usize {
        SendMessageW(parent, WM_COMMAND, 3105, 0);
        return 0;
    }
    if message == WM_MOUSEWHEEL {
        SendMessageW(parent, LIST_SCROLL, wparam, 3301);
        return 0;
    }
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(preset_list_procedure), 6);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if message == WM_KEYDOWN {
        let bar = GetDlgItem(parent, 3307);
        InvalidateRect(bar, null(), 0);
    }
    result
}
unsafe fn smoke_preset_workflow(s: &mut State) -> Result<[bool; 6], String> {
    let original = (
        s.draft.clone(),
        s.model.clone(),
        s.page,
        s.category,
        s.selected_preset.clone(),
        s.preset_manager_open,
        s.scroll,
        s.dpi,
        s.dns_settings_open,
    );
    s.page = 1;
    s.category = 0;
    s.selected_preset = None;
    s.preset_manager_open = false;
    s.scroll = 0;
    s.populate_settings();
    s.set(2102, "personal.exe");
    s.collect();
    let personal = s.draft["direct"].clone();
    SendMessageW(s.hwnd, WM_COMMAND, 202, 0);
    s.set(3310, "QA preset");
    s.preset_command(3312, BN_CLICKED);
    let id = s.selected_preset.clone().ok_or("Preset create failed")?;
    let i = s.selected_preset_index().unwrap();
    let created =
        s.page == 2 && s.draft["presets"][i]["name"] == "QA preset" && s.text(3310).is_empty();
    s.preset_command(3315, BN_CLICKED);
    s.preset_command(2311, BN_CLICKED);
    s.set(3103, "QA renamed");
    s.set(3104, "Описание набора · UTF-8");
    s.preset_command(3112, BN_CLICKED);
    let sources = s.checked(3112) && s.draft["presets"][i]["domain_rule_sets"] == json!(["games"]);
    s.preset_command(3100, BN_CLICKED);
    s.draft["presets"][i]["visible_sections"] = json!([]);
    for (n, value) in [
        "custom.exe",
        "C:\\Apps\\custom.exe",
        "list.example",
        "suffix.example",
        "8.8.8.8/32",
        "vpn.example",
    ]
    .into_iter()
    .enumerate()
    {
        SendMessageW(s.hwnd, WM_COMMAND, 2200 + n, 0);
        s.set(2105, value);
        s.preset_command(2106, BN_CLICKED);
    }
    let all_six = created
        && sources
        && s.draft["presets"][i]["name"] == "QA renamed"
        && s.draft["presets"][i]["description"] == "Описание набора · UTF-8"
        && KEYS.into_iter().all(|key| {
            s.draft["presets"][i]["rules"][key]
                .as_array()
                .is_some_and(|a| a.len() == 1)
        })
        && s.draft["direct"] == personal;
    SendMessageW(s.hwnd, WM_COMMAND, 2202, 0);
    let before_add = s.text(2102);
    s.set(2105, "undo.example");
    s.preset_command(2106, BN_CLICKED);
    let editor = s.controls[&2102];
    let undo_add = SendMessageW(editor, EM_CANUNDO, 0, 0) != 0
        && SendMessageW(editor, EM_UNDO, 0, 0) != 0
        && s.text(2102) == before_add;
    s.collect_presets();
    s.set(2105, "remove.example");
    s.preset_command(2106, BN_CLICKED);
    let line = SendMessageW(s.controls[&2102], EM_LINEINDEX, 1, 0);
    SendMessageW(s.controls[&2102], EM_SETSEL, line as usize, line);
    s.preset_command(2107, BN_CLICKED);
    let removed = s.draft["presets"][i]["rules"]["domains"] == json!(["list.example"]);
    s.set(2102, "keep.example\nremove.example");
    let end = GetWindowTextLengthW(s.controls[&2102]);
    SendMessageW(s.controls[&2102], EM_SETSEL, end as usize, end as isize);
    s.preset_command(2107, BN_CLICKED);
    let removed = removed && s.text(2102) == "keep.example";
    s.set(2102, "rules.example\r\ninvalid,REJECT");
    SendMessageW(s.hwnd, WM_COMMAND, 2203, 0);
    SendMessageW(s.hwnd, WM_COMMAND, 2202, 0);
    SendMessageW(s.controls[&3101], CB_SETCURSEL, 0, 0);
    s.preset_command(3101, CBN_SELCHANGE);
    let separate = s.text(2102) == joined(&personal["domains"]);
    SendMessageW(s.controls[&3101], CB_SETCURSEL, i + 1, 0);
    s.preset_command(3101, CBN_SELCHANGE);
    let preserved =
        undo_add && removed && separate && s.text(2102) == "rules.example\r\ninvalid,REJECT";
    let mut layout = true;
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
        let mut editor = RECT::default();
        let mut entry = RECT::default();
        GetWindowRect(s.controls[&2102], &mut editor);
        GetWindowRect(s.controls[&2105], &mut entry);
        layout &= [2102, 2104, 2105, 2106, 2107, 2311, 3101, 2200, 2205]
            .into_iter()
            .all(|id| IsWindowVisible(s.controls[&id]) != 0)
            && editor.bottom <= entry.top
            && editor.bottom - editor.top >= s.d(100)
            && [3102, 3103, 3106, 3301, 2302, 2109]
                .into_iter()
                .all(|id| IsWindowVisible(s.controls[&id]) == 0);
        if dpi == 96 {
            InvalidateRect(s.hwnd, null(), 0);
            UpdateWindow(s.hwnd);
            screenshot(s.hwnd, &s.root.join("native-presets-rules.bmp"))?;
        }
        s.preset_command(2311, BN_CLICKED);
        layout &= [3103, 3104, 3100, 2104, 3108, 3112, 2302, 2109]
            .into_iter()
            .all(|id| IsWindowVisible(s.controls[&id]) != 0)
            && [2102, 2105, 3105, 3106, 3301]
                .into_iter()
                .all(|id| IsWindowVisible(s.controls[&id]) == 0);
        if dpi == 96 {
            InvalidateRect(s.hwnd, null(), 0);
            UpdateWindow(s.hwnd);
            screenshot(s.hwnd, &s.root.join("native-presets-manager.bmp"))?;
        }
        s.preset_command(3100, BN_CLICKED);
        SendMessageW(s.hwnd, WM_COMMAND, 202, 0);
        layout &= [3301, 3307, 3310, 3312, 3313, 3314, 3315, 3102, 3106, 2104]
            .into_iter()
            .all(|id| IsWindowVisible(s.controls[&id]) != 0)
            && [2102, 3101, 3103, 3104, 3105, 3004]
                .into_iter()
                .all(|id| IsWindowVisible(s.controls[&id]) == 0);
        if dpi == 96 {
            InvalidateRect(s.hwnd, null(), 0);
            UpdateWindow(s.hwnd);
            screenshot(s.hwnd, &s.root.join("native-presets-library.bmp"))?;
        }
        s.preset_command(3315, BN_CLICKED);
    }
    s.preset_command(2311, BN_CLICKED);
    s.preset_command(2302, BN_CLICKED);
    let disabled = crate::presets::contains(&s.draft["presets"][i], "disabled_sections", "domains")
        && s.draft["presets"][i]["enabled"] == true;
    s.preset_command(2302, BN_CLICKED);
    s.preset_command(3100, BN_CLICKED);
    SendMessageW(s.hwnd, WM_COMMAND, 202, 0);
    // Exercise the actual keyboard checkbox action on the dedicated list.
    SendMessageW(s.controls[&3301], WM_KEYDOWN, VK_SPACE as usize, 0);
    let disabled_owner = s.draft["presets"][i]["enabled"] == false;
    let bytes = crate::presets::export(&s.draft, &id)?;
    let path = s.root.join("shared-preset.mukhomor.json");
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    crate::presets::import(
        &mut s.draft,
        "qa-import",
        &std::fs::read(&path).map_err(|e| e.to_string())?,
    )?;
    let imported = crate::presets::index(&s.draft, "qa-import").unwrap();
    let sharing = s.draft["presets"][imported]["rules"] == s.draft["presets"][i]["rules"]
        && s.draft["presets"][imported]["description"] == "Описание набора · UTF-8"
        && s.draft["presets"][imported]["enabled"] == false;
    s.preset_command(3106, BN_CLICKED);
    let deleted = crate::presets::index(&s.draft, &id).is_none()
        && s.selected_preset.is_none()
        && s.draft["direct"] == personal;
    s.add_preset(Some("games"));
    s.populate_settings();
    s.preset_command(3315, BN_CLICKED);
    s.preset_command(2311, BN_CLICKED);
    let restored_index = s.selected_preset_index().unwrap();
    let rules = s.draft["presets"][restored_index]["rules"].clone();
    s.draft["presets"][restored_index]["rules"]["process_names"] = json!([]);
    s.preset_command(3107, BN_CLICKED);
    let restored =
        s.draft["presets"][restored_index]["rules"] == rules && s.draft["direct"] == personal;
    SendMessageW(s.hwnd, WM_COMMAND, 203, 0);
    SendMessageW(s.hwnd, WM_COMMAND, 4015, 0);
    layout &= s.dns_visible()
        && IsWindowVisible(s.controls[&3004]) != 0
        && [3301, 3101, 3102, 3103, 3104, 2102]
            .into_iter()
            .all(|id| IsWindowVisible(s.controls[&id]) == 0);
    s.set_dpi_fonts(original.7);
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
    InvalidateRect(s.hwnd, null(), 0);
    UpdateWindow(s.hwnd);
    screenshot(s.hwnd, &s.root.join("native-presets-dns.bmp"))?;
    SendMessageW(s.hwnd, WM_COMMAND, 4016, 0);
    let issues = json!((1..=200).map(|n| json!({"preset_id":"","section":"domains","line":n,"value":"x".repeat(1024),"message":"Invalid domain: long raw value"})).collect::<Vec<_>>());
    let before_dialog = s.draft.clone();
    s.model["rule_issues_total"] = json!(200);
    let dialog = create_rule_issues_dialog(s, &s.rule_issues_text(&issues));
    let dialog_ok = if !dialog.is_null() {
        let editor = GetDlgItem(dialog, 1);
        SendMessageW(editor, EM_LINESCROLL, 0, 190);
        let readable = GetWindowTextLengthW(editor) > 32767
            && SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) > 0
            && GetWindowLongW(editor, GWL_STYLE) as u32 & ES_READONLY as u32 != 0;
        screenshot(dialog, &s.root.join("native-rule-diagnostics.bmp"))?;
        SendMessageW(dialog, WM_COMMAND, 2, 0);
        readable && IsWindow(dialog) == 0 && s.draft == before_dialog
    } else {
        false
    };
    let result = [
        all_six,
        preserved,
        disabled && disabled_owner && deleted && restored,
        layout,
        dialog_ok,
        sharing,
    ];
    (
        s.draft,
        s.model,
        s.page,
        s.category,
        s.selected_preset,
        s.preset_manager_open,
        s.scroll,
        _,
        s.dns_settings_open,
    ) = original;
    s.populate();
    Ok(result)
}
