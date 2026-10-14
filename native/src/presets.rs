//! The editable preset model. `direct` always contains standalone rules only.
use serde_json::{Value, json};

// The 4 MiB IPC frame also carries diagnostics and up to 512 server summaries.
pub const MAX_SETTINGS_BYTES: usize = 2 * 1024 * 1024;
pub const SIZE_ERROR: &str =
    "Settings exceed 2 MiB; reduce rule list size. Your draft has been kept.";
pub fn fits_transport(settings: &Value) -> bool {
    serde_json::to_string(settings).is_ok_and(|text| {
        // Windows PowerShell 5.1 uses Newtonsoft's HTML-safe JSON escaping.
        let escape_overhead: usize = text
            .chars()
            .map(|c| match c {
                '\'' | '<' | '>' | '&' => 5,
                '\u{2028}' | '\u{2029}' => 3,
                '\u{85}' => 4,
                _ => 0,
            })
            .sum();
        text.len() + escape_overhead <= MAX_SETTINGS_BYTES
    })
}

pub const SECTIONS: [&str; 6] = [
    "process_names",
    "process_paths",
    "domains",
    "domain_suffixes",
    "ip_cidrs",
    "rule_set_exclusions",
];
pub const DOMAIN_SOURCES: [(i32, &str); 4] = [
    (3108, "russia"),
    (3109, "steam"),
    (3110, "ozon"),
    (3112, "games"),
];
pub fn defaults() -> Value {
    serde_json::from_str(include_str!("../../assets/settings.default.json"))
        .expect("bundled settings")
}
pub fn normalize(settings: &mut Value) {
    if settings["schema"] != 1 {
        return;
    }
    for key in ["domain_rule_sets", "rule_set_exclusions"] {
        if !settings["direct"][key].is_array() {
            settings["direct"][key] = json!([]);
        }
    }
    let mut presets = defaults()["presets"].as_array().unwrap().clone();
    for preset in &mut presets {
        for key in SECTIONS.into_iter().chain(["domain_rule_sets"]) {
            let baseline = if key == "domain_rule_sets" {
                preset[key].clone()
            } else {
                preset["rules"][key].clone()
            };
            let saved = settings["direct"][key]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let matches =
                |value: &Value| {
                    baseline.as_array().unwrap().iter().any(|b| {
                        match (value.as_str(), b.as_str()) {
                            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                            _ => false,
                        }
                    })
                };
            let moved: Vec<_> = saved.iter().filter(|v| matches(v)).cloned().collect();
            settings["direct"][key] = json!(
                saved
                    .into_iter()
                    .filter(|v| !matches(v))
                    .collect::<Vec<_>>()
            );
            if key == "domain_rule_sets" {
                preset[key] = json!(moved);
            } else {
                preset["rules"][key] = json!(moved);
            }
        }
        let visible: Vec<_> = SECTIONS
            .into_iter()
            .filter(|key| !preset["rules"][key].as_array().unwrap().is_empty())
            .collect();
        preset["enabled"] = json!(
            !visible.is_empty() || !preset["domain_rule_sets"].as_array().unwrap().is_empty()
        );
        preset["visible_sections"] = json!(visible);
    }
    settings["presets"] = json!(presets);
    settings["schema"] = json!(2);
}
pub fn index(settings: &Value, id: &str) -> Option<usize> {
    settings["presets"]
        .as_array()?
        .iter()
        .position(|p| p["id"] == id)
}
pub fn create(settings: &mut Value, id: &str, name: &str) -> bool {
    if index(settings, id).is_some()
        || settings["presets"]
            .as_array()
            .is_none_or(|p| p.len() >= 128)
    {
        return false;
    }
    let mut rules = json!({});
    for key in SECTIONS {
        rules[key] = json!([]);
    }
    settings["presets"].as_array_mut().unwrap().push(json!({"id":id,"name":name,"description":"","enabled":true,"rules":rules,"domain_rule_sets":[],"visible_sections":SECTIONS,"disabled_sections":[]}));
    true
}
/// Add a fresh copy without changing existing presets or personal rules.
pub fn add_template(settings: &mut Value, id: &str, builtin_id: &str) -> bool {
    let baseline = defaults();
    let Some(original) = baseline["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["builtin_id"] == builtin_id)
    else {
        return false;
    };
    if !create(settings, id, original["name"].as_str().unwrap_or("")) {
        return false;
    }
    let i = index(settings, id).unwrap();
    settings["presets"][i] = original.clone();
    settings["presets"][i]["id"] = json!(id);
    settings["presets"][i]["enabled"] = json!(true);
    settings["presets"][i]["visible_sections"] = json!(SECTIONS);
    settings["presets"][i]["disabled_sections"] = json!([]);
    true
}
/// Portable presets contain rules only, never server profiles or machine state.
pub fn export(settings: &Value, id: &str) -> Result<Vec<u8>, String> {
    let i = index(settings, id).ok_or("Choose a preset first")?;
    let p = &settings["presets"][i];
    let mut rules = json!({});
    for key in SECTIONS {
        rules[key] = p["rules"][key].clone();
    }
    serde_json::to_vec_pretty(&json!({"format":"mukhomor-preset","version":1,"preset":{
        "name":p["name"],"description":p["description"],"rules":rules,
        "domain_rule_sets":p["domain_rule_sets"],"disabled_sections":p["disabled_sections"]
    }}))
    .map_err(|e| e.to_string())
}
pub fn import(settings: &mut Value, id: &str, bytes: &[u8]) -> Result<(), String> {
    let invalid = "Invalid Mukhomor preset file";
    if bytes.len() > MAX_SETTINGS_BYTES {
        return Err(SIZE_ERROR.into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let package: Value = serde_json::from_slice(bytes).map_err(|_| invalid)?;
    if package["format"] != "mukhomor-preset" || package["version"] != 1 {
        return Err(invalid.into());
    }
    let p = &package["preset"];
    let name = p["name"]
        .as_str()
        .filter(|s| {
            !s.trim().is_empty()
                && s.encode_utf16().count() <= 120
                && !s.chars().any(char::is_control)
        })
        .ok_or(invalid)?;
    let description = p["description"]
        .as_str()
        .filter(|s| s.encode_utf16().count() <= 1000)
        .ok_or(invalid)?;
    let mut rules = json!({});
    for key in SECTIONS {
        let values = p["rules"][key]
            .as_array()
            .filter(|a| a.len() <= 5000)
            .ok_or(invalid)?;
        if values
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.encode_utf16().count() > 1024))
        {
            return Err(invalid.into());
        }
        rules[key] = json!(values);
    }
    let sources = p["domain_rule_sets"]
        .as_array()
        .filter(|a| a.len() <= 32)
        .ok_or(invalid)?;
    if sources
        .iter()
        .any(|v| v.as_str().is_none_or(|s| s.encode_utf16().count() > 120))
    {
        return Err(invalid.into());
    }
    let disabled = p["disabled_sections"]
        .as_array()
        .filter(|a| a.len() <= 6)
        .ok_or(invalid)?;
    if disabled
        .iter()
        .any(|v| v.as_str().is_none_or(|s| !SECTIONS.contains(&s)))
    {
        return Err(invalid.into());
    }
    let mut next = settings.clone();
    if !create(&mut next, id, name) {
        return Err("Cannot add more than 128 presets".into());
    }
    let i = index(&next, id).unwrap();
    next["presets"][i]["description"] = json!(description);
    next["presets"][i]["rules"] = rules;
    next["presets"][i]["domain_rule_sets"] = json!(sources);
    next["presets"][i]["disabled_sections"] = json!(disabled);
    // The receiving user enables it explicitly on the presets page.
    next["presets"][i]["enabled"] = json!(false);
    let count: usize = SECTIONS
        .iter()
        .map(|key| {
            next["direct"][key].as_array().map_or(0, Vec::len)
                + next["presets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p["rules"][key].as_array().map_or(0, Vec::len))
                    .sum::<usize>()
        })
        .sum();
    if count > 30000 || !fits_transport(&next) {
        return Err(SIZE_ERROR.into());
    }
    *settings = next;
    Ok(())
}
pub fn remove(settings: &mut Value, id: &str) -> bool {
    let Some(i) = index(settings, id) else {
        return false;
    };
    settings["presets"].as_array_mut().unwrap().remove(i);
    true
}
pub fn membership(preset: &mut Value, field: &str, section: &str, present: bool) {
    let mut list = preset[field].as_array().cloned().unwrap_or_default();
    list.retain(|v| v != section);
    if present {
        list.push(json!(section));
    }
    preset[field] = json!(list);
}
pub fn contains(preset: &Value, field: &str, section: &str) -> bool {
    preset[field]
        .as_array()
        .is_some_and(|v| v.iter().any(|v| v == section))
}
pub fn restore(settings: &mut Value, id: &str) -> bool {
    let Some(i) = index(settings, id) else {
        return false;
    };
    let baseline = defaults();
    let Some(original) = baseline["presets"].as_array().unwrap().iter().find(|p| {
        p["builtin_id"] == settings["presets"][i]["builtin_id"] && p["builtin_id"].is_string()
    }) else {
        return false;
    };
    settings["presets"][i]["rules"] = original["rules"].clone();
    settings["presets"][i]["domain_rule_sets"] = original["domain_rule_sets"].clone();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_presets_roundtrip_without_profiles_identity_or_activation() {
        let mut s = defaults();
        s["private"] = json!({"server":"SECRET"});
        let bytes = export(&s, "builtin-games").unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            !text.contains("SECRET")
                && !text.contains("builtin_id")
                && !text.contains("visible_sections")
        );
        let before = s.clone();
        import(&mut s, "friend", &bytes).unwrap();
        let i = index(&s, "friend").unwrap();
        assert_eq!(s["presets"][i]["enabled"], false);
        assert_eq!(s["presets"][i]["rules"], before["presets"][0]["rules"]);
        assert_eq!(
            s["presets"][i]["description"],
            before["presets"][0]["description"]
        );
        assert!(s["presets"][i].get("builtin_id").is_none());
        assert_eq!(s["direct"], before["direct"]);
        let imported = s.clone();
        assert!(import(&mut s, "friend", &bytes).is_err());
        assert!(
            import(
                &mut s,
                "bad",
                br#"{"format":"mukhomor-preset","version":2}"#
            )
            .is_err()
        );
        let mut malformed: Value = serde_json::from_slice(&bytes).unwrap();
        malformed["preset"]["rules"]["domains"] = json!([123]);
        assert!(import(&mut s, "bad", &serde_json::to_vec(&malformed).unwrap()).is_err());
        assert!(import(&mut s, "bad", &vec![0; MAX_SETTINGS_BYTES + 1]).is_err());
        assert_eq!(s, imported);
    }
    #[test]
    fn adding_template_does_not_reset_existing_rules_or_identity() {
        let mut s = defaults();
        s["direct"]["domains"] = json!(["personal.example"]);
        let before = s.clone();
        assert!(add_template(&mut s, "new-games", "games"));
        assert_eq!(s["direct"], before["direct"]);
        for (i, p) in before["presets"].as_array().unwrap().iter().enumerate() {
            assert_eq!(&s["presets"][i], p);
        }
        let i = index(&s, "new-games").unwrap();
        assert_eq!(s["presets"][i]["builtin_id"], "games");
        assert_eq!(s["presets"][i]["visible_sections"], json!(SECTIONS));
        assert_eq!(s["presets"][i]["enabled"], true);
        assert!(!add_template(&mut s, "new-games", "games"));
        assert!(!add_template(&mut s, "unknown", "unknown"));
        assert!(remove(&mut s, "new-games"));
        assert_eq!(s, before);
    }
    #[test]
    fn settings_budget_counts_utf8_and_json_escaping_at_the_boundary() {
        let exact = json!("a".repeat(MAX_SETTINGS_BYTES - 2));
        assert!(fits_transport(&exact));
        assert!(!fits_transport(&json!("a".repeat(MAX_SETTINGS_BYTES - 1))));
        assert!(!fits_transport(&json!(
            "\u{1f600}".repeat(MAX_SETTINGS_BYTES / 4)
        )));
        assert!(!fits_transport(&json!("\n".repeat(MAX_SETTINGS_BYTES / 2))));
        let html_boundary = (MAX_SETTINGS_BYTES - 2) / 6;
        for c in ["'", "<", ">", "&", "\u{2028}", "\u{2029}"] {
            assert!(fits_transport(&json!(c.repeat(html_boundary))));
            assert!(!fits_transport(&json!(c.repeat(html_boundary + 1))));
        }
        assert!(fits_transport(&json!("\u{85}".repeat(html_boundary))));
        assert!(!fits_transport(&json!("\u{85}".repeat(html_boundary + 1))));
    }
    #[test]
    fn migration_preserves_deletions_and_ownership_once() {
        let mut s = defaults();
        s["schema"] = json!(1);
        s.as_object_mut().unwrap().remove("presets");
        s["direct"]["process_names"] = json!(["cs2.exe", "custom.exe", "qbittorrent.exe"]);
        s["direct"]["domain_rule_sets"] = json!(["steam"]);
        normalize(&mut s);
        assert_eq!(s["direct"]["process_names"], json!(["custom.exe"]));
        let games = index(&s, "builtin-games").unwrap();
        assert_eq!(
            s["presets"][games]["rules"]["process_names"],
            json!(["cs2.exe"])
        );
        assert_eq!(
            s["presets"][index(&s, "builtin-russia").unwrap()]["enabled"],
            false
        );
        let once = s.clone();
        normalize(&mut s);
        assert_eq!(s, once);
    }
    #[test]
    fn restore_keeps_identity_and_user_preferences() {
        let mut s = defaults();
        let i = index(&s, "builtin-games").unwrap();
        s["presets"][i]["name"] = json!("My games");
        s["presets"][i]["enabled"] = json!(false);
        s["presets"][i]["visible_sections"] = json!([]);
        s["presets"][i]["rules"]["process_names"] = json!([]);
        assert!(restore(&mut s, "builtin-games"));
        assert_eq!(s["presets"][i]["name"], "My games");
        assert_eq!(s["presets"][i]["enabled"], false);
        assert_eq!(s["presets"][i]["visible_sections"], json!([]));
        assert!(
            !s["presets"][i]["rules"]["process_names"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        remove(&mut s, "builtin-games");
        normalize(&mut s);
        assert!(index(&s, "builtin-games").is_none());
    }
    #[test]
    fn all_sections_are_independent_from_visibility() {
        let mut s = defaults();
        assert!(create(&mut s, "custom", "Custom"));
        assert!(!create(&mut s, "custom", "Copy"));
        let p = &mut s["presets"][index(&defaults(), "builtin-games").unwrap()];
        for section in SECTIONS {
            p["rules"][section] = json!(["entry"]);
            membership(p, "visible_sections", section, true);
        }
        membership(p, "visible_sections", "domains", false);
        assert_eq!(p["rules"]["domains"], json!(["entry"]));
        assert_eq!(p["enabled"], true);
        assert_eq!(p["visible_sections"].as_array().unwrap().len(), 5);
    }
}
