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
    ui.set_status_text("Не подключено".into());
    ui.set_primary_text("Подключиться".into());
    ui.set_phase("idle".into());
    ui.set_ready(true);
    ui.set_busy(false);
    ui.set_loaded(true);
    ui.set_autostart_available(true);
    ui.set_autoconnect(true);
    ui.set_autostart(false);
    ui.set_chosen_preset_id("".into());
    ui.set_routing_editable(true);
    ui.set_rounding(true);
    ui.set_animation(true);
    ui.set_primary_enabled(count > 0);
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
    ui.set_selected_name("Сервер 01".into());
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
        ("startup-opening", 0, 560, 740, 1., 0, 0),
        ("startup-growing", 0, 560, 740, 1., 0, 0),
        ("startup-mobile", 0, 360, 640, 1., 0, 0),
        ("startup-error", 0, 360, 580, 1., 0, 0),
        ("startup-reduced-motion", 0, 560, 740, 1., 0, 0),
        ("connection-reference", 12, 560, 740, 1., 0, 0),
        ("connection-paper", 12, 560, 740, 1., 0, 0),
        ("connection-glacier", 12, 560, 740, 1., 0, 0),
        ("connection-empty", 0, 560, 740, 1., 0, 0),
        ("connection-short", 2, 560, 740, 1., 0, 0),
        ("connection-long", 512, 560, 740, 1., 0, 0),
        ("connection-bottom", 512, 560, 740, 1., 0, 0),
        ("connection-scrolled-selection", 512, 560, 740, 1., 0, 0),
        ("connection-compact", 512, 360, 640, 1., 0, 0),
        ("connection-minimum", 512, 360, 580, 1., 0, 0),
        ("connection-small", 512, 560, 740, 0.85, 0, 0),
        ("connection-large", 512, 620, 900, 1.25, 0, 0),
        ("import", 2, 560, 740, 1., 0, 1),
        ("import-compact", 2, 360, 640, 1., 0, 1),
        ("rename-compact", 2, 360, 640, 1., 0, 2),
        ("rules", 2, 560, 740, 1., 1, 0),
        ("rules-compact", 2, 360, 640, 1., 1, 0),
        ("rules-dirty-compact", 2, 360, 640, 1., 1, 0),
        ("presets", 2, 560, 740, 1., 2, 0),
        ("presets-paper", 2, 560, 740, 1., 2, 0),
        ("presets-focus", 2, 560, 740, 1., 2, 0),
        ("presets-compact", 2, 360, 640, 1., 2, 0),
        ("presets-long", 2, 560, 740, 1., 2, 0),
        ("preset-editor", 2, 560, 740, 1., 2, 4),
        ("settings", 2, 560, 740, 1., 3, 0),
        ("settings-paper", 2, 560, 740, 1., 3, 0),
        ("settings-disabled", 2, 560, 740, 1., 3, 0),
        ("settings-compact", 2, 360, 640, 1., 3, 0),
        ("appearance", 2, 560, 740, 1., 3, 0),
        ("appearance-dropdown", 2, 560, 740, 1., 3, 0),
        ("dirty-exit", 2, 560, 740, 1., 1, 9),
        ("dirty-exit-compact", 2, 360, 640, 1., 1, 9),
    ] {
        sample(&ui, count);
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
            ui.set_primary_text("Отключиться".into());
            ui.set_phase("connected".into());
        }
        if name.ends_with("paper") {
            mukhomor_ui::visuals::apply(&ui, mukhomor_ui::visuals::appearance::Theme::Paper, true);
        } else if name == "connection-glacier" {
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
        ui.set_appearance_open(name.starts_with("appearance"));
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
        surface.set_size(slint::PhysicalSize::new(width, height));
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
        if page == 0 {
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
