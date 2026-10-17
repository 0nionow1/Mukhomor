//! Isolated visual fixture. Never imports a profile or calls a VPN controller.
use mukhomor_ui::{
    AppWindow, I18n, Palette, Preset, Profile,
    slint::{self, ComponentHandle, Model, VecModel},
};
use std::{cell::RefCell, path::Path, rc::Rc};
fn sample(ui: &AppWindow, count: usize) {
    ui.set_animation(false);
    ui.set_boot_active(false);
    ui.set_boot_error("".into());
    ui.set_mobile(false);
    ui.global::<I18n>().on_translate(|text, _| text);
    mukhomor_ui::visuals::apply(ui, mukhomor_ui::visuals::appearance::Theme::Neon, true);
    ui.set_phase("idle".into());
    ui.set_active_network("vps".into());
    ui.set_active_name("Сервер 01".into());
    ui.set_pending_action("".into());
    ui.set_connection_failed(false);
    ui.set_live_profile_switch(true);
    ui.set_notice_error(false);
    ui.set_ready(true);
    ui.set_busy(false);
    ui.set_update_busy(false);
    ui.set_update_available(false);
    ui.set_update_install_enabled(false);
    ui.set_update_status("".into());
    ui.set_update_version("".into());
    ui.set_update_progress(-1);
    ui.set_loaded(true);
    ui.set_autostart_available(true);
    ui.set_autoconnect(true);
    ui.set_autostart(false);
    ui.set_chosen_preset_id("".into());
    ui.set_routing_editable(true);
    ui.set_rounding(true);
    ui.set_animation(true);
    ui.set_primary_enabled(count > 0);
    ui.set_tor_enabled(true);
    ui.set_p2p_selected("tor".into());
    ui.set_tor_method_label("Snowflake".into());
    ui.set_primary_stops_connection(false);
    ui.set_connection_mode(0);
    ui.set_notice("".into());
    ui.window().set_minimized(false);
    ui.set_profiles(
        Rc::new(VecModel::from(
            (0..count)
                .map(|i| Profile {
                    id: format!("preview-{i}").into(),
                    name: if i == 1 {
                        "Очень длинное название сервера для проверки границ списка и текста".into()
                    } else {
                        format!("Сервер {:02}", i + 1).into()
                    },
                    protocol: ["WireGuard", "VLESS", "Mycelium"][i % 3].into(),
                    selected: i == 0,
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    ui.set_selected_id(if count == 0 { "" } else { "preview-0" }.into());
    ui.set_selected_name(if count == 0 { "" } else { "Сервер 01" }.into());
    ui.set_theme_names(
        Rc::new(VecModel::from(
            mukhomor_ui::visuals::appearance::Theme::ALL
                .iter()
                .map(|theme| theme.name().into())
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    ui.set_rule_owners(
        Rc::new(VecModel::from(vec!["Личные правила".into(), "Игры".into()])).into(),
    );
    ui.set_template_names(Rc::new(VecModel::from(vec!["Игры".into(), "Steam".into()])).into());
    ui.set_presets(
        Rc::new(VecModel::from(vec![
            Preset {
                id: "preview-games".into(),
                name: "Игры".into(),
                description: "Игровые серверы и приложения".into(),
                enabled: true,
                builtin: true,
            },
            Preset {
                id: "preview-private".into(),
                name: "Мой набор".into(),
                description: "Личные правила".into(),
                enabled: false,
                builtin: false,
            },
        ]))
        .into(),
    );
}

struct Headless(Rc<slint::platform::software_renderer::MinimalSoftwareWindow>);
impl slint::platform::Platform for Headless {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}
fn render(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(surface.clone())))?;
    std::fs::create_dir_all(directory)?;
    let ui = AppWindow::new()?;
    ui.show()?;
    for (name, count, width, height, scale, page, dialog) in [
        ("startup-opening", 0, 560, 828, 1., 0, 0),
        ("startup-growing", 0, 560, 828, 1., 0, 0),
        ("startup-mobile", 0, 360, 640, 1., 0, 0),
        ("startup-error", 0, 360, 580, 1., 0, 0),
        ("startup-reduced-motion", 0, 560, 828, 1., 0, 0),
        ("connection-reference", 12, 560, 828, 1., 0, 0),
        ("connection-paper", 12, 560, 828, 1., 0, 0),
        ("connection-glacier", 12, 560, 828, 1., 0, 0),
        ("connection-empty", 0, 560, 828, 1., 0, 0),
        ("connection-short", 2, 560, 828, 1., 0, 0),
        ("connection-long", 512, 560, 828, 1., 0, 0),
        ("connection-bottom", 512, 560, 828, 1., 0, 0),
        ("connection-scrolled-selection", 512, 560, 828, 1., 0, 0),
        ("connection-compact", 512, 360, 640, 1., 0, 0),
        ("connection-minimum", 512, 360, 580, 1., 0, 0),
        ("connection-small", 512, 560, 828, 0.85, 0, 0),
        ("connection-large", 512, 620, 1010, 1.25, 0, 0),
        ("import", 2, 560, 828, 1., 0, 1),
        ("import-compact", 2, 360, 640, 1., 0, 1),
        ("rename-compact", 2, 360, 640, 1., 0, 2),
        ("rules", 2, 560, 828, 1., 1, 0),
        ("rules-compact", 2, 360, 640, 1., 1, 0),
        ("rules-dirty-compact", 2, 360, 640, 1., 1, 0),
        ("presets", 2, 560, 828, 1., 2, 0),
        ("presets-paper", 2, 560, 828, 1., 2, 0),
        ("presets-focus", 2, 560, 828, 1., 2, 0),
        ("presets-compact", 2, 360, 640, 1., 2, 0),
        ("presets-long", 2, 560, 828, 1., 2, 0),
        ("preset-editor", 2, 560, 828, 1., 2, 4),
        ("settings", 2, 560, 828, 1., 3, 0),
        ("update-available", 2, 560, 828, 1., 3, 0),
        ("update-mobile", 2, 360, 640, 1., 3, 0),
        ("update-downloading", 2, 360, 640, 1., 3, 0),
        ("settings-paper", 2, 560, 828, 1., 3, 0),
        ("settings-disabled", 2, 560, 828, 1., 3, 0),
        ("settings-compact", 2, 360, 640, 1., 3, 0),
        ("settings-bottom", 2, 560, 828, 1., 3, 0),
        ("settings-bottom-compact", 2, 360, 640, 1., 3, 0),
        ("settings-bottom-small", 2, 560, 828, 0.85, 3, 0),
        ("settings-bottom-large", 2, 620, 1010, 1.25, 3, 0),
        ("appearance", 2, 560, 828, 1., 3, 0),
        ("appearance-dropdown", 2, 560, 828, 1., 3, 0),
        ("dirty-exit", 2, 560, 828, 1., 1, 9),
        ("dirty-exit-compact", 2, 360, 640, 1., 1, 9),
        ("website-connection", 3, 560, 828, 1., 0, 0),
        ("website-rules", 3, 560, 828, 1., 1, 0),
        ("website-connection-paper", 3, 560, 828, 1., 0, 0),
        ("website-connection-glacier", 3, 560, 828, 1., 0, 0),
        ("website-rules-paper", 3, 560, 828, 1., 1, 0),
        ("website-rules-glacier", 3, 560, 828, 1., 1, 0),
        ("website-presets-paper", 3, 560, 828, 1., 2, 0),
        ("website-presets-glacier", 3, 560, 828, 1., 2, 0),
        ("website-appearance-paper", 3, 560, 828, 1., 3, 0),
        ("website-appearance-glacier", 3, 560, 828, 1., 3, 0),
        ("navigation-focus", 2, 560, 828, 1., 3, 0),
        ("navigation-focus-compact", 2, 360, 640, 1., 3, 0),
        ("action-focus", 2, 560, 828, 1., 3, 0),
        ("action-focus-paper", 2, 560, 828, 1., 3, 0),
        ("mode-tor", 12, 560, 828, 1., 0, 0),
        ("mode-tor-compact", 0, 360, 640, 1., 0, 0),
        ("mode-i2p", 0, 560, 828, 1., 0, 0),
        ("mode-i2p-compact", 0, 360, 640, 1., 0, 0),
        ("mode-tor-bridges", 0, 560, 828, 1., 0, 12),
        ("mode-tor-bridges-compact", 0, 360, 640, 1., 0, 12),
        ("mode-tor-amp-compact", 0, 360, 640, 1., 0, 12),
        ("mode-subscription", 12, 560, 828, 1., 0, 0),
        ("mode-subscription-paper", 12, 560, 828, 1., 0, 0),
        ("mode-subscription-glacier", 12, 560, 828, 1., 0, 0),
        ("mode-subscription-compact", 12, 360, 640, 1., 0, 0),
        ("mode-subscription-english", 12, 360, 640, 1., 0, 0),
        ("modes-interaction", 12, 560, 828, 1., 0, 0),
        ("connection-minimum-notice", 512, 360, 580, 1., 0, 0),
        ("ux-connecting-tor", 2, 560, 828, 1., 0, 0),
        ("ux-connecting-i2p", 0, 360, 640, 1., 0, 0),
        ("ux-connected-tor-vps", 2, 560, 828, 1., 0, 0),
        ("ux-connected-vps-p2p", 2, 360, 640, 1., 0, 0),
        ("ux-connected-subscription", 2, 560, 828, 1., 0, 0),
        ("ux-connected-i2p", 2, 360, 580, 1., 0, 0),
        ("ux-switching", 2, 560, 828, 1., 0, 0),
        ("ux-selecting", 2, 360, 640, 1., 0, 0),
        ("ux-disconnecting", 2, 360, 580, 1., 0, 0),
        ("ux-uncertain", 2, 560, 828, 1., 0, 0),
        ("ux-recovery", 2, 360, 640, 1., 0, 0),
        ("ux-error", 2, 360, 580, 1., 0, 0),
        ("ux-long-name", 2, 360, 640, 1., 0, 0),
    ] {
        sample(&ui, count);
        if name.starts_with("ux-") {
            let phase = match name {
                "ux-connecting-tor" | "ux-connecting-i2p" => "connecting",
                "ux-switching" => "switching",
                "ux-disconnecting" => "disconnecting",
                "ux-uncertain" => "uncertain",
                "ux-recovery" => "recovery",
                "ux-selecting" | "ux-error" => "idle",
                _ => "connected",
            };
            ui.set_phase(phase.into());
            ui.set_primary_enabled(true);
            ui.set_primary_stops_connection(phase != "idle");
            ui.set_busy(matches!(phase, "connecting" | "switching" | "disconnecting") || name == "ux-selecting");
            if name.contains("i2p") || name.contains("tor") {
                let network = if name.contains("i2p") { "i2p" } else { "tor" };
                ui.set_active_network(network.into());
                ui.set_active_name(if network == "tor" { "Tor" } else { "I2P" }.into());
                ui.set_p2p_selected(network.into());
                ui.set_connection_mode(if name.ends_with("-vps") { 0 } else { 1 });
            }
            if name == "ux-connected-vps-p2p" { ui.set_connection_mode(1); }
            if name == "ux-connected-subscription" { ui.set_connection_mode(2); }
            if name == "ux-selecting" { ui.set_pending_action("Select".into()); ui.set_primary_enabled(false); }
            if name == "ux-error" {
                ui.set_connection_failed(true);
                ui.set_notice_error(true);
                ui.set_notice("Не удалось связаться с сервером. Проверьте подключение к интернету, адрес сервера и настройки профиля, затем повторите подключение.".into());
            }
            if name == "ux-long-name" {
                ui.set_active_name("Очень длинное название подключённого сервера для проверки текста состояния".into());
            }
            assert!(!ui.get_status_text().is_empty() && !ui.get_connection_hint().is_empty());
            if name == "ux-disconnecting" { assert!(!ui.get_connection_action_enabled()); }
            if name == "ux-error" { assert_eq!(ui.get_primary_text().as_str(), "Повторить"); }
            if name == "ux-selecting" { assert!(!ui.get_connection_action_enabled()); }
            if name == "ux-connected-vps-p2p" || name == "ux-connected-tor-vps" || name == "ux-connected-subscription" {
                assert_eq!(ui.get_primary_text().as_str(), "Отключиться");
                assert!(ui.get_connection_action_enabled());
                assert!(ui.get_connection_hint().contains("Вкладка не меняет VPN"));
            }
        }
        if name == "connection-minimum-notice" {
            ui.set_notice("Конфигурация добавлена".into());
        }
        if name.starts_with("mode-tor-bridges") { ui.set_tor_method(1); }
        if name == "mode-tor-amp-compact" { ui.set_tor_method(4); }
        if name.starts_with("mode-") {
            ui.set_connection_mode(if name.starts_with("mode-tor") || name.starts_with("mode-i2p") { 1 } else { 2 });
        }
        if name.starts_with("mode-i2p") { ui.set_p2p_selected("i2p".into()); }
        if name == "mode-subscription-english" {
            ui.global::<I18n>().on_translate(|text, _| match text.as_str() {
                "Свой VPS" => "Own VPS".into(),
                "Подписка" => "Subscription".into(),
                "Подписка Mukhomor" => "Mukhomor subscription".into(),
                "В разработке" => "In development".into(),
                "Скоро" => "Coming soon".into(),
                "Не подключено" => "Disconnected".into(),
                "Подписка в разработке. Выберите VPS или P2P." => "Subscription is coming soon. Choose VPS or P2P.".into(),
                "Подключение" => "Connection".into(),
                "Правила" => "Rules".into(),
                "Наборы" => "Presets".into(),
                "Настройки" => "Settings".into(),
                _ => text,
            });
        }
        if name.starts_with("update-") {
            ui.set_update_available(true);
            ui.set_update_install_enabled(true);
            ui.set_update_version("0.8.4".into());
            ui.set_update_status("Доступна новая версия".into());
            ui.set_mobile(name == "update-mobile");
            if name == "update-downloading" {
                ui.set_update_busy(true);
                ui.set_update_install_enabled(false);
                ui.set_update_status("Загружаем обновление…".into());
                ui.set_update_progress(42);
            }
        }
        if matches!(
            name,
            "connection-reference" | "connection-paper" | "connection-glacier"
        ) {
            let names = [
                "France · Paris",
                "Netherlands · Amsterdam",
                "Germany · Frankfurt",
                "Finland · Helsinki",
                "Sweden · Stockholm",
                "Poland · Warsaw",
                "United Kingdom · London",
                "Spain · Madrid",
                "Italy · Milan",
                "Austria · Vienna",
                "Switzerland · Zurich",
                "Czechia · Prague",
            ];
            ui.set_profiles(
                Rc::new(VecModel::from(
                    names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| Profile {
                            id: format!("preview-{i}").into(),
                            name: (*name).into(),
                            protocol: "".into(),
                            selected: i == 0,
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            ui.set_phase("connected".into());
            ui.set_primary_stops_connection(true);
            ui.set_active_name("France · Paris".into());
        }
        if name.ends_with("paper") {
            mukhomor_ui::visuals::apply(&ui, mukhomor_ui::visuals::appearance::Theme::Paper, true);
        } else if name.ends_with("glacier") {
            mukhomor_ui::visuals::apply(
                &ui,
                mukhomor_ui::visuals::appearance::Theme::Glacier,
                true,
            );
        }
        if name == "presets-long" {
            ui.set_presets(
                Rc::new(VecModel::from(
                    (0..64)
                        .map(|i| Preset {
                            id: format!("preview-preset-{i}").into(),
                            name: format!("Набор {:02}", i + 1).into(),
                            enabled: i % 2 == 0,
                            ..Default::default()
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
        }
        ui.set_appearance_open(name.starts_with("appearance") || name.starts_with("website-appearance"));
        let theme = if name.ends_with("paper") {
            mukhomor_ui::visuals::appearance::Theme::Paper
        } else if name.ends_with("glacier") {
            mukhomor_ui::visuals::appearance::Theme::Glacier
        } else {
            mukhomor_ui::visuals::appearance::Theme::Neon
        };
        ui.set_theme_index(mukhomor_ui::visuals::appearance::Theme::ALL.iter().position(|item| *item == theme).unwrap() as i32);
        if name == "settings-disabled" {
            ui.set_autostart_available(false);
            ui.set_ready(false);
        }
        ui.set_page(page);
        ui.set_dialog(dialog);
        ui.set_dirty(name.contains("dirty"));
        ui.global::<Palette>().set_scale(scale);
        ui.set_rules_text(
            (0..120)
                .map(|i| format!("example-{i}.invalid"))
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        );
        // Public website fixtures use only synthetic profiles and valid example
        // process names. They never read a configuration or start a connection.
        if name.starts_with("website-") {
            ui.set_profiles(Rc::new(VecModel::from(vec![
                Profile { id: "preview-0".into(), name: "Мой VPS".into(), protocol: "WireGuard".into(), selected: true },
                Profile { id: "preview-1".into(), name: "Рабочий сервер".into(), protocol: "VLESS".into(), selected: false },
                Profile { id: "preview-2".into(), name: "Запасной сервер".into(), protocol: "Hysteria 2".into(), selected: false },
            ])).into());
            ui.set_selected_name("Мой VPS".into());
            ui.set_rule_section(0);
            ui.set_rules_text("game.exe\nlauncher.exe".into());
        }
        surface.set_size(slint::PhysicalSize::new(width, height));
        if !name.starts_with("startup-") {
            ui.set_animation(false);
            mukhomor_ui::visuals::preview_frame(&ui, theme, 2800);
        }
        if name.starts_with("startup-") {
            ui.set_animation(false);
            ui.set_boot_active(true);
            ui.set_phase("loading".into());
            ui.set_boot_status("Загружаем профили и правила…".into());
            ui.set_mobile(name == "startup-mobile");
            if name == "startup-error" {
                ui.set_boot_status("Не удалось подготовить приложение".into());
                ui.set_boot_error("Контроллер VPN недоступен. Повторите проверку.".into());
                ui.set_boot_retry_enabled(true);
            }
            slint::platform::update_timers_and_animations();
            ui.set_animation(name != "startup-reduced-motion");
            mukhomor_ui::visuals::preview_frame(&ui, mukhomor_ui::visuals::appearance::Theme::Neon, if name == "startup-opening" { 600 } else { 2800 });
        }
        slint::platform::update_timers_and_animations();
        ui.invoke_scroll_servers(match name {
            "connection-bottom" => 1.,
            "connection-scrolled-selection" => 0.001,
            _ => 0.,
        });
        let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
        surface.request_redraw();
        let mut drawn = false;
        surface.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
            drawn = true;
        });
        assert!(drawn, "scene {name} was not rendered");
        if name == "ux-error" {
            let original = pixels.clone();
            ui.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(100., ui.get_connection_hint_bottom() - 12.),
                delta_x: 0., delta_y: -1000.,
            });
            surface.request_redraw();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, width as usize); });
            assert_ne!(pixels, original, "long error message cannot be scrolled to read its end");
            ui.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(100., ui.get_connection_hint_bottom() - 12.),
                delta_x: 0., delta_y: 1000.,
            });
            surface.request_redraw();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, width as usize); });
        }
        if name.starts_with("settings-bottom") {
            ui.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(width as f32 / 2., height as f32 / 2.),
                delta_x: 0.,
                delta_y: -10000.,
            });
            slint::platform::update_timers_and_animations();
            surface.request_redraw();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, width as usize); });
        }
        if name == "modes-interaction" {
            use slint::platform::{PointerEventButton, WindowEvent};
            let commands = Rc::new(RefCell::new(Vec::new()));
            let captured = commands.clone();
            ui.on_command(move |action, _, _| captured.borrow_mut().push(action));
            let click = |x, y| {
                let position = slint::LogicalPosition::new(x, y);
                ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
            };
            for (mode, x) in [(1, 280.), (2, 472.)] {
                click(x, 124.);
                assert_eq!(ui.get_connection_mode(), mode, "mode tab did not switch");
                assert_eq!(ui.get_primary_text().as_str(), if mode == 1 { "Подключиться" } else { "Скоро" });
                assert!(ui.get_connection_hint().contains(if mode == 1 { "Tor" } else { "разработке" }));
                assert_eq!(ui.get_connection_action_enabled(), mode == 1, "Tor must connect without a VPS; subscription stays unavailable");
                click(280., ui.get_connection_caption_top() + 12.);
                assert_eq!(commands.borrow().len(), if mode == 1 { 1 } else { 0 });
                if mode == 1 { assert_eq!(commands.borrow()[0].as_str(), "primary"); }
                commands.borrow_mut().clear();
                click(100., ui.get_server_list_top() + 22.);
                click(100., ui.get_primary_action_top() + 20.);
                if mode == 1 {
                    assert_eq!(commands.borrow().iter().map(|s|s.as_str()).collect::<Vec<_>>(),vec!["p2p-select","tor-options"],"P2P selection must only reach its own actions");
                } else { assert!(commands.borrow().is_empty(), "unavailable mode reached VPS actions"); }
                commands.borrow_mut().clear();
                assert_eq!(ui.get_dialog(), 0, "hidden import action opened");
                assert_eq!(ui.get_selected_id().as_str(), "preview-0");
                assert_eq!(ui.get_profiles().row_count(), count);
            }
            // The focused subscription tab remains operable with a keyboard.
            ui.set_connection_mode(0);
            ui.window().dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
            ui.window().dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
            assert_eq!(ui.get_connection_mode(), 2);
            // An existing connection remains stoppable from the subscription tab.
            ui.set_primary_stops_connection(true);
            click(280., ui.get_connection_caption_top() + 12.);
            assert_eq!(commands.borrow().len(), 1);
            assert_eq!(commands.borrow()[0].as_str(), "primary");
            ui.set_primary_stops_connection(false);
            click(88., 124.);
            assert_eq!(ui.get_connection_mode(), 0);
            assert!(ui.get_connection_action_enabled());
            click(280., ui.get_connection_caption_top() + 12.);
            assert_eq!(commands.borrow().len(), 2, "VPS connect stopped working");
            ui.set_dialog(1);
            click(472., 124.);
            assert_eq!(ui.get_connection_mode(), 0, "modal leaked input to mode tabs");
            ui.set_dialog(0);
            ui.on_command(|_, _, _| {});
            ui.window().dispatch_event(WindowEvent::PointerExited);
            surface.request_redraw();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, width as usize); });
        }
        if name.starts_with("navigation-focus") || name.starts_with("action-focus") {
            // Move the pointer away after clicking so these captures show the
            // retained focus/selection, without the temporary hover surface.
            use slint::platform::{PointerEventButton, WindowEvent};
            let position = if name.starts_with("navigation-focus") {
                slint::LogicalPosition::new(if width < 450 { 297. } else { 484. }, height as f32 - 45.)
            } else {
                slint::LogicalPosition::new(280., 638.)
            };
            ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
            ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
            ui.window().dispatch_event(WindowEvent::PointerExited);
            surface.request_redraw();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, width as usize); });
        }
        if name == "startup-opening" {
            use slint::platform::{PointerEventButton, WindowEvent};
            let closes = Rc::new(std::cell::Cell::new(0));
            let captured = closes.clone();
            ui.on_close_window(move || captured.set(captured.get() + 1));
            let commands = Rc::new(RefCell::new(Vec::new()));
            let captured = commands.clone();
            ui.on_command(move |action, _, _| captured.borrow_mut().push(action));
            // Preparation blocks the main controls while keeping its own close
            // button usable, including when initialization is still pending.
            for position in [slint::LogicalPosition::new(280., 180.), slint::LogicalPosition::new(520., 38.)] {
                ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
            }
            assert!(commands.borrow().is_empty(), "startup leaked input to main controls");
            assert_eq!(closes.get(), 1, "startup close button is blocked");
            ui.on_close_window(|| {});
            ui.on_command(|_, _, _| {});
            ui.window().dispatch_event(WindowEvent::PointerExited);
        }
        if name == "presets-focus" {
            use slint::platform::{PointerEventButton, WindowEvent};
            let commands = Rc::new(RefCell::new(Vec::new()));
            let captured = commands.clone();
            ui.on_command(move |action, id, _| captured.borrow_mut().push((action, id)));
            // Click within the 44px hit lane, outside the small visible square.
            let position = slint::LogicalPosition::new(28., 368.);
            ui.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
            assert_eq!(
                commands.borrow().len(),
                1,
                "checkbox hit lane did not toggle once"
            );
            assert_eq!(commands.borrow()[0].0.as_str(), "preset-toggle");
            assert_eq!(commands.borrow()[0].1.as_str(), "preview-games");
            assert!(
                ui.get_chosen_preset_id().is_empty(),
                "checking also selects the row"
            );
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
            assert_eq!(
                commands.borrow().len(),
                2,
                "Space did not toggle the focused checkbox"
            );
            ui.set_busy(true);
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
            assert_eq!(
                commands.borrow().len(),
                2,
                "disabled checkbox changed state"
            );
            ui.set_busy(false);
            ui.window().dispatch_event(WindowEvent::PointerExited);
            surface.request_redraw();
            surface.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            });
            ui.on_command(|_, _, _| {});
        }
        if name == "appearance-dropdown" {
            // Open the theme list in this fixed-size visual fixture.
            use slint::platform::{PointerEventButton, WindowEvent};
            let position = slint::LogicalPosition::new(100., 210.);
            ui.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
            let theme_index = ui.get_theme_index();
            ui.window().dispatch_event(WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(528., 250.),
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(528., 470.),
            });
            ui.window().dispatch_event(WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(528., 470.),
                button: PointerEventButton::Left,
            });
            assert_eq!(
                ui.get_theme_index(),
                theme_index,
                "scrolling selects a theme"
            );
            slint::platform::update_timers_and_animations();
            surface.request_redraw();
            surface.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            });
        }
        if page == 0 && ui.get_connection_mode() == 0 {
            assert!(ui.get_connection_hint_bottom() <= ui.get_server_list_top() - 32. * scale, "{name}: hint overlaps list heading");
            assert!(
                ui.get_server_list_bottom() < ui.get_primary_action_top(),
                "{name}: list overlaps action"
            );
            let list_width = ui.get_server_list_width();
            let content_width = ui.get_server_content_width();
            assert_eq!(
                ui.get_server_content_top(),
                ui.get_server_list_top(),
                "{name}: unwanted padding above rows"
            );
            assert_eq!(
                ui.get_server_content_bottom(),
                ui.get_server_list_bottom(),
                "{name}: unwanted padding below rows"
            );
            assert!(
                ui.get_server_scroll_top() - ui.get_server_list_top() >= 12. * scale
                    && ui.get_server_list_bottom() - ui.get_server_scroll_bottom() >= 12. * scale,
                "{name}: scrollbar enters the corner cutouts"
            );
            ui.invoke_scroll_servers(1.0);
            slint::platform::update_timers_and_animations();
            assert!(
                (ui.get_server_scroll_offset() + ui.get_server_scroll_maximum()).abs() < 1.0,
                "{name}: cannot reach last server"
            );
            assert_eq!(
                ui.get_server_list_width(),
                list_width,
                "{name}: scroll changes frame width"
            );
            ui.invoke_scroll_servers(0.0);
            if dialog == 0 && ui.get_server_scroll_maximum() > 0.0 {
                use slint::platform::{PointerEventButton, WindowEvent};
                let start = slint::LogicalPosition::new(
                    ui.get_server_scroll_center_x(),
                    ui.get_server_scroll_top() + 3.,
                );
                let end = slint::LogicalPosition::new(start.x, ui.get_server_scroll_bottom() + 1.);
                ui.window().dispatch_event(WindowEvent::PointerPressed {
                    position: start,
                    button: PointerEventButton::Left,
                });
                assert!(
                    ui.get_server_scroll_offset().abs() < 1.,
                    "{name}: grabbing the thumb jumps the list"
                );
                ui.window()
                    .dispatch_event(WindowEvent::PointerMoved { position: end });
                assert!(
                    (ui.get_server_scroll_offset() + ui.get_server_scroll_maximum()).abs() < 1.,
                    "{name}: dragging the thumb cannot reach the last server"
                );
                ui.window().dispatch_event(WindowEvent::PointerReleased {
                    position: end,
                    button: PointerEventButton::Left,
                });
                // Integer pixel placement must not round the content position
                // when grabbing a thumb halfway through a large model.
                ui.invoke_scroll_servers(0.5);
                let middle = slint::LogicalPosition::new(
                    start.x,
                    (ui.get_server_scroll_top() + ui.get_server_scroll_bottom()) / 2.,
                );
                let before_grab = ui.get_server_scroll_offset();
                ui.window().dispatch_event(WindowEvent::PointerPressed {
                    position: middle,
                    button: PointerEventButton::Left,
                });
                assert!(
                    (ui.get_server_scroll_offset() - before_grab).abs() < 1.,
                    "{name}: grabbing a rounded thumb shifts the content"
                );
                ui.window().dispatch_event(WindowEvent::PointerReleased {
                    position: middle,
                    button: PointerEventButton::Left,
                });
                ui.window().dispatch_event(WindowEvent::PointerExited);
                ui.invoke_scroll_servers(0.0);
                ui.window().dispatch_event(WindowEvent::PointerScrolled {
                    position: start,
                    delta_x: 0.,
                    delta_y: -44.,
                });
                assert!(
                    ui.get_server_scroll_offset() < 0.,
                    "{name}: mouse wheel over the scrollbar does not scroll"
                );
                ui.invoke_scroll_servers(0.0);
            }
            // A scrollbar disappearing must not change row/text geometry.
            let profiles = ui.get_profiles();
            ui.set_profiles(
                Rc::new(VecModel::from(profiles.iter().take(2).collect::<Vec<_>>())).into(),
            );
            assert_eq!(
                ui.get_server_content_width(),
                content_width,
                "{name}: rows shift when the scrollbar disappears"
            );
            ui.set_profiles(profiles);
        }
        assert!(
            ui.get_navigation_bottom() <= height as f32,
            "{name}: navigation clipped at {} in {height}px window",
            ui.get_navigation_bottom()
        );
        let file = std::fs::File::create(directory.join(format!("{name}.png")))?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let bytes: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        encoder.write_header()?.write_image_data(&bytes)?;
        println!("Rendered {name}: {width}x{height}, {count} profiles");
        if name == "appearance-dropdown" {
            use slint::platform::{PointerEventButton, WindowEvent};
            let position = slint::LogicalPosition::new(8., 600.);
            ui.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerExited);
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if let Some(directory) = args.windows(2).find(|a| a[0] == "--render").map(|a| &a[1]) {
        return render(Path::new(directory));
    }
    let ui = AppWindow::new()?;
    sample(&ui, 12);
    ui.set_animation(true);
    let _animation = mukhomor_ui::visuals::animate(&ui);
    let close = ui.as_weak();
    ui.on_close_window(move || {
        if let Some(ui) = close.upgrade() {
            let _ = ui.hide();
        }
        let _ = slint::quit_event_loop();
    });
    let minimize = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = minimize.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    ui.run()?;
    Ok(())
}
