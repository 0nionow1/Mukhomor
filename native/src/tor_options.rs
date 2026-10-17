//! Shared, bounded bridge input. Errors never echo a private bridge line.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TorOptions {
    pub mode: String,
    pub bridges: String,
}
impl Default for TorOptions {
    fn default() -> Self {
        Self {
            mode: "direct".into(),
            bridges: String::new(),
        }
    }
}

// Tor Project's published Snowflake configuration, checked 2026-10-16:
// https://support.torproject.org/little-t-tor/circumvention/using-bridges/
pub const SNOWFLAKE: &str = "snowflake 192.0.2.3:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://1098762253.rsc.cdn77.org front=www.phpmyadmin.net,cdn.zk.mk ice=stun:stun.antisip.com:3478,stun:stun.epygi.com:3478,stun:stun.uls.co.za:3478,stun:stun.voipgate.com:3478,stun:stun.mixvoip.com:3478,stun:stun.nextcloud.com:3478,stun:stun.bethesda.net:3478,stun:stun.nextcloud.com:443 utls-imitate=hellorandomizedalpn";
// Official alternate rendezvous for networks blocking CDN77. The bridge
// identity is still authenticated by Arti; AMP only carries broker signalling.
// https://forum.torproject.org/t/20650 (checked 2026-10-17)
pub const SNOWFLAKE_AMP: &str = "snowflake 192.0.2.5:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://snowflake-broker.torproject.net/ ampcache=https://cdn.ampproject.org/ front=www.google.com ice=stun:stun.rtc.yandex.net,stun:stun.epygi.com:3478,stun:stun.uls.co.za:3478,stun:stun.voipgate.com:3478,stun:stun.mixvoip.com:3478,stun:stun.nextcloud.com:3478,stun:stun.bethesda.net:3478,stun:stun.nextcloud.com:443 utls-imitate=hellorandomizedalpn";

impl TorOptions {
    pub fn transport_mode(&self) -> &str {
        if self.mode == "snowflake-amp" { "snowflake" } else { &self.mode }
    }
    pub fn lines(&self) -> Vec<&str> {
        if self.mode == "snowflake-amp" {
            vec![SNOWFLAKE_AMP]
        } else if self.mode == "snowflake" {
            vec![SNOWFLAKE]
        } else {
            self.bridges.lines().collect()
        }
    }
    pub fn validate(mut self) -> Result<Self, &'static str> {
        if !matches!(
            self.mode.as_str(),
            "direct" | "obfs4" | "webtunnel" | "snowflake" | "snowflake-amp"
        ) {
            return Err("Unsupported Tor connection method");
        }
        if self.bridges.len() > 8192 {
            return Err("Tor bridge list exceeds 8 KiB");
        }
        if self.mode == "direct" || self.transport_mode() == "snowflake" {
            if !self.bridges.trim().is_empty() {
                return Err("This Tor method does not accept custom bridge lines");
            }
            self.bridges.clear();
            return Ok(self);
        }
        let mut normalized = Vec::new();
        let mut seen = BTreeSet::new();
        for line in self
            .bridges
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let line = line.strip_prefix("Bridge ").unwrap_or(line);
            if line.len() > 2048
                || !line.is_ascii()
                || line.bytes().any(|b| b < 32 || b == 127)
                || line.contains(['\\', ';'])
            {
                return Err("Invalid Tor bridge line");
            }
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            if fields.len() < 4
                || fields[0] != self.mode
                || fields[2].len() != 40
                || !fields[2].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("Invalid Tor bridge line");
            }
            let addr: std::net::SocketAddr = fields[1]
                .parse()
                .map_err(|_| "Invalid Tor bridge address")?;
            if addr.port() == 0
                || matches!(addr.port(), 53 | 853)
                || addr.ip().is_loopback()
                || addr.ip().is_unspecified()
                || addr.ip().is_multicast()
            {
                return Err("Invalid Tor bridge address");
            }
            let mut keys = BTreeSet::new();
            for field in &fields[3..] {
                let (k, v) = field.split_once('=').ok_or("Invalid Tor bridge option")?;
                let allowed = if self.mode == "obfs4" {
                    matches!(k, "cert" | "iat-mode")
                } else {
                    matches!(k, "url" | "ver" | "utls" | "cert")
                };
                if !allowed || v.is_empty() || !keys.insert(k) {
                    return Err("Invalid Tor bridge option");
                }
                if k == "url"
                    && (!v.starts_with("https://") || v.len() <= 8 || v.contains(['@', '#']))
                {
                    return Err("WebTunnel requires an HTTPS bridge URL");
                }
                if k == "iat-mode" && !matches!(v, "0" | "1" | "2") {
                    return Err("Invalid Tor bridge option");
                }
            }
            if (self.mode == "obfs4" && !keys.contains("cert"))
                || (self.mode == "webtunnel" && !keys.contains("url"))
            {
                return Err("Missing Tor bridge option");
            }
            let line = fields.join(" ");
            if seen.insert(line.clone()) {
                normalized.push(line);
            }
            if normalized.len() > 16 {
                return Err("At most 16 Tor bridges are supported");
            }
        }
        if normalized.is_empty() {
            return Err("Add at least one Tor bridge");
        }
        self.bridges = normalized.join("\n");
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amp_preset_keeps_snowflake_transport_and_rejects_custom_input() {
        let options = TorOptions { mode: "snowflake-amp".into(), bridges: String::new() }.validate().unwrap();
        assert_eq!(options.transport_mode(), "snowflake");
        assert!(options.lines()[0].contains("ampcache=https://cdn.ampproject.org/"));
        assert!(TorOptions { bridges: "obfs4 private-data".into(), ..options }.validate().is_err());
    }
    #[test]
    fn bridge_validation_rejects_injection_downgrades_and_untrusted_transports() {
        let good = "webtunnel 192.0.2.1:443 0123456789012345678901234567890123456789 url=https://example.org/secret ver=0.0.1";
        assert!(
            TorOptions {
                mode: "webtunnel".into(),
                bridges: good.into()
            }
            .validate()
            .is_ok()
        );
        for bad in [
            good.replace("https:", "http:"),
            good.replace("url=", "proxy="),
            good.replace("webtunnel", "obfs4"),
            good.replace("192.0.2.1", "127.0.0.1"),
            format!("{good} url=https://other.example/"),
            format!("{good}\nClientTransportPlugin exec evil"),
            good.replace("secret", "secret\0"),
        ] {
            assert!(
                TorOptions {
                    mode: "webtunnel".into(),
                    bridges: bad
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            TorOptions {
                mode: "snowflake".into(),
                bridges: good.into()
            }
            .validate()
            .is_err()
        );
    }
}
