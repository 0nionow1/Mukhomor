//! Small serialized metadata transactions; transport/profile activation stays in the bridge.
use serde_json::{Value, json};
use std::{collections::HashSet, io::Read, path::Path};

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn printable(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= maximum
        && !value.chars().any(|c| c <= '\u{1f}')
}
pub(super) fn normalize_index(index: &mut Value) -> Result<(), String> {
    let invalid = "Invalid profile library";
    if index["schema"] != 1 {
        return Err(invalid.into());
    }
    let profiles = index["profiles"]
        .as_array()
        .filter(|p| p.len() <= 512)
        .ok_or(invalid)?;
    let mut ids = HashSet::new();
    for profile in profiles {
        let id = profile["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or(invalid)?;
        if !ids.insert(id.to_owned()) || !profile["name"].as_str().is_some_and(|v| printable(v, 80))
        {
            return Err("Invalid profile library entry".into());
        }
        if profile.get("format").is_some_and(|v| {
            !v.as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("conf") || s.eq_ignore_ascii_case("node"))
        }) {
            return Err("Invalid profile storage format".into());
        }
        if profile
            .get("protocol")
            .is_some_and(|v| !v.as_str().is_some_and(|s| printable(s, 40)))
        {
            return Err("Invalid profile protocol label".into());
        }
    }
    let selected = index["selected"].as_str().ok_or(invalid)?.to_owned();
    if !selected.is_empty() && !ids.contains(&selected) {
        return Err("Selected server is missing from the profile library".into());
    }
    if !index["autoconnect"].is_boolean() {
        return Err(invalid.into());
    }
    if index.get("last_connected").is_none() {
        index["last_connected"] = json!(selected);
    }
    let last = index["last_connected"]
        .as_str()
        .ok_or("Invalid last connected server")?;
    if !last.is_empty() && (!valid_id(last) || last.bytes().any(|b| b.is_ascii_uppercase())) {
        return Err("Invalid last connected server".into());
    }
    if !last.is_empty() && !ids.contains(last) {
        index["last_connected"] = json!("");
    }
    Ok(())
}
fn edit(index: &mut Value, request: &Value) -> Result<(), String> {
    normalize_index(index)?;
    match request["action"].as_str() {
        Some("Rename") => {
            let id = request["id"].as_str().ok_or("Server not found")?;
            if index["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| {
                    p["id"]
                        .as_str()
                        .is_some_and(|saved| saved.eq_ignore_ascii_case(id))
                })
                .count()
                != 1
            {
                return Err("Server not found".into());
            }
            let target = index["profiles"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|p| {
                    p["id"]
                        .as_str()
                        .is_some_and(|saved| saved.eq_ignore_ascii_case(id))
                })
                .ok_or("Server not found")?;
            let name = request["name"].as_str().unwrap_or("").trim();
            if !printable(name, 80) {
                return Err("Server name must contain 1 to 80 printable characters".into());
            }
            target["name"] = json!(name);
        }
        Some("Autoconnect") => {
            index["autoconnect"] = json!(
                request["enabled"]
                    .as_bool()
                    .ok_or("Invalid autoconnect value")?
            );
        }
        _ => return Err("Unknown operation".into()),
    }
    Ok(())
}
pub(super) fn call(root: &Path, request: &Value, mut snapshot: Value) -> Result<Value, String> {
    let _root = crate::setup::pin_control_path(root, true, false)?;
    let _private = crate::setup::pin_control_path(&root.join("private"), true, true)?;
    let path = root.join("private/profiles.json");
    let mut index = match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let file = crate::setup::pin_control_path(&path, false, true)?;
            let mut bytes = Vec::new();
            file.take(524289)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 524288 {
                return Err("Invalid profile library".into());
            }
            let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
            crate::traffic_guard_cli::parse(bytes).map_err(|_| "Invalid profile library")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({"schema":1,"selected":"","last_connected":"","autoconnect":false,"profiles":[]})
        }
        Err(error) => return Err(error.to_string()),
    };
    edit(&mut index, request)?;
    let bytes = serde_json::to_vec(&index).map_err(|e| e.to_string())?;
    if bytes.len() > 524288 {
        return Err("Invalid profile library".into());
    }
    super::write_atomic_state(&path, &root.join("private/profiles.json.pending"), &bytes)
        .map_err(|e| e.to_string())?;
    snapshot["profiles"] = json!(index["profiles"].as_array().unwrap().iter().map(|p| {
        json!({"id":p["id"],"name":p["name"],"protocol":p.get("protocol").cloned().unwrap_or(json!("WireGuard / AmneziaWG"))})
    }).collect::<Vec<_>>());
    for key in ["selected", "last_connected", "autoconnect"] {
        snapshot[key] = index[key].clone();
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn index() -> Value {
        json!({"schema":1,"selected":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","autoconnect":false,
            "profiles":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"First","format":"node","protocol":"ss","extra":"preserved"}]})
    }
    #[test]
    fn metadata_edits_preserve_history_extensions_and_unicode_names() {
        let mut data = index();
        edit(&mut data,&json!({"action":"Rename","id":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","name":"  Сервер 🍄  "})).unwrap();
        assert_eq!(data["profiles"][0]["name"], "Сервер 🍄");
        assert_eq!(data["profiles"][0]["extra"], "preserved");
        assert_eq!(data["last_connected"], data["selected"]);
        edit(&mut data, &json!({"action":"Autoconnect","enabled":true})).unwrap();
        assert_eq!(data["autoconnect"], true);
        data["last_connected"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        normalize_index(&mut data).unwrap();
        assert_eq!(data["last_connected"], "");
    }
    #[test]
    fn invalid_names_ids_shapes_and_non_boolean_flags_are_rejected() {
        for name in [
            "".to_string(),
            " \n ".into(),
            "bad\nname".into(),
            "🍄".repeat(41),
            "x".repeat(81),
        ] {
            assert!(
                edit(
                    &mut index(),
                    &json!({"action":"Rename","id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":name})
                )
                .is_err()
            );
        }
        let mut data = index();
        edit(&mut data,&json!({"action":"Rename","id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"🍄".repeat(40)})).unwrap();
        for enabled in [Value::Null, json!("false"), json!(1)] {
            assert!(
                edit(
                    &mut index(),
                    &json!({"action":"Autoconnect","enabled":enabled})
                )
                .is_err()
            );
        }
        for key in ["schema", "selected", "profiles", "autoconnect"] {
            let mut data = index();
            data[key] = Value::Null;
            assert!(normalize_index(&mut data).is_err());
        }
        let mut data = index();
        data["profiles"]
            .as_array_mut()
            .unwrap()
            .push(index()["profiles"][0].clone());
        assert!(normalize_index(&mut data).is_err());
        let mut data = index();
        data["profiles"][0]["id"] = json!("../outside");
        assert!(normalize_index(&mut data).is_err());
    }

    #[test]
    fn disk_transactions_keep_private_fields_out_of_snapshots_and_fail_atomically() {
        let tree = crate::setup::cleanup_request::fixtures::Tree::new();
        let private = tree.root.join("private");
        std::fs::create_dir(&private).unwrap();
        let path = private.join("profiles.json");
        std::fs::write(&path, index().to_string()).unwrap();
        let snapshot = json!({"running":true,"settings":{"preserved":true},"traffic_guard":{"protected":true}});
        let request =
            json!({"action":"Rename","id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"Updated"});
        let result = call(&tree.root, &request, snapshot.clone()).unwrap();
        assert_eq!(result["profiles"][0]["name"], "Updated");
        assert!(result["profiles"][0].get("extra").is_none());
        assert_eq!(result["running"], true);
        assert_eq!(result["traffic_guard"], snapshot["traffic_guard"]);
        let before = std::fs::read(&path).unwrap();
        let saved: Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(saved["profiles"][0]["extra"], "preserved");
        std::fs::create_dir(private.join("profiles.json.pending")).unwrap();
        assert!(
            call(
                &tree.root,
                &json!({"action":"Autoconnect","enabled":true}),
                snapshot
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn metadata_rejects_hardlinks_and_readable_private_state_before_mutation() {
        use crate::setup::cleanup_request::fixtures::{Tree, set_acl};
        let tree = Tree::new();
        let private = tree.root.join("private");
        std::fs::create_dir(&private).unwrap();
        let outside = tree.root.join("outside.json");
        let path = private.join("profiles.json");
        let original = index().to_string();
        std::fs::write(&outside, &original).unwrap();
        std::fs::hard_link(&outside, &path).unwrap();
        let request = json!({"action":"Autoconnect","enabled":true});
        assert!(call(&tree.root, &request, json!({})).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, &original).unwrap();
        set_acl(
            &private,
            &format!(
                "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})(A;OICI;FR;;;BU)",
                tree.owner
            ),
        );
        assert!(call(&tree.root, &request, json!({})).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn directory_commit_pin_blocks_parent_replacement_and_checks_existing_writers() {
        use crate::setup::cleanup_request::fixtures::Tree;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        let tree = Tree::new();
        let private = tree.root.join("private");
        std::fs::create_dir(&private).unwrap();
        let pin = crate::setup::pin_control_path(&private, true, true).unwrap();
        assert!(std::fs::rename(&private, tree.root.join("replaced")).is_err());
        drop(pin);
        let writer = std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&private)
            .unwrap();
        assert!(crate::setup::pin_control_path(&private, true, true).is_err());
        drop(writer);
        assert!(crate::setup::pin_control_path(&private, true, true).is_ok());
    }
}
