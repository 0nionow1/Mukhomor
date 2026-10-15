//! Shared platform adapter; controller IO never runs on the Slint event-loop thread.
use crate::{
    appearance::{InterfaceSize, Theme},
    i18n::{Language, Locale},
};
use mukhomor_ui::{
    AppWindow, I18n, Palette, Preset, Profile, Tray,
    slint::{self, ComponentHandle, Model, VecModel},
    state::{Receipt, State, Startup},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

struct Reply {
    generation: u64,
    action: String,
    result: Result<Value, String>,
}
struct Host {
    state: State,
    locale: Rc<RefCell<Locale>>,
    window: slint::Weak<AppWindow>,
    tray: Option<slint::Weak<Tray>>,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    profiles: Rc<VecModel<Profile>>,
    profiles_value: Value,
    selected_value: String,
    nonce: String,
    status_pending: bool,
    last_status: Instant,
    created: Instant,
    transport_notice: Cell<bool>,
    notice_deadline: Cell<Option<Instant>>,
    operation_inflight: Option<u64>,
    startup_pending: bool,
    startup: Startup,
    autostart_executable: Option<PathBuf>,
    rule_owner: String,
    preset_serial: u64,
    validation_draft: Value,
    exit_after_save: bool,
    transport: std::sync::Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
}

impl Host {
    fn refresh_startup(&mut self) {
        self.startup.observe(&self.state, self.created.elapsed() >= Duration::from_secs(30));
        if let Some(ui) = self.window.upgrade() {
            ui.set_boot_active(!self.startup.complete);
            let locale = self.locale.borrow();
            ui.set_boot_status(locale.text(if self.startup.error.is_some() {
                "Не удалось подготовить приложение"
            } else if self.state.loaded {
                "Подготавливаем VPN…"
            } else {
                "Загружаем профили и правила…"
            }).as_ref().into());
            ui.set_boot_error(self.startup.error.as_ref().map(|e| locale.error(e).into_owned()).unwrap_or_default().into());
            ui.set_boot_retry_enabled(!self.status_pending);
        }
    }
    fn launch(&mut self, request: Value) {
        let action = request["action"].as_str().unwrap_or("Status").to_owned();
        if action == "Status" {
            if self.status_pending {
                return;
            }
            self.status_pending = true;
            self.last_status = Instant::now();
        } else {
            self.operation_inflight = Some(self.state.generation);
        }
        let sender = self.sender.clone();
        let generation = self.state.generation;
        let transport = self.transport.clone();
        let window = self.window.clone();
        std::thread::spawn(move || {
            let result = transport(request);
            let _ = sender.send(Reply {
                generation,
                action,
                result,
            });
            let _ = window.upgrade_in_event_loop(|ui| ui.invoke_worker_wakeup());
        });
    }
    fn request(&mut self, request: Value) {
        let action = request["action"].as_str().unwrap_or("");
        if action == "Exit" && self.state.dirty() {
            if let Some(ui) = self.window.upgrade() {
                ui.set_dialog(9);
                let _ = ui.show();
            }
            return;
        }
        if matches!(action, "Connect" | "Disconnect" | "Exit" | "Select") {
            self.startup_pending = false;
        }
        if matches!(action, "ApplySettings" | "ValidateSettings")
            && !crate::presets::fits_transport(&request["settings"])
        {
            self.notice(crate::presets::SIZE_ERROR, true);
            return;
        }
        if matches!(
            self.state.pending.as_ref().map(|p| p.action.as_str()),
            Some("Disconnect" | "Exit")
        ) {
            return;
        }
        if action == "ValidateSettings" && !self.state.busy() {
            self.validation_draft = self.state.draft.clone();
        }
        if let Some(request) = self.state.begin(request, &self.nonce) {
            self.notice("", false);
            self.launch(request);
            self.refresh(false);
        }
    }
    fn receive(&mut self) {
        while let Ok(reply) = self.receiver.try_recv() {
            #[cfg(target_os = "android")]
            if reply.action.starts_with("file:") {
                match reply.result {
                    Err(error) => self.notice(&error, true),
                    Ok(data) if data["cancelled"] == true => {},
                    Ok(data) => {
                        if reply.action == "file:import-file" {
                            if let Some(ui) = self.window.upgrade() {
                                ui.set_import_text(data["text"].as_str().unwrap_or("").into());
                                ui.set_import_source("Android document".into());
                            }
                        } else if reply.action == "file:preset-import" {
                            self.preset_serial += 1;
                            let id = format!("{}-{}", self.nonce, self.preset_serial);
                            let mut draft = self.state.draft.clone();
                            match crate::presets::import(&mut draft, &id, data["text"].as_str().unwrap_or("").as_bytes()) {
                                Ok(()) => { self.state.edit(draft); self.refresh_draft(); },
                                Err(error) => self.notice(&error, true),
                            }
                        }
                    }
                }
                continue;
            }
            if reply.action == "Status" {
                self.status_pending = false;
            } else if self.operation_inflight == Some(reply.generation) {
                self.operation_inflight = None;
            }
            let draft_before = self.state.draft_revision();
            let result = self
                .state
                .receive(reply.generation, &reply.action, reply.result);
            if matches!(result, Receipt::Ignored | Receipt::Unchanged) {
                continue;
            }
            if let Receipt::Completed { action, ok, error } = result {
                if action == "ApplySettings" && self.exit_after_save {
                    self.exit_after_save = false;
                    if ok {
                        if let Some(ui) = self.window.upgrade() {
                            ui.set_dialog(0);
                        }
                        self.request(json!({"action":"Exit"}));
                    }
                }
                if action == "Exit" && ok {
                    let _ = slint::quit_event_loop();
                    return;
                }
                if !ok {
                    self.notice(
                        if error.is_empty() {
                            "Не удалось завершить операцию"
                        } else {
                            &error
                        },
                        true,
                    );
                    if let Some(ui) = self.window.upgrade() {
                        let _ = ui.show();
                    }
                } else {
                    if action == "ValidateSettings" && self.validation_draft == self.state.draft {
                        if let Some(ui) = self.window.upgrade() {
                            let locale = self.locale.borrow();
                            let issues = self.state.snapshot["rule_issues"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            let message = if issues.is_empty() {
                                locale.text("Ошибок не найдено").into_owned()
                            } else {
                                issues
                                    .iter()
                                    .map(|issue| {
                                        format!(
                                            "{} · {} · {}\n{}\n{}",
                                            issue["preset_id"].as_str().unwrap_or("direct"),
                                            issue["section"].as_str().unwrap_or(""),
                                            issue["line"],
                                            issue["value"].as_str().unwrap_or(""),
                                            locale.error(issue["message"].as_str().unwrap_or(""))
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                            };
                            ui.set_rule_issues(message.into());
                            ui.set_dialog(7);
                        }
                    }
                    #[cfg(not(windows))]
                    if action == "Diagnose" {
                        if let Some(ui) = self.window.upgrade() {
                            ui.set_rule_issues(serde_json::to_string_pretty(&self.state.snapshot["diagnostics"]).unwrap_or_default().into());
                            ui.set_dialog(7);
                        }
                    }
                    self.notice(
                        match action.as_str() {
                            "Import" => "Конфигурация добавлена",
                            "Rename" => "Название сохранено",
                            "Remove" => "Конфигурация удалена",
                            "ApplySettings" => "Изменения сохранены",
                            "Update" => "Списки правил обновлены",
                            "Diagnose" if cfg!(windows) => "Проверка завершена. Результаты в журналах.",
                            _ => "",
                        },
                        false,
                    );
                    if matches!(action.as_str(), "Import" | "Rename" | "Remove") {
                        if let Some(ui) = self.window.upgrade() {
                            ui.set_dialog(0);
                            if action == "Import" {
                                ui.set_import_text("".into());
                                ui.set_import_source("".into());
                                ui.set_dialog_name("".into());
                            }
                        }
                    }
                }
            }
            if let Some(error) = self.state.transport_error.clone() {
                if self.state.loaded || self.created.elapsed() >= Duration::from_secs(8) {
                    self.notice(&error, true);
                    self.transport_notice.set(true);
                }
            } else if self.transport_notice.get() {
                self.notice("", false);
            }
            self.refresh(draft_before != self.state.draft_revision());
            if self.startup_pending && self.state.available() {
                self.startup_pending = false;
                if self.state.snapshot["autoconnect"] == true
                    && self.state.phase() == "idle"
                    && !self.state.busy()
                    && self.state.snapshot["last_error"]
                        .as_str()
                        .unwrap_or("")
                        .is_empty()
                {
                    self.request(json!({"action":"Connect","startup":true}));
                }
            }
        }
    }
    fn notice(&self, text: &str, error: bool) {
        self.transport_notice.set(false);
        self.notice_deadline
            .set((!error && !text.is_empty()).then(|| Instant::now() + Duration::from_secs(4)));
        if let Some(ui) = self.window.upgrade() {
            let locale = self.locale.borrow();
            ui.set_notice(
                if error {
                    locale.error(text)
                } else {
                    locale.text(text)
                }
                .as_ref()
                .into(),
            );
            ui.set_notice_error(error);
        }
    }
    fn refresh(&mut self, draft_changed: bool) {
        self.refresh_startup();
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let locale = self.locale.borrow();
        let phase = self.state.phase();
        ui.set_phase(phase.into());
        let (status, button) = match phase {
            "connected" => ("Подключено", "Отключиться"),
            "connecting" => ("Подключаемся…", "Отменить"),
            "disconnecting" => ("Отключаем VPN…", "Отключаем…"),
            "recovery" => ("Требуется восстановление сети", "Вернуть сеть"),
            "uncertain" => ("Состояние соединения неизвестно", "Вернуть сеть"),
            "loading" => ("Загружаем состояние…", "Подключиться"),
            _ => ("Не подключено", "Подключиться"),
        };
        ui.set_status_text(locale.text(status).as_ref().into());
        ui.set_primary_text(locale.text(button).as_ref().into());
        ui.set_ready(self.state.available());
        ui.set_busy(self.state.busy());
        ui.set_loaded(self.state.loaded);
        ui.set_primary_enabled(
            phase != "disconnecting"
                && (self.state.can_disconnect()
                    || (self.state.available()
                        && !self.state.busy()
                        && !self.state.selected().is_empty())),
        );
        ui.set_protected(
            phase == "connected" && self.state.snapshot["traffic_guard"]["protected"] == true,
        );
        ui.set_dirty(self.state.dirty());
        ui.set_routing_editable(
            self.state.available()
                && !self.state.busy()
                && !self.state.can_disconnect()
                && self.state.snapshot["traffic_guard"]["configured"] != true,
        );
        ui.set_autoconnect(self.state.snapshot["autoconnect"] == true);
        let selected = self.state.selected();
        if self.profiles_value != self.state.snapshot["profiles"] || self.selected_value != selected
        {
            let profiles = self.state.snapshot["profiles"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let rows: Vec<_> = profiles
                .iter()
                .map(|p| Profile {
                    id: p["id"].as_str().unwrap_or("").into(),
                    name: p["name"].as_str().unwrap_or("").into(),
                    protocol: p["protocol"].as_str().unwrap_or("").into(),
                    selected: p["id"] == selected,
                })
                .collect();
            // Keep the same model during polls, renames and selections: no scroll reset.
            if self.profiles.row_count() == rows.len()
                && rows
                    .iter()
                    .enumerate()
                    .all(|(i, r)| self.profiles.row_data(i).is_some_and(|old| old.id == r.id))
            {
                for (i, row) in rows.into_iter().enumerate() {
                    if self.profiles.row_data(i).as_ref() != Some(&row) {
                        self.profiles.set_row_data(i, row);
                    }
                }
            } else {
                self.profiles.set_vec(rows);
            }
            ui.set_selected_name(
                profiles
                    .iter()
                    .find(|p| p["id"] == selected)
                    .and_then(|p| p["name"].as_str())
                    .unwrap_or("")
                    .into(),
            );
            ui.set_selected_id(selected.into());
            self.profiles_value = self.state.snapshot["profiles"].clone();
            self.selected_value = selected.into();
        }
        drop(locale);
        if draft_changed {
            self.refresh_draft();
        }
    }
    fn refresh_draft(&self) {
        if let Some(ui) = self.window.upgrade() {
            let section = crate::presets::SECTIONS
                .get(ui.get_rule_section() as usize)
                .unwrap_or(&"domains");
            let preset = crate::presets::index(&self.state.draft, &self.rule_owner);
            let owner = preset.map_or(&self.state.draft["direct"], |i| {
                &self.state.draft["presets"][i]
            });
            let rules = if preset.is_some() {
                &owner["rules"]
            } else {
                owner
            };
            let values = rules[section].as_array().cloned().unwrap_or_default();
            ui.set_rules_text(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            );
            ui.set_section_enabled(!crate::presets::contains(
                owner,
                "disabled_sections",
                section,
            ));
            ui.set_source_russia(crate::presets::contains(
                owner,
                "domain_rule_sets",
                "russia",
            ));
            ui.set_source_steam(crate::presets::contains(owner, "domain_rule_sets", "steam"));
            ui.set_source_ozon(crate::presets::contains(owner, "domain_rule_sets", "ozon"));
            ui.set_source_games(crate::presets::contains(owner, "domain_rule_sets", "games"));
            ui.set_full_routing(self.state.draft["routing"]["mode"] == "full");
            ui.set_allow_lan(self.state.draft["routing"]["allow_lan"] != false);
            ui.set_dirty(self.state.dirty());
            let presets = self.state.draft["presets"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut owners: Vec<slint::SharedString> =
                vec![self.locale.borrow().text("Личные правила").as_ref().into()];
            owners.extend(
                presets
                    .iter()
                    .map(|p| p["name"].as_str().unwrap_or("").into()),
            );
            ui.set_rule_owners(Rc::new(VecModel::from(owners)).into());
            ui.set_rule_owner(preset.map_or(0, |i| i as i32 + 1));
            if let Some(selected) = presets
                .iter()
                .find(|p| p["id"].as_str() == Some(ui.get_chosen_preset_id().as_str()))
            {
                ui.set_chosen_preset_name(selected["name"].as_str().unwrap_or("").into());
                ui.set_chosen_preset_description(
                    selected["description"].as_str().unwrap_or("").into(),
                );
            } else {
                ui.set_chosen_preset_id("".into());
                ui.set_chosen_preset_name("".into());
                ui.set_chosen_preset_description("".into());
            }
            ui.set_presets(
                Rc::new(VecModel::from(
                    presets
                        .iter()
                        .map(|p| Preset {
                            id: p["id"].as_str().unwrap_or("").into(),
                            name: p["name"].as_str().unwrap_or("").into(),
                            description: p["description"].as_str().unwrap_or("").into(),
                            enabled: p["enabled"] == true,
                            builtin: p["builtin_id"].is_string(),
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            let defaults = crate::presets::defaults();
            ui.set_template_names(
                Rc::new(VecModel::from(
                    defaults["presets"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            self.locale
                                .borrow()
                                .text(p["name"].as_str().unwrap_or(""))
                                .as_ref()
                                .into()
                        })
                        .collect::<Vec<slint::SharedString>>(),
                ))
                .into(),
            );
        }
    }
    fn edit_rules(&mut self, text: &str) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let key = crate::presets::SECTIONS
            .get(ui.get_rule_section() as usize)
            .unwrap_or(&"domains");
        let values = json!(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>()
        );
        let preset = crate::presets::index(&self.state.draft, &self.rule_owner);
        self.state.edit_rules(preset, key, values);
        ui.set_dirty(self.state.dirty());
        ui.set_rule_issues("".into());
    }
    fn edit_preset(&mut self, action: &str, id: &str, name: &str) -> Result<(), String> {
        let Some(ui) = self.window.upgrade() else {
            return Ok(());
        };
        let mut draft = self.state.draft.clone();
        let index = crate::presets::index(&draft, id);
        self.preset_serial += 1;
        let new_id = format!("{}-preset-{}", self.nonce, self.preset_serial);
        match action {
            "preset-create" | "preset-rename" => {
                if name.trim().is_empty()
                    || name.encode_utf16().count() > 120
                    || name.chars().any(char::is_control)
                {
                    return Err("Название должно содержать от 1 до 120 символов".into());
                }
                if ui.get_preset_description().encode_utf16().count() > 1000 {
                    return Err("Описание слишком длинное".into());
                }
                let i = if action == "preset-create" {
                    if !crate::presets::create(&mut draft, &new_id, name) {
                        return Err("Cannot add more than 128 presets".into());
                    }
                    crate::presets::index(&draft, &new_id).unwrap()
                } else {
                    index.ok_or("Choose a preset first")?
                };
                draft["presets"][i]["name"] = json!(name);
                draft["presets"][i]["description"] = json!(ui.get_preset_description().as_str());
                if action == "preset-create" {
                    ui.set_new_preset_name("".into());
                    ui.set_chosen_preset_id(new_id.clone().into());
                }
                ui.set_dialog(0);
            }
            "preset-toggle" => {
                if let Some(i) = index {
                    draft["presets"][i]["enabled"] = json!(draft["presets"][i]["enabled"] != true);
                }
            }
            "preset-edit" => {
                self.rule_owner = id.into();
                ui.set_page(1);
            }
            "preset-remove" => {
                crate::presets::remove(&mut draft, id);
                ui.set_dialog(0);
            }
            "preset-restore" => {
                if !crate::presets::restore(&mut draft, id) {
                    return Err("У этого набора нет исходного шаблона".into());
                }
                ui.set_dialog(0);
            }
            "preset-template" => {
                let defaults = crate::presets::defaults();
                let builtin = defaults["presets"]
                    .as_array()
                    .and_then(|p| p.get(ui.get_template_index() as usize))
                    .and_then(|p| p["builtin_id"].as_str())
                    .ok_or("Choose a template first")?;
                if !crate::presets::add_template(&mut draft, &new_id, builtin) {
                    return Err("Cannot add more than 128 presets".into());
                }
            }
            "preset-import" => {
                if let Some(path) = choose_file(&ui, false, true) {
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    std::fs::File::open(path)
                        .map_err(|e| e.to_string())?
                        .take((crate::presets::MAX_SETTINGS_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    crate::presets::import(&mut draft, &new_id, &bytes)?;
                }
            }
            "preset-export" => {
                let bytes = crate::presets::export(&draft, id)?;
                if let Some(path) = choose_file(&ui, true, true) {
                    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                    #[cfg(target_os = "android")]
                    { let error = crate::android::call("finishExport", &path.to_string_lossy())?; if !error.is_empty() { return Err(error); } }
                }
            }
            _ => {}
        }
        self.state.edit(draft);
        self.refresh_draft();
        Ok(())
    }
    fn command(&mut self, action: &str, id: &str, text: &str) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        if action == "boot-retry" {
            if !self.status_pending {
                self.created = Instant::now();
                self.state.transport_error = None;
                self.state.revision = None;
                self.launch(json!({"action":"Status"}));
                self.refresh_startup();
            }
            return;
        }
        if action == "boot-open" {
            self.startup.complete = true;
            if let Some(error) = self.startup.error.clone() { self.notice(&error, true); }
            self.refresh(false);
            return;
        }
        #[cfg(target_os = "android")]
        if matches!(action, "import-file" | "preset-import" | "preset-export") {
            let export = if action == "preset-export" {
                match crate::presets::export(&self.state.draft, id) {
                    Ok(bytes) => Some(bytes),
                    Err(error) => { self.notice(&error, true); return; }
                }
            } else { None };
            let kind = if export.is_some() { "save" } else if action == "preset-import" { "preset" } else { "profile" };
            let sender = self.sender.clone();
            let generation = self.state.generation;
            let window = self.window.clone();
            let action = format!("file:{action}");
            std::thread::spawn(move || {
                let result = (|| -> Result<Value, String> {
                    let selected = crate::android::call("chooseFile", kind)?;
                    if selected.is_empty() { return Ok(json!({"cancelled":true})); }
                    if let Some(error) = selected.strip_prefix("ERROR:") { return Err(error.into()); }
                    let path = PathBuf::from(selected);
                    let result = if let Some(bytes) = export {
                        std::fs::write(&path, bytes).map_err(|e|e.to_string()).and_then(|_| {
                            let error = crate::android::call("finishExport", &path.to_string_lossy())?;
                            if error.is_empty() { Ok(json!({})) } else { Err(error) }
                        })
                    } else {
                        use std::io::Read;
                        let mut text = String::new();
                        std::fs::File::open(&path).and_then(|f| f.take(2*1024*1024+1).read_to_string(&mut text))
                            .map(|_|json!({"text":text})).map_err(|e|e.to_string())
                    };
                    let _ = std::fs::remove_file(path);
                    result
                })();
                let _ = sender.send(Reply {generation, action, result});
                let _ = window.upgrade_in_event_loop(|ui|ui.invoke_worker_wakeup());
            });
            return;
        }
        match action {
            #[cfg(target_os = "android")]
            "vpn-settings" => { if let Err(error)=crate::android::call("vpnSettings", "") { self.notice(&error, true); } },
            "refresh" => {
                let request = self.state.status_request();
                self.launch(request);
            }
            "save-exit" => {
                if self.state.available()
                    && !self.state.busy()
                    && crate::presets::fits_transport(&self.state.draft)
                {
                    self.exit_after_save = true;
                    self.request(json!({"action":"ApplySettings","settings":self.state.draft}));
                } else if !crate::presets::fits_transport(&self.state.draft) {
                    self.notice(crate::presets::SIZE_ERROR, true);
                }
            }
            "discard-exit" => {
                self.exit_after_save = false;
                self.state.discard();
                ui.set_dialog(0);
                self.request(json!({"action":"Exit"}));
            }
            action if action.starts_with("preset-") => {
                if let Err(error) = self.edit_preset(action, id, text) {
                    self.notice(&error, true);
                }
            }
            "dismiss" => self.notice("", false),
            "primary" => self.request(
                json!({"action":if self.state.can_disconnect() {"Disconnect"} else {"Connect"}}),
            ),
            "Select" | "Rename" | "Remove" => {
                if self.state.available() && !self.state.busy() {
                    self.request(json!({"action":action,"id":id,"name":text}));
                }
            }
            "Import" => {
                let content = ui.get_import_text();
                if content.len() > 512 * 1024 {
                    self.notice("Конфигурация слишком большая (максимум 512 КБ)", true);
                } else if self.state.available() && !content.trim().is_empty() {
                    self.request(json!({"action":"Import","content":content.as_str(),"name":ui.get_dialog_name().as_str(),"source_name":ui.get_import_source().as_str(),"format":"auto"}));
                }
            }
            "import-file" => {
                self.notice("", false);
                if let Some(path) = choose_file(&ui, false, false) {
                    match read_import(&path) {
                        Ok(content) => {
                            ui.set_import_text(content.into());
                            ui.set_import_source(
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .as_ref()
                                    .into(),
                            );
                        }
                        Err(error) => self.notice(&error, true),
                    }
                }
            }
            "ApplySettings" | "ValidateSettings" => {
                self.request(json!({"action":action,"settings":self.state.draft}))
            }
            "rule-section" => self.refresh_draft(),
            "rule-owner" => {
                self.rule_owner = self.state.draft["presets"]
                    .as_array()
                    .and_then(|p| p.get(ui.get_rule_owner().wrapping_sub(1) as usize))
                    .and_then(|p| p["id"].as_str())
                    .unwrap_or("")
                    .into();
                self.refresh_draft();
            }
            "section-enabled" | "sources" => {
                let mut draft = self.state.draft.clone();
                let index = crate::presets::index(&draft, &self.rule_owner);
                let owner = if let Some(i) = index {
                    &mut draft["presets"][i]
                } else {
                    &mut draft["direct"]
                };
                if action == "section-enabled" && index.is_some() {
                    let key = crate::presets::SECTIONS
                        .get(ui.get_rule_section() as usize)
                        .unwrap_or(&"domains");
                    crate::presets::membership(
                        owner,
                        "disabled_sections",
                        key,
                        !ui.get_section_enabled(),
                    );
                } else {
                    for (source, enabled) in [
                        ("russia", ui.get_source_russia()),
                        ("steam", ui.get_source_steam()),
                        ("ozon", ui.get_source_ozon()),
                        ("games", ui.get_source_games()),
                    ] {
                        crate::presets::membership(owner, "domain_rule_sets", source, enabled);
                    }
                }
                self.state.edit(draft);
                ui.set_dirty(self.state.dirty());
            }
            "routing" => {
                let mut draft = self.state.draft.clone();
                draft["routing"]["mode"] = json!(if ui.get_full_routing() {
                    "full"
                } else {
                    "split"
                });
                draft["routing"]["allow_lan"] = json!(ui.get_allow_lan());
                self.state.edit(draft);
                ui.set_dirty(self.state.dirty());
            }
            "autoconnect" => {
                self.request(json!({"action":"Autoconnect","enabled":ui.get_autoconnect()}))
            }
            "autostart" => {
                if let Some(exe) = &self.autostart_executable {
                    if let Err(error) = crate::autostart::set(exe, ui.get_autostart()) {
                        ui.set_autostart(crate::autostart::enabled(exe).unwrap_or(false));
                        self.notice(&error, true);
                    }
                }
            }
            "appearance" => {
                let mut design = self.locale.borrow().design;
                design.theme = Theme::ALL
                    .get(ui.get_theme_index() as usize)
                    .copied()
                    .unwrap_or_default();
                design.interface_size = [
                    InterfaceSize::Small,
                    InterfaceSize::Medium,
                    InterfaceSize::Large,
                ]
                .get(ui.get_scale_index() as usize)
                .copied()
                .unwrap_or_default();
                design.rounding = ui.get_rounding();
                design.effects = ui.get_effects();
                design.animation = ui.get_animation();
                let result = self.locale.borrow_mut().set_design(design);
                if let Err(error) = result {
                    self.notice(&error, true);
                }
                apply_appearance(&ui, &self.locale.borrow());
                if let Some(tray) = self.tray.as_ref().and_then(slint::Weak::upgrade) {
                    tray.set_tray_icon(ui.get_system_icon());
                }
            }
            "language" => {
                let language = [Language::Auto, Language::Russian, Language::English]
                    .get(ui.get_language_index() as usize)
                    .copied()
                    .unwrap_or(Language::Auto);
                let result = self.locale.borrow_mut().set_language(language);
                if let Err(error) = result {
                    self.notice(&error, true);
                }
                let global = ui.global::<I18n>();
                global.set_revision(global.get_revision() + 1);
                if let Some(tray) = self.tray.as_ref().and_then(slint::Weak::upgrade) {
                    let global = tray.global::<I18n>();
                    global.set_revision(global.get_revision() + 1);
                }
                apply_appearance(&ui, &self.locale.borrow());
                self.refresh(false);
            }
            "Disconnect" | "Exit" | "Diagnose" | "Update" => self.request(json!({"action":action})),
            _ => {}
        }
    }
}

fn apply_appearance(ui: &AppWindow, locale: &Locale) {
    #[cfg(not(windows))]
    crate::portable::configure_window(ui, locale);
    let design = locale.design;
    mukhomor_ui::visuals::apply(
        ui,
        mukhomor_ui::visuals::appearance::Theme::read(Some(design.theme.id())),
        design.effects,
    );
    ui.set_animation(design.animation);
    ui.set_animation_speed(design.animation_speed as i32);
    let p = ui.global::<Palette>();
    p.set_rounded(design.rounding);
    p.set_scale(design.interface_size.percent() as f32 / 100.0);
    ui.set_theme_index(
        Theme::ALL
            .iter()
            .position(|t| *t == design.theme)
            .unwrap_or(0) as i32,
    );
    ui.set_theme_names(
        Rc::new(VecModel::from(
            Theme::ALL
                .iter()
                .map(|t| locale.text(t.name()).as_ref().into())
                .collect::<Vec<slint::SharedString>>(),
        ))
        .into(),
    );
    ui.set_scale_index(match design.interface_size {
        InterfaceSize::Small => 0,
        InterfaceSize::Medium => 1,
        InterfaceSize::Large => 2,
    });
    ui.set_rounding(design.rounding);
    ui.set_language_index(match locale.language() {
        Language::Auto => 0,
        Language::Russian => 1,
        Language::English => 2,
    });
}
#[cfg(windows)]
fn choose_file(ui: &AppWindow, save: bool, preset: bool) -> Option<PathBuf> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buffer = vec![0u16; 32768];
    let filter = crate::platform::wide(if preset {
        "Mukhomor preset\0*.json\0\0"
    } else {
        "VPN configuration\0*.conf;*.ovpn;*.yaml;*.yml;*.json;*.txt\0All files\0*.*\0\0"
    });
    let extension = crate::platform::wide("json");
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of_val(&dialog) as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    if let Ok(handle) = ui.window().window_handle().window_handle() {
        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
            dialog.hwndOwner = handle.hwnd.get() as _;
        }
    }
    dialog.Flags = OFN_PATHMUSTEXIST
        | OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    if preset {
        dialog.lpstrDefExt = extension.as_ptr();
    }
    if unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    } == 0
    {
        return None;
    }
    let count = buffer.iter().position(|c| *c == 0)?;
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..count])))
}
fn read_import(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(512 * 1024 + 4)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "Файл должен иметь кодировку UTF-8")?;
    let text = text.trim_start_matches('\u{feff}');
    if text.len() > 512 * 1024 {
        return Err("Конфигурация слишком большая (максимум 512 КБ)".into());
    }
    if text.trim().is_empty() {
        return Err("Файл конфигурации пуст".into());
    }
    Ok(text.into())
}

pub fn run(root: PathBuf) -> Result<(), String> {
    let startup = std::env::args().any(|a| a == "--startup");
    #[cfg(target_os = "linux")]
    let Some(instance) = crate::portable::Instance::acquire(&root, startup)? else { return Ok(()); };
    #[cfg(windows)]
    let identity = crate::desktop::window_identity(&root)?;
    #[cfg(windows)]
    let Some(instance) = (unsafe { crate::desktop::single_instance(&identity, !startup)? }) else {
        return Ok(());
    };
    let ui = AppWindow::new().map_err(|e| e.to_string())?;
    #[cfg(target_os = "android")]
    crate::android::prepare_launch(ui.window())?;
    #[cfg(windows)]
    let proxy = ActivationWindow::new(&identity, &ui)?;
    let isolated = root.join("qa.marker").is_file();
    let locale = Rc::new(RefCell::new(Locale::new(&root, isolated)?));
    locale.borrow_mut().enable_background_saves();
    let translate = locale.clone();
    ui.global::<I18n>()
        .on_translate(move |text, _| translate.borrow().text(&text).as_ref().into());
    apply_appearance(&ui, &locale.borrow());
    let _visual_animation = mukhomor_ui::visuals::animate(&ui);
    let minimize = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = minimize.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let close = ui.as_weak();
    ui.on_close_window(move || {
        if let Some(ui) = close.upgrade() {
            let _ = ui.hide();
        }
    });
    #[cfg(windows)]
    let autostart_executable = if isolated {
        None
    } else {
        crate::setup::installed(&root).ok()
    };
    #[cfg(not(windows))]
    let autostart_executable = crate::portable::autostart_executable();
    ui.set_autostart_available(autostart_executable.is_some());
    ui.set_autostart(
        autostart_executable
            .as_ref()
            .is_some_and(|exe| crate::autostart::enabled(exe).unwrap_or(false)),
    );
    let (sender, receiver) = mpsc::channel();
    let controller_root = root.clone();
    #[cfg(windows)]
    let transport = std::sync::Arc::new(move |request: Value| {
        crate::desktop::controller_request(
            &request,
            |value| crate::backend::rpc(&controller_root, value),
            || crate::setup::ensure_controller_for_recovery(&controller_root),
        )
    });
    #[cfg(not(windows))]
    let transport = {
        // Spawn/open the controller on the request worker after the first
        // frame, so private storage and VPN initialization run under the intro.
        let prepared = std::sync::Mutex::new(None::<crate::portable::Transport>);
        std::sync::Arc::new(move |request: Value| {
            let transport = {
                let mut slot = prepared.lock().map_err(|_| "Controller preparation failed")?;
                if slot.is_none() { *slot = Some(crate::portable::transport(&controller_root)?); }
                slot.as_ref().unwrap().clone()
            };
            transport(request)
        })
    };
    let profiles = Rc::new(VecModel::default());
    ui.set_profiles(profiles.clone().into());
    let host = Rc::new(RefCell::new(Host {
        state: State::default(),
        locale,
        window: ui.as_weak(),
        tray: None,
        sender,
        receiver,
        profiles,
        profiles_value: Value::Null,
        selected_value: String::new(),
        nonce: format!(
            "slint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        status_pending: false,
        last_status: Instant::now(),
        created: Instant::now(),
        transport_notice: Cell::new(false),
        notice_deadline: Cell::new(None),
        operation_inflight: None,
        startup_pending: cfg!(target_os = "android") || std::env::args().any(|a| a == "--startup"),
        startup: Startup::default(),
        autostart_executable,
        rule_owner: String::new(),
        preset_serial: 0,
        validation_draft: Value::Null,
        exit_after_save: false,
        transport,
    }));
    let commands = host.clone();
    ui.on_command(move |action, id, text| {
        if let Ok(mut host) = commands.try_borrow_mut() {
            host.command(&action, &id, &text);
        }
    });
    let edits = host.clone();
    ui.on_edit_rules(move |text| {
        if let Ok(mut host) = edits.try_borrow_mut() {
            host.edit_rules(&text);
        }
    });
    let results = host.clone();
    ui.on_worker_wakeup(move || {
        if let Ok(mut host) = results.try_borrow_mut() {
            host.receive();
        }
    });
    let timer = slint::Timer::default();
    let poll = host.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            if let Ok(mut host) = poll.try_borrow_mut() {
                #[cfg(target_os = "linux")]
                if instance.activation_requested() { if let Some(ui)=host.window.upgrade() { activate(&ui); } }
                host.receive();
                host.refresh_startup();
                if let Some(Err(error)) = host.locale.borrow().saved_result() {
                    host.notice(&error, true);
                }
                if host
                    .notice_deadline
                    .get()
                    .is_some_and(|at| Instant::now() >= at)
                {
                    host.notice("", false);
                }
                let interval =
                    if !host.state.available() || host.state.busy() || host.startup_pending {
                        Duration::from_millis(250)
                    } else {
                        Duration::from_secs(3)
                    };
                if host.last_status.elapsed() < interval {
                    return;
                }
                if host.state.pending.is_some()
                    || host.startup_pending
                    || host
                        .window
                        .upgrade()
                        .is_some_and(|ui| ui.window().is_visible())
                {
                    let request = host.state.status_request();
                    host.launch(request);
                }
            }
        },
    );
    #[cfg(not(target_os = "android"))]
    let tray = Tray::new().map_err(|e| e.to_string())?;
    let tray_available = Cell::new(false);
    #[cfg(not(target_os = "android"))]
    {
    host.borrow_mut().tray = Some(tray.as_weak());
    tray.set_tray_icon(ui.get_system_icon());
    let translate = host.borrow().locale.clone();
    tray.global::<I18n>()
        .on_translate(move |text, _| translate.borrow().text(&text).as_ref().into());
    let show = ui.as_weak();
    tray.on_show_window(move || {
        if let Some(ui) = show.upgrade() {
            activate(&ui);
        }
    });
    let disconnect = host.clone();
    tray.on_disconnect(move || {
        disconnect
            .borrow_mut()
            .request(json!({"action":"Disconnect"}))
    });
    let exit = host.clone();
    tray.on_exit(move || exit.borrow_mut().request(json!({"action":"Exit"})));
    #[cfg(windows)]
    { tray.show().map_err(|e| e.to_string())?; tray_available.set(true); }
    #[cfg(target_os = "linux")]
    { tray_available.set(tray.show().is_ok()); }
    }
    ui.set_background_available(cfg!(target_os = "android") || tray_available.get());
    #[cfg(not(target_os = "android"))]
    let can_hide = tray_available.get();
    #[cfg(not(target_os = "android"))]
    let close = host.clone();
    ui.on_close_window(move || {
        #[cfg(target_os = "android")]
        { let _ = crate::android::call("background", ""); }
        #[cfg(not(target_os = "android"))]
        if can_hide { if let Some(ui)=close.borrow().window.upgrade() { let _=ui.hide(); } }
        else { close.borrow_mut().request(json!({"action":"Exit"})); }
    });
    let close = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = close.upgrade() {
            if cfg!(target_os = "android") && ui.get_dialog() != 0 { ui.set_dialog(0); }
            else { ui.invoke_close_window(); }
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    host.borrow_mut().refresh(false);
    host.borrow_mut().launch(json!({"action":"Status"}));
    if !startup || !tray_available.get() {
        ui.show().map_err(|e| e.to_string())?;
    }
    slint::run_event_loop().map_err(|e| e.to_string())?;
    timer.stop();
    #[cfg(not(target_os = "android"))]
    let _ = tray.hide();
    #[cfg(windows)]
    drop(proxy);
    #[cfg(not(target_os = "android"))]
    drop(tray);
    drop(ui);
    drop(host);
    #[cfg(windows)]
    drop(instance);
    Ok(())
}

thread_local! { static ACTIVE_WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) }; }
/// Keep the existing per-user/per-root activation and installer retirement
/// protocol. Winit pumps this hidden HWND on the same Windows event thread.
#[cfg(windows)]
struct ActivationWindow {
    hwnd: windows_sys::Win32::Foundation::HWND,
    class: Vec<u16>,
}
#[cfg(windows)]
impl ActivationWindow {
    fn new(identity: &str, ui: &AppWindow) -> Result<Self, String> {
        use windows_sys::Win32::{
            System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
        };
        let class = crate::platform::wide(identity);
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(activation_procedure);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            if RegisterClassW(&wc) == 0 {
                return Err("Cannot register the Mukhomor activation window".into());
            }
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                UnregisterClassW(class.as_ptr(), instance);
                return Err("Cannot create the Mukhomor activation window".into());
            }
            ACTIVE_WINDOW.with(|active| *active.borrow_mut() = Some(ui.as_weak()));
            Ok(Self { hwnd, class })
        }
    }
}
#[cfg(windows)]
impl Drop for ActivationWindow {
    fn drop(&mut self) {
        use windows_sys::Win32::{
            System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
        };
        ACTIVE_WINDOW.with(|active| *active.borrow_mut() = None);
        unsafe {
            DestroyWindow(self.hwnd);
            UnregisterClassW(self.class.as_ptr(), GetModuleHandleW(std::ptr::null()));
        }
    }
}
#[cfg(windows)]
unsafe extern "system" fn activation_procedure(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if message == WM_APP + 3 {
        ACTIVE_WINDOW.with(|active| {
            if let Some(ui) = active.borrow().as_ref().and_then(slint::Weak::upgrade) {
                activate(&ui);
            }
        });
        0
    } else if message == WM_APP + 4 || message == WM_ENDSESSION && wparam != 0 {
        let _ = slint::quit_event_loop();
        0
    } else if message == WM_QUERYENDSESSION {
        1
    } else {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
}

#[cfg(windows)]
fn activate(ui: &AppWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    let _ = ui.show();
    if let Ok(handle) = ui.window().window_handle().window_handle() {
        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
            unsafe {
                ShowWindow(handle.hwnd.get() as _, SW_RESTORE);
                SetForegroundWindow(handle.hwnd.get() as _);
            }
        }
    }
    ui.invoke_command("refresh".into(), "".into(), "".into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        time::Instant,
    };

    struct Headless(Rc<slint::platform::software_renderer::MinimalSoftwareWindow>);
    impl slint::platform::Platform for Headless {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    fn drain(host: &Rc<RefCell<Host>>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            {
                let mut host = host.borrow_mut();
                host.receive();
                if host.state.pending.is_none() && !host.status_pending {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "mock controller did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn ui_commands_preserve_rule_ownership_and_save_or_discard_before_exit() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(surface))).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.global::<I18n>().on_translate(|text, _| text);
        let fixture_root =
            std::env::temp_dir().join(format!("mukhomor-slint-viewmodel-{}", std::process::id()));
        let locale = Rc::new(RefCell::new(Locale::new(&fixture_root, true).unwrap()));
        let snapshot = json!({"profiles":[{"id":"server-a","name":"Alpha","protocol":"vless"}],
            "settings":crate::presets::defaults(),"service_ready":true,"phase":"idle","selected":"server-a"});
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recording = requests.clone();
        let data = Arc::new(Mutex::new(snapshot.clone()));
        let transport = Arc::new(move |request: Value| {
            recording.lock().unwrap().push(request.clone());
            let mut data = data.lock().unwrap();
            if request["action"] == "ApplySettings" {
                data["settings"] = request["settings"].clone();
            }
            if request["action"] == "ValidateSettings" {
                data["rule_issues"] = json!([]);
            }
            data["last_operation"] = json!({"request_id":request["request_id"],"ok":true});
            Ok(json!({"ok":true,"request_id":request["request_id"],"data":data.clone()}))
        });
        let (sender, receiver) = mpsc::channel();
        let profiles = Rc::new(VecModel::default());
        ui.set_profiles(profiles.clone().into());
        let host = Rc::new(RefCell::new(Host {
            state: State::default(),
            locale,
            window: ui.as_weak(),
            tray: None,
            sender,
            receiver,
            profiles,
            profiles_value: Value::Null,
            selected_value: String::new(),
            nonce: "fixture".into(),
            status_pending: false,
            last_status: Instant::now(),
            created: Instant::now(),
            transport_notice: Cell::new(false),
            notice_deadline: Cell::new(None),
            operation_inflight: None,
            startup_pending: false,
            startup: Startup::default(),
            autostart_executable: None,
            rule_owner: String::new(),
            preset_serial: 0,
            validation_draft: Value::Null,
            exit_after_save: false,
            transport,
        }));
        let mut preview = snapshot.clone();
        preview["initializing"] = json!(true);
        preview["service_ready"] = json!(false);
        host.borrow_mut().state.receive(0, "Status", Ok(json!({"ok":false,"data":preview})));
        host.borrow_mut().refresh(true);
        assert!(ui.get_boot_active(), "metadata preview is not ready");
        host.borrow_mut().launch(json!({"action":"Status"}));
        drain(&host);
        assert!(!ui.get_boot_active(), "real worker receipt must finish launch");
        assert!(requests.lock().unwrap().iter().all(|r| r["action"] == "Status"));
        requests.lock().unwrap().clear();
        host.borrow_mut().refresh(true);
        {
            let mut h = host.borrow_mut();
            h.sender
                .send(Reply {
                    generation: 0,
                    action: "Status".into(),
                    result: Err("Temporary transport failure".into()),
                })
                .unwrap();
            h.receive();
            assert!(ui.get_notice_error());
            assert!(!ui.get_notice().is_empty());
            let data = h.state.snapshot.clone();
            h.sender
                .send(Reply {
                    generation: 0,
                    action: "Status".into(),
                    result: Ok(json!({"ok":true,"data":data})),
                })
                .unwrap();
            h.receive();
            assert!(
                ui.get_notice().is_empty(),
                "recovered status must clear the transient banner"
            );
            h.notice("Actual operation failure", true);
            let data = h.state.snapshot.clone();
            h.sender
                .send(Reply {
                    generation: 0,
                    action: "Status".into(),
                    result: Ok(json!({"ok":true,"data":data})),
                })
                .unwrap();
            h.receive();
            assert_eq!(ui.get_notice(), "Actual operation failure");
            h.notice("", false);
        }
        let commands = host.clone();
        ui.on_command(move |action, id, text| commands.borrow_mut().command(&action, &id, &text));
        let edits = host.clone();
        ui.on_edit_rules(move |text| edits.borrow_mut().edit_rules(&text));

        // The exact callbacks used by the .slint controls, not just reducer calls.
        ui.invoke_command("preset-create".into(), "".into(), "Personal fixture".into());
        let created = host.borrow().state.draft["presets"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ui.invoke_command("preset-edit".into(), created.as_str().into(), "".into());
        ui.set_rule_section(2);
        ui.invoke_edit_rules("example.invalid\nsecond.invalid".into());
        {
            let host = host.borrow();
            let i = crate::presets::index(&host.state.draft, &created).unwrap();
            assert_eq!(
                host.state.draft["presets"][i]["rules"]["domains"],
                json!(["example.invalid", "second.invalid"])
            );
            assert!(
                host.state.draft["direct"]["domains"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        ui.invoke_command("Exit".into(), "".into(), "".into());
        assert_eq!(ui.get_dialog(), 9);
        assert!(requests.lock().unwrap().is_empty());
        ui.invoke_command("save-exit".into(), "".into(), "".into());
        drain(&host);
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| r["action"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            ["ApplySettings", "Exit"]
        );
        assert!(!host.borrow().state.dirty());

        ui.invoke_edit_rules("unsaved.invalid".into());
        ui.invoke_command("Exit".into(), "".into(), "".into());
        assert_eq!(ui.get_dialog(), 9);
        ui.invoke_command("discard-exit".into(), "".into(), "".into());
        drain(&host);
        assert_eq!(requests.lock().unwrap().last().unwrap()["action"], "Exit");
        assert!(!host.borrow().state.dirty());

        ui.invoke_command("refresh".into(), "".into(), "".into());
        drain(&host);
        assert_eq!(requests.lock().unwrap().last().unwrap()["action"], "Status");
        assert_eq!(ui.get_selected_id(), "server-a");

        ui.set_import_text("public fixture text".into());
        ui.set_import_source("fixture.txt".into());
        ui.invoke_command("Import".into(), "".into(), "".into());
        drain(&host);
        let recorded = requests.lock().unwrap();
        let import = recorded.last().unwrap();
        assert_eq!(import["action"], "Import");
        assert_eq!(import["content"], "public fixture text");
        assert_eq!(import["source_name"], "fixture.txt");
        assert!(ui.get_import_text().is_empty());
    }
}

#[cfg(target_os = "linux")]
fn activate(ui: &AppWindow) { let _ = ui.show(); ui.window().set_minimized(false); }
#[cfg(not(windows))]
fn choose_file(ui: &AppWindow, save: bool, preset: bool) -> Option<PathBuf> {
    crate::portable::choose_file(ui, save, preset)
}
