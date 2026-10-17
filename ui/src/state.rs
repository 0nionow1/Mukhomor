//! Controller receipts and editable state, independent of the UI event loop.
use serde_json::{Value, json};

pub fn is_p2p(network: &Value) -> bool {
    matches!(network.as_str(), Some("tor" | "i2p"))
}

#[derive(Clone, Debug)]
pub struct Pending {
    pub id: String,
    pub action: String,
    pub generation: u64,
    pub network: String,
    pub profile_id: String,
    pub reconnecting: bool,
    draft_version: u64,
}

#[derive(Default, Debug)]
pub struct State {
    pub snapshot: Value,
    pub draft: Value,
    pub pending: Option<Pending>,
    pub revision: Option<String>,
    pub generation: u64,
    pub loaded: bool,
    pub transport_error: Option<String>,
    pub connection_failed: bool,
    draft_version: u64,
    dirty: bool,
}

#[derive(Debug, PartialEq)]
pub enum Receipt {
    Ignored,
    Unchanged,
    Updated,
    Completed {
        action: String,
        ok: bool,
        error: String,
    },
}

/// One launch, completed only by a usable controller snapshot. Animation time
/// never changes VPN state, and later polling failures do not replay the intro.
#[derive(Default, Debug)]
pub struct Startup {
    pub complete: bool,
    pub error: Option<String>,
}
impl Startup {
    pub fn observe(&mut self, state: &State, timed_out: bool) {
        if self.complete { return; }
        if state.available() {
            self.complete = true;
            self.error = None;
        } else {
            self.error = state.transport_error.clone().or_else(|| {
                state.snapshot["last_error"].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
            }).or_else(|| timed_out.then(|| "Подготовка занимает больше времени. Можно повторить проверку или открыть приложение.".into()));
        }
    }
}

