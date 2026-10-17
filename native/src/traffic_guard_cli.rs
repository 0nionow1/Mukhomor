use crate::{traffic_guard, traffic_guard_tun};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use std::{fmt, io::Read, path::Path};

// serde_json::Value normally accepts duplicate object keys. Reject them at
// every depth before policy validation, including escaped equivalent keys.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, x: bool) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_i64<E: de::Error>(self, x: i64) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_u64<E: de::Error>(self, x: u64) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_f64<E: de::Error>(self, x: f64) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_str<E: de::Error>(self, x: &str) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_string<E: de::Error>(self, x: String) -> Result<Strict, E> {
                Ok(Strict(json!(x)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut result = Vec::new();
                while let Some(Strict(v)) = a.next_element()? {
                    result.push(v);
                }
                Ok(Strict(Value::Array(result)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut result = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if result.contains_key(&k) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    let Strict(v) = a.next_value()?;
                    result.insert(k, v);
                }
                Ok(Strict(Value::Object(result)))
            }
        }
        d.deserialize_any(V)
    }
}
fn request(path: &str) -> Result<Value, String> {
    let file = std::fs::File::open(path).map_err(|_| "WFP_REQUEST_READ")?;
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| "WFP_REQUEST_READ")?;
    if bytes.len() > 16384 {
        return Err("WFP_REQUEST_SIZE".into());
    }
    parse(&bytes)
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|v| v.0)
        .map_err(|_| "WFP_REQUEST_JSON".into())
}
fn refine_tunnel_status(mut status: Value, active: bool) -> Value {
    status["active_owned_tun"] = json!(active);
    if !active {
        status["protected"] = json!(false);
    }
    status
}
pub fn run(args: &[String], base: &Path) -> Result<(), String> {
    let mut action = None;
    let mut input = None;
    let mut output = None;
    let mut explicit_base = false;
    let mut i = 1;
    while i < args.len() {
        let slot = match args[i].as_str() {
            "--traffic-guard-action" => &mut action,
            "--request" => &mut input,
            "--output" => &mut output,
            "--base" => {
                if explicit_base || i + 1 >= args.len() {
                    return Err("WFP_CLI_ARGUMENT".into());
                }
                explicit_base = true;
                i += 2;
                continue;
            }
            _ => return Err("WFP_CLI_ARGUMENT".into()),
        };
        if slot.is_some() || i + 1 >= args.len() {
            return Err("WFP_CLI_ARGUMENT".into());
        }
        *slot = Some(args[i + 1].as_str());
        i += 2;
    }
    let action = action.ok_or("WFP_CLI_ACTION")?;
    let output = output.ok_or("WFP_OUTPUT_REQUIRED")?;
    if !base.is_absolute()
        || !matches!(
            action,
            "preview" | "arm" | "update" | "status" | "resolve-tun" | "disarm" | "inspect"
        )
    {
        return Err("WFP_CLI_ACTION".into());
    }
    let result = if action == "resolve-tun" {
        json!({"ok":true,"tunnel_luid":traffic_guard_tun::resolve(base)?})
    } else if action == "inspect" {
        traffic_guard::inspect()?
    } else if action == "disarm" {
        traffic_guard::disarm()?
    } else {
        let value = request(input.ok_or("WFP_REQUEST_REQUIRED")?)?;
        let plan = traffic_guard::Plan::parse(&value)?;
        // The request cannot authorize an arbitrary executable or physical NIC.
        let core = value["core_executable"].as_str().ok_or("WFP_PLAN_PATH")?;
        let allowed: &[&str] = if matches!(value["transport"].as_str(),Some("tor-bridges"|"tor-snowflake")) {
            &["mukhomor-pt.exe"]
        } else if value["transport"] == "tor" {
            &["mukhomor-tor.exe"]
        } else {
            &["mihomo-mycelium.exe", "mihomo-windows-amd64-compatible.exe"]
        };
        if !allowed.iter().any(|n| {
            base.join("bin")
                .join(n)
                .to_string_lossy()
                .eq_ignore_ascii_case(core)
        }) {
            return Err("WFP_CORE_BUNDLE_REQUIRED".into());
        }
        let luid = value["tunnel_luid"].as_u64().ok_or("WFP_PLAN_LUID")?;
        match action {
            "preview" => traffic_guard::preview(&plan),
            "arm" | "update" => {
                if luid != 0 && traffic_guard_tun::resolve(base)? != luid {
                    return Err("WFP_OWNED_TUN_REQUIRED".into());
                }
                if luid != 0 && !traffic_guard_tun::active(base, luid)? {
                    return Err("WFP_OWNED_TUN_INACTIVE".into());
                }
                if action == "arm" {
                    traffic_guard::arm(&plan)?
                } else {
                    traffic_guard::update(&plan)?
                }
            }
            "status" => {
                let status = traffic_guard::status(&plan)?;
                let active = traffic_guard_tun::active(base, luid).unwrap_or(false);
                refine_tunnel_status(status, active)
            }
            _ => return Err("WFP_CLI_ACTION".into()),
        }
    };
    std::fs::write(
        output,
        serde_json::to_vec(&result).map_err(|_| "WFP_OUTPUT_JSON")?,
    )
    .map_err(|_| "WFP_OUTPUT_WRITE".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_tunnel_preserves_filter_proof_and_never_claims_protection() {
        let rules = json!({"armed":true,"verified":true,"protected":true});
        let inactive = refine_tunnel_status(rules.clone(), false);
        assert_eq!(
            inactive,
            json!({"armed":true,"verified":true,"protected":false,"active_owned_tun":false})
        );
        let active = refine_tunnel_status(rules, true);
        assert_eq!(active["protected"], true);
        assert_eq!(active["active_owned_tun"], true);
        let unverified = refine_tunnel_status(
            json!({"armed":true,"verified":false,"protected":false}),
            true,
        );
        assert_eq!(unverified["protected"], false);
        assert_eq!(unverified["verified"], false);
    }
    #[test]
    fn rejects_duplicate_nested_and_escaped_json_keys_and_trailing_input() {
        for invalid in [
            r#"{"schema":1,"schema":2}"#,
            r#"{"e":[{"port":1,"\u0070ort":2}]}"#,
            "{}{}",
            "[NaN]",
        ] {
            assert!(parse(invalid.as_bytes()).is_err(), "{invalid}");
        }
        assert_eq!(
            parse(br#"{"n":18446744073709551615,"a":[true,null,"x"]}"#).unwrap()["n"].as_u64(),
            Some(u64::MAX)
        );
    }
}