impl State {
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.snapshot["busy"] == true
    }

    pub fn available(&self) -> bool {
        self.loaded && self.transport_error.is_none() && self.snapshot["service_ready"] == true
            && self.snapshot["initializing"] != true
    }

    pub fn can_disconnect(&self) -> bool {
        self.snapshot["can_disconnect"] == true
            || self.snapshot["running"] == true
            || self.snapshot["starting"] == true
            || self.snapshot["dns_recovery_pending"] == true
            || self.pending.as_ref().is_some_and(|p| p.action == "Connect" || p.reconnecting)
    }

    pub fn phase(&self) -> &str {
        // A lost receipt must not keep claiming an unconfirmed transition.
        if self.loaded && (self.transport_error.is_some() || self.snapshot["status_uncertain"] == true) {
            return "uncertain";
        }
        if let Some(pending) = &self.pending {
            match pending.action.as_str() {
                "Disconnect" | "Exit" => return "disconnecting",
                "Connect" => return "connecting",
                "Select" if pending.reconnecting => return "switching",
                _ => {}
            }
        }
        if !self.loaded || self.snapshot["initializing"] == true {
            return "loading";
        }
        if self.transport_error.is_some() || self.snapshot["status_uncertain"] == true {
            "uncertain"
        } else if self.snapshot["dns_recovery_pending"] == true {
            "recovery"
        } else if self.snapshot["starting"] == true {
            "connecting"
        } else if self.snapshot["running"] == true {
            "connected"
        } else if !self.loaded {
            "loading"
        } else {
            self.snapshot["phase"].as_str().unwrap_or("idle")
        }
    }

    pub fn selected(&self) -> &str {
        self.snapshot["selected"].as_str().unwrap_or("")
    }

    pub fn connection_network(&self) -> &str {
        self.pending.as_ref().filter(|p| p.action == "Connect")
            .map(|p| p.network.as_str())
            .unwrap_or_else(|| self.snapshot["network"].as_str().unwrap_or("vps"))
    }

    pub fn connection_name(&self) -> &str {
        match self.connection_network() {
            "tor" => return "Tor",
            "i2p" => return "I2P",
            _ => {}
        }
        let id = self.pending.as_ref()
            .filter(|p| matches!(p.action.as_str(), "Connect" | "Select"))
            .map(|p| p.profile_id.as_str())
            .unwrap_or_else(|| if self.snapshot["running"] == true {
                self.snapshot["last_connected"].as_str().filter(|s| !s.is_empty()).unwrap_or(self.selected())
            } else { self.selected() });
        self.snapshot["profiles"].as_array()
            .and_then(|profiles| profiles.iter().find(|p| p["id"] == id))
            .and_then(|p| p["name"].as_str()).unwrap_or("")
    }

    pub fn dirty(&self) -> bool {
        self.loaded && self.dirty
    }

    pub fn draft_revision(&self) -> u64 { self.draft_version }

    pub fn edit_rules(&mut self, preset: Option<usize>, section: &str, values: Value) {
        let target = match preset {
            Some(index) => &mut self.draft["presets"][index]["rules"][section],
            None => &mut self.draft["direct"][section],
        };
        if *target != values {
            *target = values;
            self.draft_version += 1;
            self.dirty = self.draft != self.snapshot["settings"];
        }
    }

    pub fn edit(&mut self, draft: Value) {
        if self.draft != draft {
            self.draft = draft;
            self.draft_version += 1;
            self.dirty = self.draft != self.snapshot["settings"];
        }
    }

    pub fn discard(&mut self) {
        self.edit(self.snapshot["settings"].clone());
    }

    pub fn begin(&mut self, mut request: Value, nonce: &str) -> Option<Value> {
        let action = request["action"].as_str()?.to_owned();
        if action == "Status" || (self.busy() && !matches!(action.as_str(), "Disconnect" | "Exit"))
        {
            return None;
        }
        self.generation += 1;
        if matches!(action.as_str(), "Connect" | "Select" | "Disconnect" | "Exit") {
            self.connection_failed = false;
        }
        let id = format!("{nonce}-{}", self.generation);
        request["request_id"] = json!(id);
        self.pending = Some(Pending {
            reconnecting: action == "Select" && self.snapshot["running"] == true,
            network: request["network"].as_str().unwrap_or("vps").into(),
            profile_id: request["id"].as_str().unwrap_or(self.selected()).into(),
            id,
            action,
            generation: self.generation,
            draft_version: self.draft_version,
        });
        self.revision = None;
        Some(request)
    }

    pub fn status_request(&self) -> Value {
        let mut request = json!({"action":"Status"});
        if let Some(revision) = &self.revision {
            request["if_revision"] = json!(revision);
        }
        request
    }

    /// A lost mutation receipt is ambiguous. Keep it pending and reconcile by ID;
    /// never repeat Import/Connect/Disconnect merely because transport failed.
    pub fn receive(
        &mut self,
        generation: u64,
        action: &str,
        reply: Result<Value, String>,
    ) -> Receipt {
        if generation != self.generation {
            return Receipt::Ignored;
        }
        let mut reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                self.transport_error = Some(error);
                self.revision = None;
                return Receipt::Updated;
            }
        };
        if action != "Status"
            && self.pending.as_ref().is_some_and(|pending| {
                reply["request_id"]
                    .as_str()
                    .is_some_and(|id| id != pending.id)
            })
        {
            self.transport_error = Some("Ответ относится к другой операции".into());
            self.revision = None;
            return Receipt::Updated;
        }
        if reply["not_modified"] == true {
            if reply["ok"] != true
                || reply["revision"].as_str() != self.revision.as_deref()
                || !self.loaded
            {
                self.revision = None;
                self.transport_error = Some("Некорректный ответ контроллера".into());
            } else {
                if self.transport_error.take().is_none() {
                    return Receipt::Unchanged;
                }
            }
            return Receipt::Updated;
        }
        let completion = self.pending.as_ref().and_then(|pending| {
            let last = &reply["data"]["last_operation"];
            if action == "Status" {
                (last["request_id"].as_str() == Some(pending.id.as_str())).then(|| last.clone())
            } else {
                // NativeBridge authenticates this single request/response channel.
                (action == pending.action && reply["ok"].is_boolean())
                    .then(|| json!({"ok":reply["ok"],"error":reply["error"]}))
            }
        });
        let valid_snapshot = reply["data"].is_object()
            && reply["data"]["profiles"].is_array()
            && reply["data"]["settings"].is_object();
        if valid_snapshot {
            let mut snapshot = reply["data"].take();
            if snapshot.get("rule_issues").is_none()
                && snapshot["settings"] == self.snapshot["settings"]
            {
                snapshot["rule_issues"] = self.snapshot["rule_issues"].clone();
                snapshot["rule_issues_total"] = self.snapshot["rule_issues_total"].clone();
            }
            let saved = self.pending.as_ref().is_some_and(|p| {
                p.action == "ApplySettings" && p.draft_version == self.draft_version
            }) && completion.as_ref().is_some_and(|c| c["ok"] == true);
            if (!self.loaded || saved || !self.dirty) && self.draft != snapshot["settings"] {
                self.draft = snapshot["settings"].clone();
                self.draft_version += 1;
            }
            self.dirty = self.draft != snapshot["settings"];
            self.snapshot = snapshot;
            self.loaded = true;
            self.revision = reply["revision"].as_str().map(str::to_owned);
            self.transport_error = None;
        } else if action == "Status" {
            self.transport_error = Some(
                reply["error"]
                    .as_str()
                    .unwrap_or("Некорректный ответ контроллера")
                    .into(),
            );
            self.revision = None;
        }
        if let Some(completion) = completion {
            let pending = self.pending.take().unwrap();
            if matches!(pending.action.as_str(), "Connect" | "Select") {
                self.connection_failed = completion["ok"] != true;
            }
            return Receipt::Completed {
                action: pending.action,
                ok: completion["ok"] == true,
                error: completion["error"].as_str().unwrap_or("").into(),
            };
        }
        Receipt::Updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(settings: Value) -> Value {
        json!({"ok":true,"revision":"epoch:1","data":{"profiles":[],"settings":settings,"service_ready":true}})
    }

    #[test]
    fn pending_target_and_switching_survive_intermediate_snapshots() {
        let mut s = State::default();
        let mut reply = snapshot(json!({}));
        reply["data"]["profiles"] = json!([{"id":"a","name":"Alpha"},{"id":"b","name":"Beta"}]);
        reply["data"]["selected"] = json!("a");
        reply["data"]["last_connected"] = json!("a");
        reply["data"]["running"] = json!(true);
        s.receive(0, "Status", Ok(reply.clone()));
        s.begin(json!({"action":"Select","id":"b"}), "test").unwrap();
        assert_eq!(s.phase(), "switching");
        assert_eq!(s.connection_name(), "Beta");
        reply["data"]["running"] = json!(false);
        s.receive(s.generation, "Status", Ok(reply));
        assert_eq!(s.phase(), "switching");
        assert!(s.can_disconnect());
        s.begin(json!({"action":"Disconnect"}), "test").unwrap();
        assert_eq!(s.phase(), "disconnecting");
    }

    #[test]
    fn connecting_p2p_uses_request_target_and_lost_receipt_is_uncertain() {
        for network in ["tor", "i2p"] {
            let mut s = State::default();
            s.receive(0, "Status", Ok(snapshot(json!({}))));
            s.begin(json!({"action":"Connect","network":network}), "test").unwrap();
            assert_eq!(s.connection_network(), network);
            assert_eq!(s.connection_name(), if network == "tor" { "Tor" } else { "I2P" });
            assert_eq!(s.phase(), "connecting");
            s.receive(s.generation, "Connect", Err("lost reply".into()));
            assert_eq!(s.phase(), "uncertain");
            assert!(s.can_disconnect());
            assert!(s.pending.is_some());
        }
    }

    #[test]
    fn connection_failure_remains_visible_after_polling_until_retry() {
        let mut s = State::default();
        let reply = snapshot(json!({}));
        s.receive(0, "Status", Ok(reply.clone()));
        let request = s.begin(json!({"action":"Connect"}), "test").unwrap();
        s.receive(s.generation, "Connect", Ok(json!({"ok":false,"request_id":request["request_id"],"error":"offline","data":reply["data"]})));
        s.receive(s.generation, "Status", Ok(reply));
        assert!(s.connection_failed);
        s.begin(json!({"action":"Connect"}), "test").unwrap();
        assert!(!s.connection_failed);
    }

    #[test]
    fn startup_requires_ready_snapshot_and_never_replays_after_poll_failure() {
        let mut state = State::default();
        let mut startup = Startup::default();
        startup.observe(&state, false);
        assert!(!startup.complete);
        let mut preview = snapshot(json!({}));
        preview["data"]["initializing"] = json!(true);
        preview["data"]["service_ready"] = json!(false);
        state.receive(0, "Status", Ok(preview));
        startup.observe(&state, true);
        assert!(!startup.complete, "a timeout cannot declare VPN ready");
        assert!(startup.error.is_some());
        state.receive(0, "Status", Err("offline".into()));
        startup.observe(&state, false);
        assert_eq!(startup.error.as_deref(), Some("offline"));
        state.receive(0, "Status", Ok(snapshot(json!({}))));
        startup.observe(&state, false);
        assert!(startup.complete);
        assert!(startup.error.is_none());
        assert!(!state.can_disconnect(), "preparation must not connect the VPN");
        state.receive(0, "Status", Err("later failure".into()));
        startup.observe(&state, false);
        assert!(startup.complete);
    }
    #[test]
    fn metadata_preview_is_visible_without_claiming_readiness() {
        let mut s = State::default();
        let mut answer = snapshot(json!({"direct":{"domains":[]}}));
        answer["ok"] = json!(false);
        answer["data"]["initializing"] = json!(true);
        answer["data"]["service_ready"] = json!(false);
        answer["data"]["profiles"] = json!([{"id":"a","name":"Saved server"}]);
        s.receive(0, "Status", Ok(answer));
        assert_eq!(s.snapshot["profiles"][0]["name"], "Saved server");
        assert_eq!(s.phase(), "loading");
        assert!(!s.available());
        assert!(s.transport_error.is_none());
    }
    #[test]
    fn unchanged_status_preserves_large_draft_without_refreshing_it() {
        let mut s = State::default();
        s.receive(0, "Status", Ok(snapshot(json!({"direct":{"domains":vec!["example.invalid";5000]}}))));
        s.edit_rules(None, "domains", json!(["edited.invalid"]));
        let version = s.draft_revision();
        let pointer = s.draft["direct"]["domains"].as_array().unwrap().as_ptr();
        for _ in 0..100 {
            assert_eq!(s.receive(0, "Status", Ok(json!({"ok":true,"not_modified":true,"revision":"epoch:1"}))), Receipt::Unchanged);
        }
        assert_eq!(s.draft_revision(), version);
        assert_eq!(s.draft["direct"]["domains"].as_array().unwrap().as_ptr(), pointer);
        assert!(s.dirty());
        s.receive(0, "Status", Err("Temporarily unavailable".into()));
        s.receive(0, "Status", Ok(snapshot(json!({"direct":{"domains":[]}}))));
        assert!(s.transport_error.is_none());
        assert_eq!(s.draft["direct"]["domains"], json!(["edited.invalid"]));
    }
    #[test]
    fn polling_and_old_save_receipts_do_not_erase_newer_edits() {
        let mut s = State::default();
        s.receive(0, "Status", Ok(snapshot(json!({"value":1}))));
        s.edit(json!({"value":2}));
        s.receive(0, "Status", Ok(snapshot(json!({"value":1}))));
        assert_eq!(s.draft["value"], 2);
        s.begin(json!({"action":"ApplySettings","settings":s.draft}), "test")
            .unwrap();
        s.edit(json!({"value":3}));
        s.receive(1, "ApplySettings", Ok(snapshot(json!({"value":2}))));
        assert_eq!(s.draft["value"], 3);
        assert!(s.dirty());
    }
    #[test]
    fn cancellation_supersedes_connect_and_old_status() {
        let mut s = State::default();
        s.begin(json!({"action":"Connect"}), "test").unwrap();
        assert!(s.can_disconnect());
        s.begin(json!({"action":"Disconnect"}), "test").unwrap();
        assert_eq!(
            s.receive(1, "Connect", Ok(snapshot(json!({})))),
            Receipt::Ignored
        );
        assert_eq!(
            s.receive(0, "Status", Ok(snapshot(json!({})))),
            Receipt::Ignored
        );
        assert_eq!(s.phase(), "disconnecting");
    }
    #[test]
    fn lost_import_receipt_is_reconciled_once_without_resubmitting() {
        let mut s = State::default();
        s.begin(json!({"action":"Import"}), "test").unwrap();
        s.receive(1, "Import", Err("Disconnected".into()));
        assert!(s.busy());
        assert!(s.begin(json!({"action":"Import"}), "test").is_none());
        let mut answer = snapshot(json!({}));
        answer["data"]["last_operation"] = json!({"request_id":"test-1","ok":true});
        assert_eq!(
            s.receive(1, "Status", Ok(answer.clone())),
            Receipt::Completed {
                action: "Import".into(),
                ok: true,
                error: String::new()
            }
        );
        assert_eq!(s.receive(1, "Status", Ok(answer)), Receipt::Updated);
        assert!(!s.busy());
    }
    #[test]
    fn unknown_status_cannot_claim_disconnected_or_ready() {
        let mut s = State::default();
        s.receive(0, "Status", Ok(snapshot(json!({}))));
        s.receive(0, "Status", Err("Offline".into()));
        assert_eq!(s.phase(), "uncertain");
        assert!(!s.available());
        assert_eq!(s.status_request(), json!({"action":"Status"}));
    }
    #[test]
    fn saving_without_later_edits_adopts_normalized_settings() {
        let mut s = State::default();
        s.receive(0, "Status", Ok(snapshot(json!({"value":1}))));
        s.edit(json!({"value":2}));
        s.begin(json!({"action":"ApplySettings"}), "test").unwrap();
        s.receive(
            1,
            "ApplySettings",
            Ok(snapshot(json!({"value":2,"normalized":true}))),
        );
        assert_eq!(s.draft["normalized"], true);
        assert!(!s.dirty());
    }
    #[test]
    fn foreign_receipt_and_malformed_status_do_not_complete_the_current_operation() {
        let mut s = State::default();
        s.begin(json!({"action":"Import"}), "test").unwrap();
        s.receive(
            1,
            "Import",
            Ok(json!({"ok":true,"request_id":"another-window"})),
        );
        assert!(s.pending.is_some());
        s.receive(1, "Status", Ok(json!({"ok":true,"data":{}})));
        assert!(!s.loaded);
        assert!(s.pending.is_some());
        assert!(!s.available());
    }
    #[test]
    fn failed_exit_keeps_recovery_available() {
        let mut s = State::default();
        s.begin(json!({"action":"Exit"}), "test").unwrap();
        let mut answer = snapshot(json!({}));
        answer["ok"] = json!(false);
        answer["error"] = json!("DNS cleanup failed");
        answer["data"]["dns_recovery_pending"] = json!(true);
        let result = s.receive(1, "Exit", Ok(answer));
        assert!(matches!(result, Receipt::Completed { ok: false, .. }));
        assert_eq!(s.phase(), "recovery");
        assert!(s.can_disconnect());
        assert!(!s.busy());
    }
}
