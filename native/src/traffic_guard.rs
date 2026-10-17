//! Persistent, narrowly scoped WFP policy. Never activated by this module's tests.
//!
//! The SYSTEM service must supply its protected installed core path and the LUID
//! of its own Wintun adapter. A user-supplied physical-interface LUID is unsafe.
//! Persistence protects against service exit; it is not a claim about drivers,
//! malicious administrators, raw L2 traffic, or an untested reboot sequence.
//! Sources: Microsoft WFP object management, filter arbitration, ALE
//! reauthorization and FWPM_FILTER0 documentation. DHCP/NDP rule shapes were
//! reviewed against WireGuard Windows tunnel/firewall/rules.go (MIT).

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::*;
use windows_sys::core::GUID;

const MAX_ENDPOINTS: usize = 8;
const MAX_FILTERS: usize = 128;
const MAGIC: &[u8] = b"Mukhomor.WFP.v1\0";
const PROVIDER: GUID = GUID::from_u128(0xe0863c5b_3e6c_47da_83dd_bf8a89b90000);
const SUBLAYER: GUID = GUID::from_u128(0xe0863c5b_3e6c_47da_83dd_bf8a89b90001);
const FILTER_BASE: u128 = 0xe0863c5b_3e6c_47da_83dd_bf8a89b80000;
const SUBLAYER_WEIGHT: u16 = 0xffff;
// This is a requested priority, not a readback invariant. BFE assigns the
// closest available sublayer weight (Microsoft: "Installing a Provider").
// FWP_UINT8 encodes a BFE weight range, whose legal values are 0..=15.
// Keep distinct ranges in the same security order as the rule priorities.
// Microsoft: https://learn.microsoft.com/en-us/windows/win32/fwp/filter-weight-assignment
const WEIGHT_DEFAULT_BLOCK: u8 = 1;
const WEIGHT_PRIVATE_LAN: u8 = 6;
const WEIGHT_LINK_CONTROL: u8 = 8;
const WEIGHT_TOR_TRANSPORT: u8 = 9;
const WEIGHT_PHYSICAL_DNS_BLOCK: u8 = 10;
const WEIGHT_CORE_ENDPOINT: u8 = 12;
const WEIGHT_TUNNEL: u8 = 14;
const WEIGHT_LOOPBACK: u8 = 15;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Endpoint {
    address: IpAddr,
    port: u16,
    protocol: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    core_executable: String,
    tunnel_luid: u64,
    endpoints: Vec<Endpoint>,
    allow_private_lan: bool,
    tor: bool,
    tor_transport: String,
}

impl Plan {
    /// Pure parsing: no file, token, interface, engine or routing operations.
    /// The caller must reject duplicate JSON keys before converting text to Value.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let o = value.as_object().ok_or("WFP_PLAN_OBJECT")?;
        let tor = value["schema"] == 2 && matches!(value["transport"].as_str(),Some("tor"|"tor-bridges"|"tor-snowflake"));
        if o.len() != (if tor { 6 } else { 5 })
            || o.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "schema"
                        | "core_executable"
                        | "tunnel_luid"
                        | "endpoints"
                        | "allow_private_lan"
                        | "transport"
                )
            })
        {
            return Err("WFP_PLAN_FIELDS".into());
        }
        if !tor && (value["schema"].as_u64() != Some(1) || o.contains_key("transport")) {
            return Err("WFP_PLAN_SCHEMA".into());
        }
        let path = value["core_executable"].as_str().ok_or("WFP_PLAN_PATH")?;
        let b = path.as_bytes();
        if b.len() < 8
            || b.len() > 512
            || !b[0].is_ascii_alphabetic()
            || b[1] != b':'
            || b[2] != b'\\'
            || path
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '*' | '?' | '"' | '<' | '>' | '|'))
            || path[2..].contains(':')
            || !path.to_ascii_lowercase().ends_with(".exe")
            || path[3..]
                .split('\\')
                .any(|p| p.is_empty() || p == "." || p == ".." || p.ends_with(['.', ' ']))
        {
            return Err("WFP_PLAN_PATH".into());
        }
        let tunnel_luid = value["tunnel_luid"].as_u64().ok_or("WFP_PLAN_LUID")?;
        let allow_private_lan = value["allow_private_lan"].as_bool().ok_or("WFP_PLAN_LAN")?;
        let rows = value["endpoints"].as_array().ok_or("WFP_PLAN_ENDPOINTS")?;
        if (!tor && rows.is_empty()) || rows.len() > MAX_ENDPOINTS || (tor && (!rows.is_empty() || allow_private_lan)) {
            return Err("WFP_PLAN_ENDPOINT_COUNT".into());
        }
        let mut unique = BTreeSet::new();
        for row in rows {
            let r = row.as_object().ok_or("WFP_PLAN_ENDPOINT")?;
            if r.len() != 3
                || r.keys()
                    .any(|k| !matches!(k.as_str(), "address" | "port" | "protocol"))
            {
                return Err("WFP_PLAN_ENDPOINT_FIELDS".into());
            }
            let address: IpAddr = row["address"]
                .as_str()
                .ok_or("WFP_PLAN_ADDRESS")?
                .parse()
                .map_err(|_| "WFP_PLAN_ADDRESS")?;
            if address.is_unspecified()
                || address.is_loopback()
                || address.is_multicast()
                || matches!(address, IpAddr::V4(a) if a == Ipv4Addr::BROADCAST || a.is_link_local() || a.octets()[0] == 0 || a.octets()[0] >= 224)
                || matches!(address, IpAddr::V6(a) if a.to_ipv4_mapped().is_some() || a.is_unicast_link_local())
            {
                return Err("WFP_PLAN_ADDRESS".into());
            }
            let port = row["port"]
                .as_u64()
                .filter(|p| (1..=65535).contains(p))
                .ok_or("WFP_PLAN_PORT")? as u16;
            let protocol = match row["protocol"].as_str() {
                Some("tcp") => 6,
                Some("udp") => 17,
                _ => return Err("WFP_PLAN_PROTOCOL".into()),
            };
            if !unique.insert(Endpoint {
                address,
                port,
                protocol,
            }) {
                return Err("WFP_PLAN_DUPLICATE_ENDPOINT".into());
            }
        }
        Ok(Self {
            core_executable: path.into(),
            tunnel_luid,
            endpoints: unique.into_iter().collect(),
            allow_private_lan,
            tor,
            tor_transport: if tor {value["transport"].as_str().unwrap().into()} else {String::new()},
        })
    }
    fn value(&self) -> Value {
        let mut value = json!({"schema":1,"core_executable":self.core_executable,"tunnel_luid":self.tunnel_luid,
            "endpoints":self.endpoints.iter().map(|e| json!({"address":e.address.to_string(),
                "port":e.port,"protocol":if e.protocol == 6 {"tcp"} else {"udp"}})).collect::<Vec<_>>(),
            "allow_private_lan":self.allow_private_lan});
        if self.tor { value["schema"] = json!(2); value["transport"] = json!(self.tor_transport); }
        value
    }
    fn marker(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(self.value().to_string().as_bytes());
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Layer {
    Connect4,
    Connect6,
    Accept4,
    Accept6,
    BootIn4,
    BootIn6,
    BootOut4,
    BootOut6,
}
impl Layer {
    fn key(self) -> GUID {
        match self {
            Self::Connect4 => FWPM_LAYER_ALE_AUTH_CONNECT_V4,
            Self::Connect6 => FWPM_LAYER_ALE_AUTH_CONNECT_V6,
            Self::Accept4 => FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
            Self::Accept6 => FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6,
            Self::BootIn4 => FWPM_LAYER_INBOUND_IPPACKET_V4,
            Self::BootIn6 => FWPM_LAYER_INBOUND_IPPACKET_V6,
            Self::BootOut4 => FWPM_LAYER_OUTBOUND_IPPACKET_V4,
            Self::BootOut6 => FWPM_LAYER_OUTBOUND_IPPACKET_V6,
        }
    }
    fn boot(self) -> bool {
        matches!(
            self,
            Self::BootIn4 | Self::BootIn6 | Self::BootOut4 | Self::BootOut6
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Protocol,
    RemoteAddress,
    LocalAddress,
    RemotePort,
    LocalPort,
    Flags,
    Interface,
    App,
    User,
}
impl Field {
    fn key(self) -> GUID {
        match self {
            Self::Protocol => FWPM_CONDITION_IP_PROTOCOL,
            Self::RemoteAddress => FWPM_CONDITION_IP_REMOTE_ADDRESS,
            Self::LocalAddress => FWPM_CONDITION_IP_LOCAL_ADDRESS,
            Self::RemotePort => FWPM_CONDITION_IP_REMOTE_PORT,
            Self::LocalPort => FWPM_CONDITION_IP_LOCAL_PORT,
            Self::Flags => FWPM_CONDITION_FLAGS,
            Self::Interface => FWPM_CONDITION_IP_LOCAL_INTERFACE,
            Self::App => FWPM_CONDITION_ALE_APP_ID,
            Self::User => FWPM_CONDITION_ALE_USER_ID,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Data {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    Address(IpAddr, u8),
    App,
    System,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Condition {
    field: Field,
    flags_match: bool,
    data: Data,
}
fn c(field: Field, data: Data) -> Condition {
    Condition {
        field,
        flags_match: false,
        data,
    }
}
fn ip(field: Field, address: &str, prefix: u8) -> Condition {
    c(
        field,
        Data::Address(address.parse().expect("static IP"), prefix),
    )
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Rule {
    id: u16,
    layer: Layer,
    permit: bool,
    weight: u8,
    conditions: Vec<Condition>,
}
impl Rule {
    fn key(&self) -> GUID {
        GUID::from_u128(FILTER_BASE + self.id as u128)
    }
    fn marker(&self) -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&self.id.to_be_bytes());
        b
    }
    fn flags(&self) -> u32 {
        if self.layer.boot() {
            FWPM_FILTER_FLAG_BOOTTIME
        } else {
            FWPM_FILTER_FLAG_PERSISTENT
        }
    }
}

fn rules(plan: &Plan) -> Vec<Rule> {
    let mut result = Vec::new();
    let mut add = |layer, permit, weight, conditions| {
        let id = (result.len() + 1) as u16;
        result.push(Rule {
            id,
            layer,
            permit,
            weight,
            conditions,
        });
    };
    for layer in [
        Layer::Connect4,
        Layer::Connect6,
        Layer::Accept4,
        Layer::Accept6,
    ] {
        add(layer, false, WEIGHT_DEFAULT_BLOCK, vec![]);
        add(
            layer,
            true,
            WEIGHT_LOOPBACK,
            vec![Condition {
                field: Field::Flags,
                flags_match: true,
                data: Data::U32(FWP_CONDITION_FLAG_IS_LOOPBACK),
            }],
        );
        if plan.tunnel_luid != 0 {
            add(
                layer,
                true,
                WEIGHT_TUNNEL,
                vec![c(Field::Interface, Data::U64(plan.tunnel_luid))],
            );
        }
    }
    if plan.tor {
        // Dynamic Tor relays are permitted only for the installed helper as
        // SYSTEM. Physical DNS blocks retain higher priority. Only Snowflake's
        // own bridge adapter may use WebRTC UDP; user UDP is still blocked.
        for layer in [Layer::Connect4, Layer::Connect6] {
          for protocol in if plan.tor_transport=="tor-snowflake" { &[6u8,17][..] } else { &[6u8][..] } {
            add(layer, true, WEIGHT_TOR_TRANSPORT, vec![
                c(Field::App, Data::App), c(Field::User, Data::System),
                c(Field::Protocol, Data::U8(*protocol)),
            ]);
          }
        }
    }
    for e in &plan.endpoints {
        add(
            if e.address.is_ipv6() {
                Layer::Connect6
            } else {
                Layer::Connect4
            },
            true,
            WEIGHT_CORE_ENDPOINT,
            vec![
                c(Field::App, Data::App),
                c(Field::User, Data::System),
                c(Field::Protocol, Data::U8(e.protocol)),
                c(Field::RemotePort, Data::U16(e.port)),
                c(
                    Field::RemoteAddress,
                    Data::Address(e.address, if e.address.is_ipv6() { 128 } else { 32 }),
                ),
            ],
        );
    }
    // Loopback/TUN and only the exact core endpoint are above physical DNS blocks.
    for layer in [Layer::Connect4, Layer::Connect6] {
        for port in [53, 853] {
            for protocol in [6, 17] {
                add(
                    layer,
                    false,
                    WEIGHT_PHYSICAL_DNS_BLOCK,
                    vec![
                        c(Field::Protocol, Data::U8(protocol)),
                        c(Field::RemotePort, Data::U16(port)),
                    ],
                );
            }
        }
    }
    for layer in [Layer::Accept4, Layer::Accept6] {
        for port in [53, 853] {
            for protocol in [6, 17] {
                add(
                    layer,
                    false,
                    WEIGHT_PHYSICAL_DNS_BLOCK,
                    vec![
                        c(Field::Protocol, Data::U8(protocol)),
                        c(Field::LocalPort, Data::U16(port)),
                    ],
                );
            }
        }
    }
    // No unrestricted service permit. DHCP exceptions contain both UDP ports.
    add(
        Layer::Connect4,
        true,
        WEIGHT_LINK_CONTROL,
        vec![
            c(Field::Protocol, Data::U8(17)),
            c(Field::LocalPort, Data::U16(68)),
            c(Field::RemotePort, Data::U16(67)),
            ip(Field::RemoteAddress, "255.255.255.255", 32),
        ],
    );
    add(
        Layer::Accept4,
        true,
        WEIGHT_LINK_CONTROL,
        vec![
            c(Field::Protocol, Data::U8(17)),
            c(Field::LocalPort, Data::U16(68)),
            c(Field::RemotePort, Data::U16(67)),
        ],
    );
    for remote in ["ff02::1:2", "ff05::1:3"] {
        add(
            Layer::Connect6,
            true,
            WEIGHT_LINK_CONTROL,
            vec![
                c(Field::Protocol, Data::U8(17)),
                c(Field::LocalPort, Data::U16(546)),
                c(Field::RemotePort, Data::U16(547)),
                ip(Field::LocalAddress, "fe80::", 10),
                ip(Field::RemoteAddress, remote, 128),
            ],
        );
    }
    add(
        Layer::Accept6,
        true,
        WEIGHT_LINK_CONTROL,
        vec![
            c(Field::Protocol, Data::U8(17)),
            c(Field::LocalPort, Data::U16(546)),
            c(Field::RemotePort, Data::U16(547)),
            ip(Field::LocalAddress, "fe80::", 10),
            ip(Field::RemoteAddress, "fe80::", 10),
        ],
    );
    // At ALE ICMP type/code use LOCAL_PORT/REMOTE_PORT respectively (SDK aliases).
    // These are link-local NDP exceptions, not general Internet ICMPv6 permits.
    // Hop-limit 255 cannot be expressed at these ALE layers: VM verification gate.
    let ndp = |kind| {
        vec![
            c(Field::Protocol, Data::U8(58)),
            c(Field::LocalPort, Data::U16(kind)),
            c(Field::RemotePort, Data::U16(0)),
        ]
    };
    let mut cs = ndp(133);
    cs.push(ip(Field::RemoteAddress, "ff02::2", 128));
    add(Layer::Connect6, true, WEIGHT_LINK_CONTROL, cs);
    let mut cs = ndp(134);
    cs.push(ip(Field::RemoteAddress, "fe80::", 10));
    add(Layer::Accept6, true, WEIGHT_LINK_CONTROL, cs);
    for kind in [135, 136] {
        // Multicast neighbor resolution, unicast link-local updates, DAD (::).
        for remote in [
            ip(Field::RemoteAddress, "ff02::1:ff00:0", 104),
            ip(Field::RemoteAddress, "fe80::", 10),
        ] {
            let mut cs = ndp(kind);
            cs.push(remote);
            add(Layer::Connect6, true, WEIGHT_LINK_CONTROL, cs);
        }
        for remote in [
            ip(Field::RemoteAddress, "fe80::", 10),
            ip(Field::RemoteAddress, "::", 128),
        ] {
            let mut cs = ndp(kind);
            cs.push(remote);
            add(Layer::Accept6, true, WEIGHT_LINK_CONTROL, cs);
        }
    }
    let mut cs = ndp(136);
    cs.push(ip(Field::RemoteAddress, "ff02::1", 128));
    add(Layer::Connect6, true, WEIGHT_LINK_CONTROL, cs);
    if plan.allow_private_lan {
        for layer in [Layer::Connect4, Layer::Accept4] {
            for (address, prefix) in [("10.0.0.0", 8), ("172.16.0.0", 12), ("192.168.0.0", 16)] {
                add(
                    layer,
                    true,
                    WEIGHT_PRIVATE_LAN,
                    vec![ip(Field::RemoteAddress, address, prefix)],
                );
            }
        }
        for layer in [Layer::Connect6, Layer::Accept6] {
            add(
                layer,
                true,
                WEIGHT_PRIVATE_LAN,
                vec![ip(Field::RemoteAddress, "fc00::", 7)],
            );
        }
    }
    // BOOTTIME and PERSISTENT may not be combined. Boot blocks deliberately have
    // no DNS, DHCP, endpoint or TUN exceptions until BFE activates runtime policy.
    for layer in [
        Layer::BootIn4,
        Layer::BootIn6,
        Layer::BootOut4,
        Layer::BootOut6,
    ] {
        add(layer, false, WEIGHT_DEFAULT_BLOCK, vec![]);
    }
    assert!(result.len() <= MAX_FILTERS);
    result
}

// A discovered saved policy does not prove that its TUN still exists. Only the
// service may establish that identity through its protected Wintun helper.
#[cfg(any(windows, test))]
fn inspection_result(plan: Option<&Plan>) -> Value {
    json!({"schema":1,"action":"inspect","armed":plan.is_some(),"verified":true,
        "protected":false,"persistent":plan.is_some(),
        "boot_policy_verified":plan.is_some(),"runtime_policy_verified":plan.is_some(),
        "filter_count":plan.map(|p|rules(p).len()).unwrap_or(0),
        "plan":plan.map(Plan::value),"tunnel_identity_requires_service_validation":true})
}

pub fn preview(plan: &Plan) -> Value {
    let rs = rules(plan);
    json!({"schema":1,"action":"preview","armed":false,"verified":false,"protected":false,
        "bootstrap":plan.tunnel_luid==0,"runtime_filters":rs.iter().filter(|r|!r.layer.boot()).count(),
        "boot_filters":4,"persistent":true,"system_endpoint_permit":true,
        "plan":plan.value(),"rules":rs.iter().map(|r|json!({"id":r.id,"layer":format!("{:?}",r.layer),
            "permit":r.permit,"weight":r.weight,"persistent":!r.layer.boot(),"boottime":r.layer.boot(),
            "conditions":r.conditions.iter().map(|c|format!("{:?}",c)).collect::<Vec<_>>() })).collect::<Vec<_>>()})
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}
fn key_id(key: &GUID) -> Option<u16> {
    (1..=MAX_FILTERS as u16).find(|id| guid_eq(key, &GUID::from_u128(FILTER_BASE + *id as u128)))
}

#[cfg(any(windows, test))]
const ENUM_PAGE_SIZE: u32 = 256;
#[cfg(any(windows, test))]
const MAX_ENUMERATED_FILTERS: usize = 100_000;
// FWPM_LAYER0.layerId is a UINT16; bound the complete layer snapshot, not just
// the eight layers in our policy. Unexpected owned filters can exist elsewhere.
#[cfg(any(windows, test))]
const MAX_ENUMERATED_LAYERS: usize = u16::MAX as usize + 1;

#[cfg(any(windows, test))]
fn charge_enumeration(
    total: &mut usize,
    count: u32,
    maximum: usize,
    category: &str,
) -> Result<(), String> {
    let next = total.checked_add(count as usize).ok_or(category)?;
    if next > maximum {
        return Err(category.into());
    }
    *total = next;
    Ok(())
}

#[cfg(any(windows, test))]
fn filter_enum_template(layer: GUID) -> Result<FWPM_FILTER_ENUM_TEMPLATE0, String> {
    if guid_eq(&layer, &GUID::from_u128(0)) {
        return Err("WFP_LAYER_ENUM_INVALID".into());
    }
    Ok(FWPM_FILTER_ENUM_TEMPLATE0 {
        layerKey: layer,
        enumType: FWP_FILTER_ENUM_FULLY_CONTAINED,
        flags: FWP_FILTER_ENUM_FLAG_INCLUDE_BOOTTIME | FWP_FILTER_ENUM_FLAG_INCLUDE_DISABLED,
        actionMask: u32::MAX,
        ..Default::default()
    })
}

#[cfg(any(windows, test))]
fn owned_filter_rule<'a>(
    key: &GUID,
    sublayer: &GUID,
    provider: Option<&GUID>,
    expected: &'a [Rule],
) -> Result<Option<&'a Rule>, String> {
    let relevant = guid_eq(sublayer, &SUBLAYER)
        || key_id(key).is_some()
        || provider.is_some_and(|p| guid_eq(p, &PROVIDER));
    if !relevant {
        return Ok(None);
    }
    expected
        .iter()
        .find(|r| guid_eq(&r.key(), key))
        .map(Some)
        .ok_or_else(|| "WFP_UNEXPECTED_OWN_FILTER".into())
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Foundation::*, Security::Authorization::*, Security::*, System::Rpc::RPC_C_AUTHN_WINNT,
        System::Threading::*,
    };

    fn ok(code: u32, category: &str) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(format!("{category}:{code:08x}"))
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn require_system() -> Result<(), String> {
        unsafe {
            let mut token = ptr::null_mut();
            if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0
                && (GetLastError() != ERROR_NO_TOKEN
                    || OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0)
            {
                return Err("WFP_SYSTEM_TOKEN".into());
            }
            let token = Handle(token);
            let mut data = [0usize; 128];
            let mut needed = 0;
            if GetTokenInformation(
                token.0,
                TokenUser,
                data.as_mut_ptr().cast(),
                mem::size_of_val(&data) as u32,
                &mut needed,
            ) == 0
                || needed as usize > mem::size_of_val(&data)
            {
                return Err("WFP_SYSTEM_TOKEN".into());
            }
            let user = &*(data.as_ptr().cast::<TOKEN_USER>());
            if IsWellKnownSid(user.User.Sid, WinLocalSystemSid) == 0 {
                return Err("WFP_SYSTEM_REQUIRED".into());
            }
        }
        Ok(())
    }
    struct LocalSd(PSECURITY_DESCRIPTOR, u32);
    impl LocalSd {
        fn from(s: &str) -> Result<Self, String> {
            let w = wide(s);
            let mut sd = ptr::null_mut();
            let mut len = 0;
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    &mut len,
                )
            } == 0
                || sd.is_null()
            {
                return Err("WFP_SECURITY_DESCRIPTOR".into());
            }
            Ok(Self(sd, len))
        }
    }
    impl Drop for LocalSd {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    struct WfpMem<T>(*mut T);
    impl<T> Drop for WfpMem<T> {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    FwpmFreeMemory0((&mut self.0 as *mut *mut T).cast());
                }
            }
        }
    }
    struct Engine(HANDLE);
    impl Engine {
        fn open() -> Result<Self, String> {
            let session = FWPM_SESSION0 {
                txnWaitTimeoutInMSec: 5000,
                ..Default::default()
            };
            let mut h = ptr::null_mut();
            ok(
                unsafe {
                    FwpmEngineOpen0(
                        ptr::null(),
                        RPC_C_AUTHN_WINNT,
                        ptr::null(),
                        &session,
                        &mut h,
                    )
                },
                "WFP_ENGINE_OPEN",
            )?;
            if h.is_null() {
                return Err("WFP_ENGINE_HANDLE".into());
            }
            Ok(Self(h))
        }
    }
    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe {
                FwpmEngineClose0(self.0);
            }
        }
    }
    struct Transaction<'a> {
        engine: &'a Engine,
        active: bool,
    }
    impl<'a> Transaction<'a> {
        fn begin(engine: &'a Engine, read_only: bool) -> Result<Self, String> {
            ok(
                unsafe {
                    FwpmTransactionBegin0(engine.0, if read_only { FWPM_TXN_READ_ONLY } else { 0 })
                },
                "WFP_TRANSACTION_BEGIN",
            )?;
            Ok(Self {
                engine,
                active: true,
            })
        }
        fn commit(mut self) -> Result<(), String> {
            let code = unsafe { FwpmTransactionCommit0(self.engine.0) };
            if code != 0 {
                let error = format!("WFP_TRANSACTION_COMMIT:{code:08x}");
                return match self.abort() {
                    Ok(()) => Err(error),
                    Err(abort) => Err(format!("{error}|{abort}")),
                };
            }
            self.active = false;
            Ok(())
        }
        fn abort(mut self) -> Result<(), String> {
            let code = unsafe { FwpmTransactionAbort0(self.engine.0) };
            if code == 0 {
                self.active = false;
            }
            ok(code, "WFP_TRANSACTION_ABORT")
        }
    }
    impl Drop for Transaction<'_> {
        fn drop(&mut self) {
            if self.active {
                unsafe {
                    FwpmTransactionAbort0(self.engine.0);
                }
            }
        }
    }
    fn transact<T>(
        engine: &Engine,
        read_only: bool,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let tx = Transaction::begin(engine, read_only)?;
        match operation() {
            Ok(value) => {
                tx.commit()?;
                Ok(value)
            }
            Err(error) => match tx.abort() {
                Ok(()) => Err(error),
                Err(abort) => Err(format!("{error}|{abort}")),
            },
        }
    }
    struct EnumHandle<'a> {
        engine: &'a Engine,
        handle: HANDLE,
    }
    impl Drop for EnumHandle<'_> {
        fn drop(&mut self) {
            unsafe {
                FwpmFilterDestroyEnumHandle0(self.engine.0, self.handle);
            }
        }
    }
    struct LayerEnumHandle<'a> {
        engine: &'a Engine,
        handle: HANDLE,
    }
    impl Drop for LayerEnumHandle<'_> {
        fn drop(&mut self) {
            unsafe {
                FwpmLayerDestroyEnumHandle0(self.engine.0, self.handle);
            }
        }
    }

    fn all_layer_keys(engine: &Engine) -> Result<Vec<GUID>, String> {
        let mut handle = ptr::null_mut();
        // Microsoft documents NULL as all layers. The caller's explicit
        // transaction covers both this snapshot and every filter enumerator.
        ok(
            unsafe { FwpmLayerCreateEnumHandle0(engine.0, ptr::null(), &mut handle) },
            "WFP_LAYER_ENUM_BEGIN",
        )?;
        if handle.is_null() {
            return Err("WFP_LAYER_ENUM_HANDLE".into());
        }
        let handle = LayerEnumHandle { engine, handle };
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        let mut total = 0;
        loop {
            let mut entries = ptr::null_mut();
            let mut count = 0;
            let code = unsafe {
                FwpmLayerEnum0(
                    engine.0,
                    handle.handle,
                    ENUM_PAGE_SIZE,
                    &mut entries,
                    &mut count,
                )
            };
            let entries = WfpMem(entries);
            ok(code, "WFP_LAYER_ENUM")?;
            if count > ENUM_PAGE_SIZE || count != 0 && entries.0.is_null() {
                return Err("WFP_LAYER_ENUM_INVALID".into());
            }
            charge_enumeration(
                &mut total,
                count,
                MAX_ENUMERATED_LAYERS,
                "WFP_LAYER_ENUM_BOUND",
            )?;
            let chunk = if count == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(entries.0, count as usize) }
            };
            for p in chunk {
                if p.is_null() {
                    return Err("WFP_LAYER_ENUM_INVALID".into());
                }
                let key = unsafe { (**p).layerKey };
                // Never submit a zero/default layer GUID to the filter API.
                filter_enum_template(key)?;
                if !seen.insert((key.data1, key.data2, key.data3, key.data4)) {
                    return Err("WFP_LAYER_ENUM_DUPLICATE".into());
                }
                keys.push(key);
            }
            if count == 0 {
                break;
            }
        }
        if keys.is_empty() {
            return Err("WFP_LAYER_ENUM_EMPTY".into());
        }
        Ok(keys)
    }

    // WFP-returned pointers are trusted API allocations, but bounds/nulls are
    // checked before every dereference. No supplied JSON pointer reaches FFI.
    unsafe fn blob_bytes<'a>(blob: &FWP_BYTE_BLOB, max: usize) -> Result<&'a [u8], String> {
        if blob.size as usize > max || (blob.size != 0 && blob.data.is_null()) {
            return Err("WFP_BLOB_INVALID".into());
        }
        if blob.size == 0 {
            Ok(&[])
        } else {
            Ok(unsafe { std::slice::from_raw_parts(blob.data, blob.size as usize) })
        }
    }
    fn blob(bytes: &[u8]) -> FWP_BYTE_BLOB {
        FWP_BYTE_BLOB {
            size: bytes.len() as u32,
            data: bytes.as_ptr().cast_mut(),
        }
    }
    fn descriptor_safe(sd: PSECURITY_DESCRIPTOR) -> Result<(), String> {
        unsafe {
            if sd.is_null() {
                return Err("WFP_OBJECT_SECURITY".into());
            }
            let mut owner = ptr::null_mut();
            let mut defaulted = 0;
            let mut present = 0;
            let mut acl = ptr::null_mut();
            let mut control = 0;
            let mut revision = 0;
            if GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted) == 0
                || owner.is_null()
                || IsWellKnownSid(owner, WinLocalSystemSid) == 0
                || defaulted != 0
                || GetSecurityDescriptorControl(sd, &mut control, &mut revision) == 0
                || control & SE_DACL_PROTECTED == 0
                || GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0
                || present == 0
                || acl.is_null()
                || defaulted != 0
            {
                return Err("WFP_OBJECT_SECURITY".into());
            }
            let mut info = ACL_SIZE_INFORMATION::default();
            if GetAclInformation(
                acl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                mem::size_of_val(&info) as u32,
                AclSizeInformation,
            ) == 0
                || info.AceCount != 2
            {
                return Err("WFP_OBJECT_SECURITY".into());
            }
            let mut system = false;
            let mut admin = false;
            for i in 0..info.AceCount {
                let mut ace = ptr::null_mut();
                if GetAce(acl, i, &mut ace) == 0 || ace.is_null() {
                    return Err("WFP_OBJECT_SECURITY".into());
                }
                let a = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                if a.Header.AceType != 0
                    || a.Header.AceFlags != 0
                    || a.Mask == 0
                    || a.Header.AceSize < mem::size_of::<ACCESS_ALLOWED_ACE>() as u16
                {
                    return Err("WFP_OBJECT_SECURITY".into());
                }
                let sid = (&a.SidStart as *const u32).cast_mut().cast();
                if IsWellKnownSid(sid, WinLocalSystemSid) != 0 && !system {
                    system = true;
                } else if IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0 && !admin {
                    admin = true;
                } else {
                    return Err("WFP_OBJECT_SECURITY".into());
                }
            }
            if !system || !admin {
                return Err("WFP_OBJECT_SECURITY".into());
            }
        }
        Ok(())
    }
    enum Object {
        Provider,
        Sublayer,
        Filter,
    }
    fn verify_security(engine: &Engine, key: &GUID, kind: Object) -> Result<(), String> {
        let mut sd = ptr::null_mut();
        let flags = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let code = unsafe {
            match kind {
                Object::Provider => FwpmProviderGetSecurityInfoByKey0(
                    engine.0,
                    key,
                    flags,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut sd,
                ),
                Object::Sublayer => FwpmSubLayerGetSecurityInfoByKey0(
                    engine.0,
                    key,
                    flags,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut sd,
                ),
                Object::Filter => FwpmFilterGetSecurityInfoByKey0(
                    engine.0,
                    key,
                    flags,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut sd,
                ),
            }
        };
        let allocation = WfpMem(sd);
        ok(code, "WFP_OBJECT_SECURITY_READ")?;
        descriptor_safe(allocation.0)
    }

    struct Materials {
        app: Box<FWP_BYTE_BLOB>,
        _app_bytes: Vec<u8>,
        user_sd: LocalSd,
    }
    impl Materials {
        fn new(plan: &Plan) -> Result<Self, String> {
            let mut app = ptr::null_mut();
            let w = wide(&plan.core_executable);
            let code = unsafe { FwpmGetAppIdFromFileName0(w.as_ptr(), &mut app) };
            let app = WfpMem(app);
            ok(code, "WFP_CORE_APP_ID")?;
            if app.0.is_null() {
                return Err("WFP_CORE_APP_ID".into());
            }
            let bytes = unsafe { blob_bytes(&*app.0, 8192) }?.to_vec();
            if bytes.is_empty() {
                return Err("WFP_CORE_APP_ID".into());
            }
            Self::from_app_bytes(bytes)
        }
        fn from_app_bytes(bytes: Vec<u8>) -> Result<Self, String> {
            // ALE_USER_ID performs an access check for FWP_ACTRL_MATCH_FILTER (1).
            let user_sd = LocalSd::from("O:SYG:SYD:(A;;CC;;;SY)")?;
            let app = Box::new(blob(&bytes));
            Ok(Self {
                app,
                _app_bytes: bytes,
                user_sd,
            })
        }
    }
    // Each pointer-valued condition owns a Box. Vec relocation cannot move its
    // pointee. All backing objects remain alive through the synchronous add call.
    #[allow(dead_code)]
    enum Backing {
        U64(Box<u64>),
        V4(Box<FWP_V4_ADDR_AND_MASK>),
        V6(Box<FWP_V6_ADDR_AND_MASK>),
        Blob(Box<FWP_BYTE_BLOB>),
        None,
    }
    fn condition(c: &Condition, m: &Materials) -> (FWPM_FILTER_CONDITION0, Backing) {
        let mut v = FWP_CONDITION_VALUE0::default();
        let backing = match &c.data {
            Data::U8(n) => {
                v.r#type = FWP_UINT8;
                v.Anonymous.uint8 = *n;
                Backing::None
            }
            Data::U16(n) => {
                v.r#type = FWP_UINT16;
                v.Anonymous.uint16 = *n;
                Backing::None
            }
            Data::U32(n) => {
                v.r#type = FWP_UINT32;
                v.Anonymous.uint32 = *n;
                Backing::None
            }
            Data::U64(n) => {
                let mut b = Box::new(*n);
                v.r#type = FWP_UINT64;
                v.Anonymous.uint64 = &mut *b;
                Backing::U64(b)
            }
            Data::Address(IpAddr::V4(a), prefix) => {
                let mask = if *prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - *prefix)
                };
                let mut b = Box::new(FWP_V4_ADDR_AND_MASK {
                    addr: u32::from_be_bytes(a.octets()),
                    mask,
                });
                v.r#type = FWP_V4_ADDR_MASK;
                v.Anonymous.v4AddrMask = &mut *b;
                Backing::V4(b)
            }
            Data::Address(IpAddr::V6(a), prefix) => {
                let mut b = Box::new(FWP_V6_ADDR_AND_MASK {
                    addr: a.octets(),
                    prefixLength: *prefix,
                });
                v.r#type = FWP_V6_ADDR_MASK;
                v.Anonymous.v6AddrMask = &mut *b;
                Backing::V6(b)
            }
            Data::App => {
                v.r#type = FWP_BYTE_BLOB_TYPE;
                v.Anonymous.byteBlob = (&*m.app as *const FWP_BYTE_BLOB).cast_mut();
                Backing::None
            }
            Data::System => {
                let mut b = Box::new(FWP_BYTE_BLOB {
                    size: m.user_sd.1,
                    data: m.user_sd.0.cast(),
                });
                v.r#type = FWP_SECURITY_DESCRIPTOR_TYPE;
                v.Anonymous.sd = &mut *b;
                Backing::Blob(b)
            }
        };
        (
            FWPM_FILTER_CONDITION0 {
                fieldKey: c.field.key(),
                matchType: if c.flags_match {
                    FWP_MATCH_FLAGS_ALL_SET
                } else {
                    FWP_MATCH_EQUAL
                },
                conditionValue: v,
            },
            backing,
        )
    }
    fn with_filter<T>(
        rule: &Rule,
        m: &Materials,
        operation: impl FnOnce(&FWPM_FILTER0) -> Result<T, String>,
    ) -> Result<T, String> {
        if rule.weight > 15 {
            return Err("WFP_FILTER_WEIGHT_RANGE".into());
        }
        let mut provider = PROVIDER;
        let marker = rule.marker();
        let name = wide("Mukhomor traffic guard");
        let built: Vec<_> = rule.conditions.iter().map(|c| condition(c, m)).collect();
        let mut conditions: Vec<_> = built.iter().map(|(c, _)| *c).collect();
        let f = FWPM_FILTER0 {
            filterKey: rule.key(),
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_ptr().cast_mut(),
                description: ptr::null_mut(),
            },
            flags: rule.flags(),
            providerKey: &mut provider,
            providerData: blob(&marker),
            layerKey: rule.layer.key(),
            subLayerKey: SUBLAYER,
            weight: FWP_VALUE0 {
                r#type: FWP_UINT8,
                Anonymous: FWP_VALUE0_0 { uint8: rule.weight },
            },
            numFilterConditions: conditions.len() as u32,
            filterCondition: if conditions.is_empty() {
                ptr::null_mut()
            } else {
                conditions.as_mut_ptr()
            },
            action: FWPM_ACTION0 {
                r#type: if rule.permit {
                    FWP_ACTION_PERMIT
                } else {
                    FWP_ACTION_BLOCK
                },
                ..Default::default()
            },
            ..Default::default()
        };
        operation(&f)
    }
    fn add_rule(engine: &Engine, rule: &Rule, m: &Materials, sd: &LocalSd) -> Result<(), String> {
        with_filter(rule, m, |f| {
            ok(
                unsafe { FwpmFilterAdd0(engine.0, f, sd.0, ptr::null_mut()) },
                "WFP_FILTER_ADD",
            )
        })
    }
    unsafe fn condition_matches(
        actual: &FWPM_FILTER_CONDITION0,
        expected: &Condition,
        m: &Materials,
    ) -> Result<bool, String> {
        if !guid_eq(&actual.fieldKey, &expected.field.key())
            || actual.matchType
                != if expected.flags_match {
                    FWP_MATCH_FLAGS_ALL_SET
                } else {
                    FWP_MATCH_EQUAL
                }
        {
            return Ok(false);
        }
        let v = &actual.conditionValue;
        Ok(unsafe {
            match &expected.data {
                Data::U8(n) => v.r#type == FWP_UINT8 && v.Anonymous.uint8 == *n,
                Data::U16(n) => v.r#type == FWP_UINT16 && v.Anonymous.uint16 == *n,
                Data::U32(n) => v.r#type == FWP_UINT32 && v.Anonymous.uint32 == *n,
                Data::U64(n) => {
                    v.r#type == FWP_UINT64
                        && !v.Anonymous.uint64.is_null()
                        && *v.Anonymous.uint64 == *n
                }
                Data::Address(IpAddr::V4(a), prefix) => {
                    v.r#type == FWP_V4_ADDR_MASK
                        && !v.Anonymous.v4AddrMask.is_null()
                        && (*v.Anonymous.v4AddrMask).addr == u32::from_be_bytes(a.octets())
                        && (*v.Anonymous.v4AddrMask).mask
                            == if *prefix == 0 {
                                0
                            } else {
                                u32::MAX << (32 - *prefix)
                            }
                }
                Data::Address(IpAddr::V6(a), prefix) => {
                    v.r#type == FWP_V6_ADDR_MASK
                        && !v.Anonymous.v6AddrMask.is_null()
                        && (*v.Anonymous.v6AddrMask).addr == a.octets()
                        && (*v.Anonymous.v6AddrMask).prefixLength == *prefix
                }
                Data::App => {
                    v.r#type == FWP_BYTE_BLOB_TYPE
                        && !v.Anonymous.byteBlob.is_null()
                        && blob_bytes(&*v.Anonymous.byteBlob, 8192)? == blob_bytes(&m.app, 8192)?
                }
                Data::System => {
                    v.r#type == FWP_SECURITY_DESCRIPTOR_TYPE
                        && !v.Anonymous.sd.is_null()
                        && blob_bytes(&*v.Anonymous.sd, 4096)?
                            == std::slice::from_raw_parts(
                                m.user_sd.0.cast::<u8>(),
                                m.user_sd.1 as usize,
                            )
                }
            }
        })
    }
    // Identify the first rejected field without exporting its value. The exact
    // comparisons remain fail-closed, allowing only documented BFE bookkeeping.
    fn filter_mismatch(
        actual: &FWPM_FILTER0,
        rule: &Rule,
        m: &Materials,
    ) -> Result<Option<&'static str>, String> {
        unsafe {
            // BFE marks stored boot filters disabled after initialization; runtime
            // DISABLED is always a failure. INDEXED only accelerates lookup;
            // BFE may add it automatically without changing the filter action.
            let flags = if rule.layer.boot() {
                actual.flags & !(FWPM_FILTER_FLAG_DISABLED | FWPM_FILTER_FLAG_INDEXED)
            } else {
                actual.flags & !FWPM_FILTER_FLAG_INDEXED
            };
            if !guid_eq(&actual.filterKey, &rule.key()) {
                return Ok(Some("WFP_FILTER_MISMATCH_KEY"));
            }
            if flags != rule.flags() {
                return Ok(Some("WFP_FILTER_MISMATCH_FLAGS"));
            }
            if actual.providerKey.is_null() || !guid_eq(&*actual.providerKey, &PROVIDER) {
                return Ok(Some("WFP_FILTER_MISMATCH_PROVIDER"));
            }
            if !guid_eq(&actual.subLayerKey, &SUBLAYER) {
                return Ok(Some("WFP_FILTER_MISMATCH_SUBLAYER"));
            }
            if !guid_eq(&actual.layerKey, &rule.layer.key()) {
                return Ok(Some("WFP_FILTER_MISMATCH_LAYER"));
            }
            if actual.weight.r#type != FWP_UINT8 {
                return Ok(Some("WFP_FILTER_MISMATCH_WEIGHT_TYPE"));
            }
            if actual.weight.Anonymous.uint8 != rule.weight {
                return Ok(Some("WFP_FILTER_MISMATCH_WEIGHT_VALUE"));
            }
            if actual.action.r#type
                != if rule.permit {
                    FWP_ACTION_PERMIT
                } else {
                    FWP_ACTION_BLOCK
                }
            {
                return Ok(Some("WFP_FILTER_MISMATCH_ACTION"));
            }
            if actual.Anonymous.rawContext != 0 {
                return Ok(Some("WFP_FILTER_MISMATCH_CONTEXT"));
            }
            if actual.numFilterConditions as usize != rule.conditions.len() {
                return Ok(Some("WFP_FILTER_MISMATCH_CONDITION_COUNT"));
            }
            if blob_bytes(&actual.providerData, 128)? != rule.marker() {
                return Ok(Some("WFP_FILTER_MISMATCH_PROVIDER_DATA"));
            }
            if actual.numFilterConditions != 0 && actual.filterCondition.is_null() {
                return Ok(Some("WFP_FILTER_MISMATCH_CONDITION_POINTER"));
            }
            let cs = if actual.numFilterConditions == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(
                    actual.filterCondition,
                    actual.numFilterConditions as usize,
                )
            };
            // BFE may reorder conditions. Match as a multiset, never accept extra.
            let mut seen = vec![false; cs.len()];
            for expected in &rule.conditions {
                let mut found = false;
                for (i, c) in cs.iter().enumerate() {
                    if !seen[i] && condition_matches(c, expected, m)? {
                        seen[i] = true;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Ok(Some("WFP_FILTER_MISMATCH_CONDITION_MULTISET"));
                }
            }
        }
        Ok(None)
    }
    #[cfg(test)]
    fn filter_matches(actual: &FWPM_FILTER0, rule: &Rule, m: &Materials) -> Result<bool, String> {
        Ok(filter_mismatch(actual, rule, m)?.is_none())
    }
    // Error-only diagnostics use raw flags and the trusted expected rule. L is
    // the EXPECTED layer: flags are checked before the stored layer comparison.
    // No pointer, object identity, address or policy material is formatted.
    fn flags_mismatch_error(actual_flags: u32, rule: &Rule) -> String {
        if !(1..=MAX_FILTERS as u16).contains(&rule.id) {
            return "WFP_FILTER_MISMATCH_FLAGS".into();
        }
        let expected = rule.flags();
        let layer = match rule.layer {
            Layer::Connect4 => "CONNECT4",
            Layer::Connect6 => "CONNECT6",
            Layer::Accept4 => "ACCEPT4",
            Layer::Accept6 => "ACCEPT6",
            Layer::BootIn4 => "BOOTIN4",
            Layer::BootIn6 => "BOOTIN6",
            Layer::BootOut4 => "BOOTOUT4",
            Layer::BootOut6 => "BOOTOUT6",
        };
        let detail = format!(
            "WFP_FILTER_MISMATCH_FLAGS:E={expected:08x}:A={actual_flags:08x}:X={:08x}:R={:03}:B={}:L={layer}",
            actual_flags ^ expected,
            rule.id,
            rule.layer.boot(),
        );
        if detail.len() >= 128 || !detail.is_ascii() {
            return "WFP_FILTER_MISMATCH_FLAGS".into();
        }
        detail
    }
    fn own_filters(
        engine: &Engine,
        expected: &[Rule],
        m: Option<&Materials>,
        priority: Option<u16>,
    ) -> Result<Vec<GUID>, String> {
        let mut found = Vec::new();
        let mut total = 0usize;
        let mut foreign_weights: Vec<(GUID, u16)> = Vec::new();
        // A non-NULL filter template requires an actual layer GUID. Enumerate
        // every BFE layer, not just policy layers: restricting the scan would
        // hide orphaned/foreign-shaped filters with our provider, key or sublayer.
        // NULL filter templates are not used because boot/disabled inclusion
        // must remain explicit. Both snapshots are in the existing transaction.
        for layer in all_layer_keys(engine)? {
            let template = filter_enum_template(layer)?;
            let mut handle = ptr::null_mut();
            ok(
                unsafe { FwpmFilterCreateEnumHandle0(engine.0, &template, &mut handle) },
                "WFP_FILTER_ENUM_BEGIN",
            )?;
            if handle.is_null() {
                return Err("WFP_FILTER_ENUM_HANDLE".into());
            }
            let handle = EnumHandle { engine, handle };
            loop {
                let mut entries = ptr::null_mut();
                let mut count = 0;
                let code = unsafe {
                    FwpmFilterEnum0(
                        engine.0,
                        handle.handle,
                        ENUM_PAGE_SIZE,
                        &mut entries,
                        &mut count,
                    )
                };
                let entries = WfpMem(entries);
                ok(code, "WFP_FILTER_ENUM")?;
                if count > ENUM_PAGE_SIZE || count != 0 && entries.0.is_null() {
                    return Err("WFP_FILTER_ENUM_INVALID".into());
                }
                // The budget is cumulative across all layers and includes
                // unrelated filters. Exhaustion never becomes an absence proof.
                charge_enumeration(
                    &mut total,
                    count,
                    MAX_ENUMERATED_FILTERS,
                    "WFP_FILTER_ENUM_BOUND",
                )?;
                let chunk = if count == 0 {
                    &[]
                } else {
                    unsafe { std::slice::from_raw_parts(entries.0, count as usize) }
                };
                for p in chunk {
                    if p.is_null() {
                        return Err("WFP_FILTER_ENUM_INVALID".into());
                    }
                    let f = unsafe { &**p };
                    let provider = if f.providerKey.is_null() {
                        None
                    } else {
                        Some(unsafe { &*f.providerKey })
                    };
                    let Some(rule) =
                        owned_filter_rule(&f.filterKey, &f.subLayerKey, provider, expected)?
                    else {
                        if let Some(ours) = priority {
                            if can_preempt_guard(f, expected) {
                                let weight = if let Some((_, weight)) = foreign_weights.iter()
                                    .find(|(key, _)| guid_eq(key, &f.subLayerKey)) {
                                    *weight
                                } else {
                                    let mut raw = ptr::null_mut();
                                    ok(unsafe { FwpmSubLayerGetByKey0(engine.0, &f.subLayerKey, &mut raw) }, "WFP_PRIORITY_READ")?;
                                    let raw = WfpMem(raw);
                                    if raw.0.is_null() { return Err("WFP_PRIORITY_INVALID".into()); }
                                    let weight = unsafe { (*raw.0).weight };
                                    foreign_weights.push((f.subLayerKey, weight));
                                    weight
                                };
                                if weight >= ours {
                                    return Err("WFP_HIGHER_PRIORITY_PERMIT".into());
                                }
                            }
                        }
                        continue;
                    };
                    if found.iter().any(|k| guid_eq(k, &f.filterKey)) {
                        return Err("WFP_FILTER_MISMATCH_DUPLICATE".into());
                    }
                    if let Some(category) = filter_mismatch(f, rule, m.ok_or("WFP_ORPHAN_FILTER")?)?
                    {
                        if category == "WFP_FILTER_MISMATCH_FLAGS" {
                            return Err(flags_mismatch_error(f.flags, rule));
                        }
                        return Err(category.into());
                    }
                    verify_security(engine, &f.filterKey, Object::Filter)?;
                    found.push(f.filterKey);
                }
                if count == 0 {
                    break;
                }
            }
        }
        if found.len() != expected.len() {
            return Err("WFP_FILTER_MISSING".into());
        }
        Ok(found)
    }
    // A foreign hard permit can prevent a lower sublayer's block from taking
    // effect. Never declare protection just because BFE accepted a lower weight.
    // A terminating/unknown callout can clear FWPS_RIGHT_ACTION_WRITE at run
    // time, even without CLEAR_ACTION_RIGHT on its management-plane filter.
    // Its eventual action cannot be proven safe from an FWPM_FILTER0 snapshot.
    fn can_preempt_guard(filter: &FWPM_FILTER0, expected: &[Rule]) -> bool {
        filter.flags & (FWPM_FILTER_FLAG_DISABLED | FWPM_FILTER_FLAG_BOOTTIME) == 0
            && (matches!(filter.action.r#type,
                FWP_ACTION_CALLOUT_TERMINATING | FWP_ACTION_CALLOUT_UNKNOWN)
                || (filter.action.r#type == FWP_ACTION_PERMIT
                    && filter.flags & FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT != 0))
            && expected.iter().any(|r| !r.layer.boot() && guid_eq(&r.layer.key(), &filter.layerKey))
    }
    fn sublayer_matches(s: &FWPM_SUBLAYER0) -> Result<bool, String> {
        Ok(guid_eq(&s.subLayerKey, &SUBLAYER)
            && s.flags == FWPM_SUBLAYER_FLAG_PERSISTENT
            && !s.providerKey.is_null()
            && unsafe { guid_eq(&*s.providerKey, &PROVIDER) && blob_bytes(&s.providerData, 128)? == MAGIC })
    }
    fn stored_plan(engine: &Engine, check_priority: bool) -> Result<Option<Plan>, String> {
        let mut p = ptr::null_mut();
        let code = unsafe { FwpmProviderGetByKey0(engine.0, &PROVIDER, &mut p) };
        let p = WfpMem(p);
        if code == FWP_E_PROVIDER_NOT_FOUND as u32 {
            let mut s = ptr::null_mut();
            let code = unsafe { FwpmSubLayerGetByKey0(engine.0, &SUBLAYER, &mut s) };
            let _s = WfpMem(s);
            if code != FWP_E_SUBLAYER_NOT_FOUND as u32 {
                return Err("WFP_ORPHAN_SUBLAYER".into());
            }
            own_filters(engine, &[], None, None)?;
            return Ok(None);
        }
        ok(code, "WFP_PROVIDER_READ")?;
        if p.0.is_null() {
            return Err("WFP_PROVIDER_INVALID".into());
        }
        let p = unsafe { &*p.0 };
        if !guid_eq(&p.providerKey, &PROVIDER)
            || p.flags != FWPM_PROVIDER_FLAG_PERSISTENT
            || !p.serviceName.is_null()
        {
            return Err("WFP_PROVIDER_MISMATCH".into());
        }
        let data = unsafe { blob_bytes(&p.providerData, 8192) }?;
        let tail = data.strip_prefix(MAGIC).ok_or("WFP_PROVIDER_IDENTITY")?;
        let value: Value = serde_json::from_slice(tail).map_err(|_| "WFP_PROVIDER_PLAN")?;
        let plan = Plan::parse(&value)?;
        if plan.marker() != data {
            return Err("WFP_PROVIDER_PLAN_CANONICAL".into());
        }
        verify_security(engine, &PROVIDER, Object::Provider)?;
        let mut s = ptr::null_mut();
        let code = unsafe { FwpmSubLayerGetByKey0(engine.0, &SUBLAYER, &mut s) };
        let s = WfpMem(s);
        ok(code, "WFP_SUBLAYER_READ")?;
        if s.0.is_null() {
            return Err("WFP_SUBLAYER_INVALID".into());
        }
        let s = unsafe { &*s.0 };
        if !sublayer_matches(s)? {
            return Err("WFP_SUBLAYER_MISMATCH".into());
        }
        verify_security(engine, &SUBLAYER, Object::Sublayer)?;
        let materials = Materials::new(&plan)?;
        own_filters(engine, &rules(&plan), Some(&materials), check_priority.then_some(s.weight))?;
        Ok(Some(plan))
    }
    fn add_policy(engine: &Engine, plan: &Plan) -> Result<(), String> {
        let sd = LocalSd::from("O:SYG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
        let marker = plan.marker();
        let name = wide("Mukhomor traffic guard");
        let provider = FWPM_PROVIDER0 {
            providerKey: PROVIDER,
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_ptr().cast_mut(),
                description: ptr::null_mut(),
            },
            flags: FWPM_PROVIDER_FLAG_PERSISTENT,
            providerData: blob(&marker),
            serviceName: ptr::null_mut(),
        };
        ok(
            unsafe { FwpmProviderAdd0(engine.0, &provider, sd.0) },
            "WFP_PROVIDER_ADD",
        )?;
        let mut key = PROVIDER;
        let sublayer = FWPM_SUBLAYER0 {
            subLayerKey: SUBLAYER,
            displayData: provider.displayData,
            flags: FWPM_SUBLAYER_FLAG_PERSISTENT,
            providerKey: &mut key,
            providerData: blob(MAGIC),
            weight: SUBLAYER_WEIGHT,
        };
        ok(
            unsafe { FwpmSubLayerAdd0(engine.0, &sublayer, sd.0) },
            "WFP_SUBLAYER_ADD",
        )?;
        let m = Materials::new(plan)?;
        for r in rules(plan) {
            add_rule(engine, &r, &m, &sd)?;
        }
        Ok(())
    }
    fn remove_policy(engine: &Engine, plan: &Plan) -> Result<(), String> {
        // Caller already verified every object under this same write transaction.
        for r in rules(plan) {
            ok(
                unsafe { FwpmFilterDeleteByKey0(engine.0, &r.key()) },
                "WFP_FILTER_DELETE",
            )?;
        }
        ok(
            unsafe { FwpmSubLayerDeleteByKey0(engine.0, &SUBLAYER) },
            "WFP_SUBLAYER_DELETE",
        )?;
        ok(
            unsafe { FwpmProviderDeleteByKey0(engine.0, &PROVIDER) },
            "WFP_PROVIDER_DELETE",
        )
    }
    pub fn inspect() -> Result<Value, String> {
        // Discovery returns the private saved executable/endpoint policy. It is
        // restricted to the service identity even though no object is mutated.
        require_system()?;
        let e = Engine::open()?;
        let stored = transact(&e, true, || stored_plan(&e, true))?;
        Ok(inspection_result(stored.as_ref()))
    }
    pub fn status(plan: &Plan) -> Result<Value, String> {
        let e = Engine::open()?;
        let stored = transact(&e, true, || stored_plan(&e, true))?;
        let matches = stored.as_ref() == Some(plan);
        Ok(
            json!({"schema":1,"action":"status","armed":stored.is_some(),"verified":matches,
            "protected":matches&&plan.tunnel_luid!=0,"bootstrap":plan.tunnel_luid==0,
            "persistent":stored.is_some(),"boot_policy_verified":matches,"runtime_policy_verified":matches,
            "policy_matches_request":matches,"filter_count":stored.as_ref().map(|p|rules(p).len()).unwrap_or(0),
            "tunnel_identity_requires_service_validation":true}),
        )
    }
    pub fn arm(plan: &Plan) -> Result<Value, String> {
        require_system()?;
        let e = Engine::open()?;
        transact(&e, false, || {
            if let Some(old) = stored_plan(&e, false)? {
                if old != *plan {
                    remove_policy(&e, &old)?;
                    add_policy(&e, plan)?;
                }
            } else {
                add_policy(&e, plan)?;
            }
            if stored_plan(&e, true)?.as_ref() != Some(plan) {
                return Err("WFP_PRECOMMIT_VERIFICATION".into());
            }
            Ok(())
        })?;
        let mut result = status(plan)?;
        result["action"] = json!("arm");
        Ok(result)
    }
    pub fn disarm() -> Result<Value, String> {
        require_system()?;
        let e = Engine::open()?;
        transact(&e, false, || {
            // Foreign conflicts must not prevent explicit recovery. Ownership,
            // object ACLs and every own filter remain strictly verified.
            if let Some(old) = stored_plan(&e, false)? {
                remove_policy(&e, &old)?;
            }
            if stored_plan(&e, false)?.is_some() {
                return Err("WFP_DISARM_VERIFICATION".into());
            }
            Ok(())
        })?;
        Ok(json!({"schema":1,"action":"disarm","armed":false,"verified":true,"protected":false}))
    }
    #[cfg(test)]
    mod memory_tests {
        use super::*;
        #[test]
        fn bfe_assigned_sublayer_weight_preserves_strict_identity() {
            let mut provider = PROVIDER;
            let base = FWPM_SUBLAYER0 {
                subLayerKey: SUBLAYER, flags: FWPM_SUBLAYER_FLAG_PERSISTENT,
                providerKey: &mut provider, providerData: blob(MAGIC), weight: 65532,
                ..Default::default()
            };
            for weight in [0, 32768, 65532, 65535] {
                assert!(sublayer_matches(&FWPM_SUBLAYER0 { weight, ..base }).unwrap());
            }
            assert!(!sublayer_matches(&FWPM_SUBLAYER0 { flags: 0, ..base }).unwrap());
            assert!(!sublayer_matches(&FWPM_SUBLAYER0 { flags: 3, ..base }).unwrap());
            assert!(!sublayer_matches(&FWPM_SUBLAYER0 { subLayerKey: PROVIDER, ..base }).unwrap());
            assert!(!sublayer_matches(&FWPM_SUBLAYER0 { providerKey: ptr::null_mut(), ..base }).unwrap());
            assert!(!sublayer_matches(&FWPM_SUBLAYER0 { providerData: blob(b"foreign"), ..base }).unwrap());
            provider = SUBLAYER;
            assert!(!sublayer_matches(&base).unwrap());
            assert!(guid_eq(&provider, &SUBLAYER));
        }
        #[test]
        fn foreign_hard_permits_cannot_silently_preempt_the_guard() {
            let expected = rules(&Plan::parse(&super::super::tests::fixture()).unwrap());
            let mut filter = FWPM_FILTER0 {
                layerKey: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
                flags: FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT,
                action: FWPM_ACTION0 { r#type: FWP_ACTION_PERMIT, ..Default::default() },
                ..Default::default()
            };
            assert!(can_preempt_guard(&filter, &expected));
            for action in [FWP_ACTION_CALLOUT_TERMINATING, FWP_ACTION_CALLOUT_UNKNOWN] {
                filter.action.r#type = action;
                filter.flags = FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
                assert!(can_preempt_guard(&filter, &expected));
                filter.flags = 0;
                assert!(can_preempt_guard(&filter, &expected));
                for excluded in [FWPM_FILTER_FLAG_DISABLED, FWPM_FILTER_FLAG_BOOTTIME] {
                    filter.flags = excluded;
                    assert!(!can_preempt_guard(&filter, &expected));
                }
            }
            filter.flags = FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
            filter.action.r#type = FWP_ACTION_BLOCK;
            assert!(!can_preempt_guard(&filter, &expected));
            filter.action.r#type = FWP_ACTION_CALLOUT_INSPECTION;
            assert!(!can_preempt_guard(&filter, &expected));
            filter.action.r#type = FWP_ACTION_PERMIT;
            filter.flags = 0;
            assert!(!can_preempt_guard(&filter, &expected));
            for excluded in [FWPM_FILTER_FLAG_DISABLED, FWPM_FILTER_FLAG_BOOTTIME] {
                filter.flags = FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT | excluded;
                assert!(!can_preempt_guard(&filter, &expected));
            }
            filter.flags = FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
            filter.layerKey = GUID::from_u128(0);
            assert!(!can_preempt_guard(&filter, &expected));
        }
        #[test]
        fn bfe_indexing_does_not_relax_action_or_disabled_checks() {
            let plan = Plan::parse(&super::super::tests::fixture()).unwrap();
            let materials = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            for rule in rules(&plan) {
                with_filter(&rule, &materials, |filter| {
                    let mut indexed = *filter;
                    indexed.flags |= FWPM_FILTER_FLAG_INDEXED;
                    assert_eq!(filter_mismatch(&indexed, &rule, &materials)?, None);
                    indexed.flags |= FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
                    assert_eq!(filter_mismatch(&indexed, &rule, &materials)?, Some("WFP_FILTER_MISMATCH_FLAGS"));
                    indexed.flags = filter.flags | FWPM_FILTER_FLAG_INDEXED | FWPM_FILTER_FLAG_DISABLED;
                    assert_eq!(filter_mismatch(&indexed, &rule, &materials)?, if rule.layer.boot() { None } else { Some("WFP_FILTER_MISMATCH_FLAGS") });
                    Ok(())
                }).unwrap();
            }
        }
        #[test]
        #[ignore = "explicit SYSTEM-only BFE readback; transaction is always aborted, never activates filters"]
        fn live_bfe_policy_readback_and_rollback() {
            require_system().unwrap();
            let engine = Engine::open().unwrap();
            let tx = Transaction::begin(&engine, false).unwrap();
            assert!(stored_plan(&engine, false).unwrap().is_none(), "Disconnect Mukhomor first");
            let mut value = super::super::tests::fixture();
            value["core_executable"] = json!(std::env::current_exe().unwrap().to_string_lossy());
            for transport in ["vps", "tor", "tor-bridges", "tor-snowflake"] {
                if transport!="vps" {
                    value["schema"] = json!(2);
                    value["transport"] = json!(transport);
                    value["endpoints"] = json!([]);
                    value["allow_private_lan"] = json!(false);
                }
                for luid in [0, 42] {
                    value["tunnel_luid"] = json!(luid);
                    let plan = Plan::parse(&value).unwrap();
                    add_policy(&engine, &plan).unwrap();
                    assert_eq!(stored_plan(&engine, false).unwrap(), Some(plan.clone()));
                    match stored_plan(&engine, true) {
                        Ok(actual) => { assert_eq!(actual, Some(plan)); println!("BFE priority: no overriding foreign permit"); }
                        Err(error) => { assert_eq!(error, "WFP_HIGHER_PRIORITY_PERMIT"); println!("BFE priority: conflicting foreign permit correctly rejected"); }
                    }
                    // Recovery still verifies ownership and can remove only our policy.
                    remove_policy(&engine, &stored_plan(&engine, false).unwrap().unwrap()).unwrap();
                    assert!(stored_plan(&engine, false).unwrap().is_none());
                    println!("BFE readback: transport={transport}, tunnel={}", luid != 0);
                }
            }
            tx.abort().unwrap();
            assert!(transact(&engine, true, || stored_plan(&engine, false)).unwrap().is_none());
            println!("PASS: real BFE policy, strict readback, recovery and transaction rollback");
        }
        #[test]
        fn flags_error_is_bounded_raw_and_preserves_strict_refusal() {
            let mut rule = Rule {
                id: 128,
                layer: Layer::Connect4,
                permit: false,
                weight: WEIGHT_DEFAULT_BLOCK,
                conditions: vec![],
            };
            let largest = flags_mismatch_error(u32::MAX, &rule);
            assert_eq!(
                largest,
                "WFP_FILTER_MISMATCH_FLAGS:E=00000001:A=ffffffff:X=fffffffe:R=128:B=false:L=CONNECT4"
            );
            // Exercise the existing main.rs error JSON shape without using an
            // executable, stream, engine or raw stdout/stderr transport.
            let wire = json!({"ok": false, "error": &largest}).to_string();
            let decoded: Value = serde_json::from_str(&wire).unwrap();
            assert_eq!(decoded["ok"], false);
            assert_eq!(decoded["error"], largest);
            let layers = [
                (Layer::Connect4, "CONNECT4", false),
                (Layer::Connect6, "CONNECT6", false),
                (Layer::Accept4, "ACCEPT4", false),
                (Layer::Accept6, "ACCEPT6", false),
                (Layer::BootIn4, "BOOTIN4", true),
                (Layer::BootIn6, "BOOTIN6", true),
                (Layer::BootOut4, "BOOTOUT4", true),
                (Layer::BootOut6, "BOOTOUT6", true),
            ];
            for (layer, label, boot) in layers {
                rule.layer = layer;
                let detail = flags_mismatch_error(u32::MAX, &rule);
                assert!(detail.is_ascii() && detail.len() < 128);
                assert!(detail.ends_with(&format!(":B={boot}:L={label}")));
                assert_eq!(detail.split(':').count(), 7);
                for id in [0, 129] {
                    rule.id = id;
                    assert_eq!(
                        flags_mismatch_error(u32::MAX, &rule),
                        "WFP_FILTER_MISMATCH_FLAGS"
                    );
                }
                rule.id = 128;
            }
            // Establish a valid SDK-shaped filter, then reject only changed
            // flags. Boot DISABLED remains handled exactly as before; raw A/X
            // diagnostics still include it when another flag causes refusal.
            let p = Plan::parse(&super::super::tests::fixture()).unwrap();
            let material = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            let rs = rules(&p);
            for boot in [false, true] {
                let rule = rs.iter().find(|r| r.layer.boot() == boot).unwrap();
                with_filter(rule, &material, |actual| {
                    assert_eq!(filter_mismatch(actual, rule, &material)?, None);
                    let mut changed = *actual;
                    if boot {
                        changed.flags |= FWPM_FILTER_FLAG_DISABLED;
                        assert_eq!(filter_mismatch(&changed, rule, &material)?, None);
                    }
                    changed.flags |= FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
                    assert_eq!(
                        filter_mismatch(&changed, rule, &material)?,
                        Some("WFP_FILTER_MISMATCH_FLAGS")
                    );
                    assert!(!filter_matches(&changed, rule, &material)?);
                    let detail = flags_mismatch_error(changed.flags, rule);
                    if boot {
                        assert!(detail.contains(":E=00000002:A=0000002a:X=00000028:"));
                    } else {
                        assert!(detail.contains(":E=00000001:A=00000009:X=00000008:"));
                    }
                    assert!(detail.is_ascii() && detail.len() < 128);
                    Ok(())
                })
                .unwrap();
            }
        }
        #[test]
        fn ffi_weight_ranges_are_legal_and_readback_remains_exact() {
            let material = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            for lan in [false, true] {
                for luid in [0, 42] {
                    let mut value = super::super::tests::fixture();
                    value["allow_private_lan"] = json!(lan);
                    value["tunnel_luid"] = json!(luid);
                    for rule in rules(&Plan::parse(&value).unwrap()) {
                        with_filter(&rule, &material, |actual| {
                            assert_eq!(actual.weight.r#type, FWP_UINT8);
                            let weight = unsafe { actual.weight.Anonymous.uint8 };
                            assert!(weight <= 15, "BFE rejects UINT8 ranges above 15");
                            assert_eq!(weight, rule.weight);
                            assert!(filter_matches(actual, &rule, &material)?);
                            let mut changed = *actual;
                            changed.weight.r#type = FWP_EMPTY;
                            assert!(!filter_matches(&changed, &rule, &material)?);
                            let mut changed = *actual;
                            changed.weight.Anonymous.uint8 = 16;
                            assert!(!filter_matches(&changed, &rule, &material)?);
                            let mut changed = *actual;
                            changed.weight.Anonymous.uint8 = if weight == 1 { 6 } else { 1 };
                            assert!(!filter_matches(&changed, &rule, &material)?);
                            Ok(())
                        })
                        .unwrap();
                    }
                }
            }
        }
        #[test]
        fn ffi_rejects_old_invalid_range_before_calling_the_operation() {
            let material = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            let plan = Plan::parse(&super::super::tests::fixture()).unwrap();
            let mut rule = rules(&plan).remove(0);
            for weight in [16, 60, 80, 85, 90, 95, 100, 255] {
                rule.weight = weight;
                let mut called = false;
                let error = with_filter(&rule, &material, |_| {
                    called = true;
                    Ok(())
                })
                .unwrap_err();
                assert_eq!(error, "WFP_FILTER_WEIGHT_RANGE");
                assert!(!called);
            }
        }
        #[test]
        fn object_descriptor_rejects_world_null_missing_owner_or_ace() {
            let good = LocalSd::from("O:SYG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)").unwrap();
            descriptor_safe(good.0).unwrap();
            for s in [
                "O:SYG:SYD:P(A;;GA;;;WD)(A;;GA;;;BA)",
                "O:BAG:SYD:P(A;;GA;;;SY)(A;;GA;;;BA)",
                "O:SYG:SYD:P(A;;GA;;;SY)",
                "O:SYG:SYD:NO_ACCESS_CONTROL",
            ] {
                let sd = LocalSd::from(s).unwrap();
                assert!(descriptor_safe(sd.0).is_err(), "accepted {s}");
            }
        }
        #[test]
        fn ffi_mismatch_categories_preserve_exact_readback_and_condition_reordering() {
            let p = Plan::parse(&super::super::tests::fixture()).unwrap();
            let material = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            let rs = rules(&p);
            let endpoint = rs
                .iter()
                .find(|r| r.weight == WEIGHT_CORE_ENDPOINT)
                .unwrap();
            with_filter(endpoint, &material, |actual| {
                // Establish an independently valid SDK-shaped readback before
                // changing one field. This test never calls a BFE engine API.
                assert_eq!(filter_mismatch(actual, endpoint, &material)?, None);
                assert!(filter_matches(actual, endpoint, &material)?);
                let mut altered = *actual;
                altered.flags |= FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
                assert_eq!(
                    filter_mismatch(&altered, endpoint, &material)?,
                    Some("WFP_FILTER_MISMATCH_FLAGS")
                );
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let mut altered = *actual;
                altered.weight.r#type = FWP_UINT64;
                assert_eq!(
                    filter_mismatch(&altered, endpoint, &material)?,
                    Some("WFP_FILTER_MISMATCH_WEIGHT_TYPE")
                );
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let mut altered = *actual;
                altered.weight.Anonymous.uint8 = 0;
                assert_eq!(
                    filter_mismatch(&altered, endpoint, &material)?,
                    Some("WFP_FILTER_MISMATCH_WEIGHT_VALUE")
                );
                let mut conditions = unsafe {
                    std::slice::from_raw_parts(
                        actual.filterCondition,
                        actual.numFilterConditions as usize,
                    )
                }
                .to_vec();
                conditions.reverse();
                let mut altered = *actual;
                altered.filterCondition = conditions.as_mut_ptr();
                assert_eq!(filter_mismatch(&altered, endpoint, &material)?, None);
                assert!(filter_matches(&altered, endpoint, &material)?);
                let wrong_material = Materials::from_app_bytes(vec![1, 2, 3, 5]).unwrap();
                assert_eq!(
                    filter_mismatch(&altered, endpoint, &wrong_material)?,
                    Some("WFP_FILTER_MISMATCH_CONDITION_MULTISET")
                );
                assert!(!filter_matches(&altered, endpoint, &wrong_material)?);
                Ok(())
            })
            .unwrap();
            let boot = rs.iter().find(|r| r.layer.boot()).unwrap();
            with_filter(boot, &material, |actual| {
                let mut disabled = *actual;
                disabled.flags |= FWPM_FILTER_FLAG_DISABLED;
                assert_eq!(filter_mismatch(&disabled, boot, &material)?, None);
                Ok(())
            })
            .unwrap();
        }
        #[test]
        fn ffi_deep_compare_rejects_wrong_flags_action_layer_endpoint_app_and_user() {
            let p = Plan::parse(&super::super::tests::fixture()).unwrap();
            let material = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
            let rs = rules(&p);
            let endpoint = rs
                .iter()
                .find(|r| r.weight == WEIGHT_CORE_ENDPOINT)
                .unwrap();
            with_filter(endpoint, &material, |actual| {
                assert!(filter_matches(actual, endpoint, &material)?);
                let mut altered = *actual;
                altered.flags |= FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let mut altered = *actual;
                altered.flags |= FWPM_FILTER_FLAG_DISABLED;
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let mut altered = *actual;
                altered.action.r#type = FWP_ACTION_BLOCK;
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let mut altered = *actual;
                altered.layerKey = FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4;
                assert!(!filter_matches(&altered, endpoint, &material)?);
                let wrong_material = Materials::from_app_bytes(vec![1, 2, 3, 5]).unwrap();
                assert!(!filter_matches(actual, endpoint, &wrong_material)?);
                let mut wrong_user = Materials::from_app_bytes(vec![1, 2, 3, 4]).unwrap();
                wrong_user.user_sd = LocalSd::from("O:SYG:SYD:(A;;CC;;;WD)").unwrap();
                assert!(!filter_matches(actual, endpoint, &wrong_user)?);
                let mut changed = endpoint.clone();
                for condition in &mut changed.conditions {
                    if condition.field == Field::RemotePort {
                        condition.data = Data::U16(443);
                    }
                }
                assert!(!filter_matches(actual, &changed, &material)?);
                let mut changed = endpoint.clone();
                for condition in &mut changed.conditions {
                    if condition.field == Field::RemoteAddress {
                        condition.data = Data::Address("203.0.113.8".parse().unwrap(), 32);
                    }
                }
                assert!(!filter_matches(actual, &changed, &material)?);
                // Reordered SDK conditions are equivalent; missing or extra are not.
                let mut conditions = unsafe {
                    std::slice::from_raw_parts(
                        actual.filterCondition,
                        actual.numFilterConditions as usize,
                    )
                }
                .to_vec();
                conditions.reverse();
                let mut altered = *actual;
                altered.filterCondition = conditions.as_mut_ptr();
                assert!(filter_matches(&altered, endpoint, &material)?);
                altered.numFilterConditions -= 1;
                assert!(!filter_matches(&altered, endpoint, &material)?);
                Ok(())
            })
            .unwrap();
            for rule in rs {
                with_filter(&rule, &material, |f| {
                    assert!(filter_matches(f, &rule, &material)?);
                    Ok(())
                })
                .unwrap();
            }
        }
    }
}

pub fn arm(plan: &Plan) -> Result<Value, String> {
    #[cfg(windows)]
    {
        platform::arm(plan)
    }
    #[cfg(not(windows))]
    {
        let _ = plan;
        Err("WFP_WINDOWS_REQUIRED".into())
    }
}
pub fn update(plan: &Plan) -> Result<Value, String> {
    let mut v = arm(plan)?;
    v["action"] = json!("update");
    Ok(v)
}
pub fn status(plan: &Plan) -> Result<Value, String> {
    #[cfg(windows)]
    {
        platform::status(plan)
    }
    #[cfg(not(windows))]
    {
        let _ = plan;
        Err("WFP_WINDOWS_REQUIRED".into())
    }
}
pub fn inspect() -> Result<Value, String> {
    #[cfg(windows)]
    {
        platform::inspect()
    }
    #[cfg(not(windows))]
    {
        Err("WFP_WINDOWS_REQUIRED".into())
    }
}
pub fn disarm() -> Result<Value, String> {
    #[cfg(windows)]
    {
        platform::disarm()
    }
    #[cfg(not(windows))]
    {
        Err("WFP_WINDOWS_REQUIRED".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tor_transport_is_tcp_only_and_below_physical_dns_blocks() {
        let value = json!({"schema":2,"transport":"tor","core_executable":"C:\\Program Files\\Mukhomor\\bin\\mukhomor-tor.exe","tunnel_luid":0,"endpoints":[],"allow_private_lan":false});
        let plan = Plan::parse(&value).unwrap();
        assert_eq!(plan.value(), value);
        let rs = rules(&plan);
        let permits: Vec<_> = rs.iter().filter(|r| r.weight == WEIGHT_TOR_TRANSPORT).collect();
        assert_eq!(permits.len(), 2);
        for r in permits {
            assert!(r.permit && r.weight < WEIGHT_PHYSICAL_DNS_BLOCK);
            assert_eq!(r.conditions, vec![c(Field::App,Data::App),c(Field::User,Data::System),c(Field::Protocol,Data::U8(6))]);
        }
        let mut bad = value.clone();bad["allow_private_lan"] = json!(true);
        assert!(Plan::parse(&bad).is_err());
        bad = value;bad["endpoints"] = json!([{"address":"1.1.1.1","port":443,"protocol":"tcp"}]);
        assert!(Plan::parse(&bad).is_err());
    }
    pub(super) fn fixture() -> Value {
        json!({"schema":1,"core_executable":"C:\\Program Files\\Mukhomor\\bin\\mihomo.exe",
        "tunnel_luid":42,"endpoints":[{"address":"203.0.113.7","port":8444,"protocol":"tcp"}],"allow_private_lan":false})
    }
    #[test]
    fn snowflake_udp_is_scoped_to_the_adapter_and_never_permits_dns_or_user_udp() {
        for transport in ["tor-bridges","tor-snowflake"] {
            let value=json!({"schema":2,"transport":transport,"core_executable":"C:\\Program Files\\Mukhomor\\bin\\mukhomor-pt.exe","tunnel_luid":0,"endpoints":[],"allow_private_lan":false});
            let plan=Plan::parse(&value).unwrap();assert_eq!(plan.value(),value);let rs=rules(&plan);
            for layer in [Layer::Connect4,Layer::Connect6] {
                let mut f=internet(layer);f.protocol=17;f.remote_port=3478;
                assert!(!allows(&rs,&f));f.app=true;assert!(!allows(&rs,&f));f.system=true;
                assert_eq!(allows(&rs,&f),transport=="tor-snowflake");
                for port in [53,853] {f.remote_port=port;assert!(!allows(&rs,&f));f.protocol=6;assert!(!allows(&rs,&f));}
            }
        }
    }
    #[test]
    fn filter_enumeration_requires_a_real_layer_and_includes_boot_and_disabled() {
        assert_eq!(
            filter_enum_template(GUID::from_u128(0)).err().as_deref(),
            Some("WFP_LAYER_ENUM_INVALID")
        );
        // A non-policy layer is valid too: orphan detection must scan beyond
        // the layers which our normal policy installs filters on.
        let layer = GUID::from_u128(0x05c668e5_237d_4687_857b_e1b10469e890);
        let template = filter_enum_template(layer).unwrap();
        assert!(guid_eq(&template.layerKey, &layer));
        assert_eq!(template.enumType, FWP_FILTER_ENUM_FULLY_CONTAINED);
        assert_eq!(
            template.flags,
            FWP_FILTER_ENUM_FLAG_INCLUDE_BOOTTIME | FWP_FILTER_ENUM_FLAG_INCLUDE_DISABLED
        );
        assert_eq!(template.actionMask, u32::MAX);
        assert!(template.providerKey.is_null());
        assert!(template.providerContextTemplate.is_null());
        assert!(template.filterCondition.is_null());
        assert!(template.calloutKey.is_null());
        assert_eq!(template.numFilterConditions, 0);
    }
    #[test]
    fn enumeration_budget_is_global_and_rejection_does_not_reset_or_overflow_it() {
        let mut total = 0;
        charge_enumeration(&mut total, 61_000, MAX_ENUMERATED_FILTERS, "BOUND").unwrap();
        charge_enumeration(&mut total, 39_000, MAX_ENUMERATED_FILTERS, "BOUND").unwrap();
        assert_eq!(total, MAX_ENUMERATED_FILTERS);
        assert!(charge_enumeration(&mut total, 1, MAX_ENUMERATED_FILTERS, "BOUND").is_err());
        assert_eq!(total, MAX_ENUMERATED_FILTERS);
        let mut overflowing = usize::MAX;
        assert!(charge_enumeration(&mut overflowing, 1, usize::MAX, "BOUND").is_err());
        assert_eq!(overflowing, usize::MAX);
        let mut layers = MAX_ENUMERATED_LAYERS;
        assert!(charge_enumeration(&mut layers, 1, MAX_ENUMERATED_LAYERS, "BOUND").is_err());
        assert_eq!(layers, MAX_ENUMERATED_LAYERS);
    }
    #[test]
    fn owned_filter_identity_on_an_unexpected_layer_is_never_treated_as_absent() {
        let layer = GUID::from_u128(0x05c668e5_237d_4687_857b_e1b10469e890);
        let expected = rules(&Plan::parse(&fixture()).unwrap());
        assert!(expected.iter().all(|r| !guid_eq(&r.layer.key(), &layer)));
        let foreign = GUID::from_u128(0x887abf99_dd60_485c_8a69_806361ec5bb7);
        for (key, sublayer, provider) in [
            (foreign, SUBLAYER, None),
            (foreign, foreign, Some(&PROVIDER)),
            (GUID::from_u128(FILTER_BASE + 1), foreign, None),
        ] {
            let filter = FWPM_FILTER0 {
                layerKey: layer,
                filterKey: key,
                subLayerKey: sublayer,
                ..Default::default()
            };
            assert_eq!(
                owned_filter_rule(&filter.filterKey, &filter.subLayerKey, provider, &[])
                    .err()
                    .as_deref(),
                Some("WFP_UNEXPECTED_OWN_FILTER")
            );
        }
        assert!(
            owned_filter_rule(&foreign, &foreign, Some(&foreign), &[])
                .unwrap()
                .is_none()
        );
        assert!(
            owned_filter_rule(&expected[0].key(), &SUBLAYER, Some(&PROVIDER), &expected)
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn discovery_never_claims_a_live_tunnel_from_saved_policy() {
        let absent = inspection_result(None);
        assert_eq!(absent["armed"], false);
        assert_eq!(absent["verified"], true);
        assert_eq!(absent["protected"], false);
        assert_eq!(absent["filter_count"], 0);
        assert!(absent["plan"].is_null());
        for luid in [0, 42] {
            let mut v = fixture();
            v["tunnel_luid"] = json!(luid);
            let p = Plan::parse(&v).unwrap();
            let discovered = inspection_result(Some(&p));
            assert_eq!(discovered["armed"], true);
            assert_eq!(discovered["verified"], true);
            assert_eq!(discovered["protected"], false);
            assert_eq!(discovered["persistent"], true);
            assert_eq!(discovered["boot_policy_verified"], true);
            assert_eq!(discovered["runtime_policy_verified"], true);
            assert_eq!(discovered["filter_count"], rules(&p).len());
            assert_eq!(Plan::parse(&discovered["plan"]).unwrap(), p);
        }
    }
    #[test]
    fn strict_parse_rejects_unsafe_policy_inputs() {
        let mut cases = Vec::new();
        let mut v = fixture();
        v["schema"] = json!(2);
        cases.push(v);
        let mut v = fixture();
        v["extra"] = json!(true);
        cases.push(v);
        for path in [
            "mihomo.exe",
            "\\\\server\\mihomo.exe",
            "C:\\..\\mihomo.exe",
            "C:\\x\\.\\mihomo.exe",
            "C:\\x\\mihomo.exe:stream",
            "C:\\x.\\mihomo.exe",
            "C:/x/mihomo.exe",
            "C:\\x\\bad?.exe",
        ] {
            let mut v = fixture();
            v["core_executable"] = json!(path);
            cases.push(v);
        }
        for address in [
            "localhost",
            "0.0.0.0",
            "127.0.0.1",
            "::",
            "::1",
            "224.1.2.3",
            "ff02::2",
            "255.255.255.255",
            "0.0.0.1",
            "169.254.1.2",
            "240.0.0.1",
            "::ffff:203.0.113.7",
            "fe80::1",
            "fe80::1%3",
        ] {
            let mut v = fixture();
            v["endpoints"][0]["address"] = json!(address);
            cases.push(v);
        }
        for port in [
            json!(0),
            json!(65536),
            json!(8444.5),
            json!("8444"),
            json!(-1),
        ] {
            let mut v = fixture();
            v["endpoints"][0]["port"] = port;
            cases.push(v);
        }
        for protocol in ["TCP", "icmp", "any", ""] {
            let mut v = fixture();
            v["endpoints"][0]["protocol"] = json!(protocol);
            cases.push(v);
        }
        let mut v = fixture();
        v["endpoints"] = json!([]);
        cases.push(v);
        let mut v = fixture();
        v["endpoints"] = json!(vec![v["endpoints"][0].clone(); 9]);
        cases.push(v);
        let mut v = fixture();
        let duplicate = v["endpoints"][0].clone();
        v["endpoints"].as_array_mut().unwrap().push(duplicate);
        cases.push(v);
        for v in cases {
            assert!(Plan::parse(&v).is_err(), "accepted {v}");
        }
    }
    #[test]
    fn plan_canonical_order_and_bootstrap_are_explicit() {
        let mut v = fixture();
        v["tunnel_luid"] = json!(0);
        let p = Plan::parse(&v).unwrap();
        assert_eq!(preview(&p)["protected"], false);
        assert_eq!(preview(&p)["bootstrap"], true);
        assert!(
            rules(&p)
                .iter()
                .all(|r| r.conditions.iter().all(|c| c.field != Field::Interface))
        );
        let mut v = fixture();
        v["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(json!({"address":"2001:db8::7","port":8444,"protocol":"udp"}));
        let a = Plan::parse(&v).unwrap();
        v["endpoints"].as_array_mut().unwrap().reverse();
        assert_eq!(Plan::parse(&v).unwrap(), a);
        assert_eq!(Plan::parse(&a.value()).unwrap(), a);
    }
    #[test]
    fn boot_blocks_and_runtime_persistence_are_disjoint() {
        let p = Plan::parse(&fixture()).unwrap();
        let rs = rules(&p);
        let boot: Vec<_> = rs.iter().filter(|r| r.layer.boot()).collect();
        assert_eq!(boot.len(), 4);
        for r in boot {
            assert!(!r.permit);
            assert!(r.conditions.is_empty());
            assert_eq!(r.flags(), FWPM_FILTER_FLAG_BOOTTIME);
        }
        for r in &rs {
            assert_ne!(
                r.flags(),
                FWPM_FILTER_FLAG_BOOTTIME | FWPM_FILTER_FLAG_PERSISTENT
            );
            assert_eq!(r.flags() & FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT, 0);
            assert_eq!(key_id(&r.key()), Some(r.id));
        }
        let keys: BTreeSet<_> = rs.iter().map(|r| r.id).collect();
        assert_eq!(keys.len(), rs.len());
        assert_eq!(key_id(&PROVIDER), None);
        assert_eq!(key_id(&SUBLAYER), None);
    }
    #[test]
    fn exact_endpoint_conjoins_system_app_protocol_address_port() {
        let p = Plan::parse(&fixture()).unwrap();
        let rs = rules(&p);
        let endpoints: Vec<_> = rs
            .iter()
            .filter(|r| r.permit && r.weight == WEIGHT_CORE_ENDPOINT)
            .collect();
        assert_eq!(endpoints.len(), 1);
        let r = endpoints[0];
        assert_eq!(r.layer, Layer::Connect4);
        assert_eq!(r.conditions.len(), 5);
        for (f, d) in [
            (Field::App, Data::App),
            (Field::User, Data::System),
            (Field::Protocol, Data::U8(6)),
            (Field::RemotePort, Data::U16(8444)),
            (
                Field::RemoteAddress,
                Data::Address("203.0.113.7".parse().unwrap(), 32),
            ),
        ] {
            assert!(r.conditions.iter().any(|c| c.field == f && c.data == d));
        }
        assert!(
            rs.iter()
                .filter(|r| r.permit && r.conditions.iter().any(|c| c.field == Field::App))
                .all(|r| r.conditions.len() == 5)
        );
    }
    #[test]
    fn legal_weight_ranges_preserve_all_security_tiers() {
        let order = [
            WEIGHT_DEFAULT_BLOCK,
            WEIGHT_PRIVATE_LAN,
            WEIGHT_LINK_CONTROL,
            WEIGHT_PHYSICAL_DNS_BLOCK,
            WEIGHT_CORE_ENDPOINT,
            WEIGHT_TUNNEL,
            WEIGHT_LOOPBACK,
        ];
        assert!(order.iter().all(|weight| *weight <= 15));
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
        let mut value = fixture();
        value["allow_private_lan"] = json!(true);
        let rs = rules(&Plan::parse(&value).unwrap());
        let actual: BTreeSet<_> = rs.iter().map(|rule| rule.weight).collect();
        assert_eq!(actual, order.into_iter().collect());
        for weight in actual {
            let actions: BTreeSet<_> = rs
                .iter()
                .filter(|rule| rule.weight == weight)
                .map(|rule| rule.permit)
                .collect();
            assert_eq!(actions.len(), 1, "equal ranges must not mix actions");
        }
    }
    #[test]
    fn dns_precedes_private_lan_and_cgnat_is_never_a_lan_prefix() {
        let mut v = fixture();
        v["allow_private_lan"] = json!(true);
        let rs = rules(&Plan::parse(&v).unwrap());
        let dns: Vec<_> = rs
            .iter()
            .filter(|r| !r.permit && r.weight == WEIGHT_PHYSICAL_DNS_BLOCK)
            .collect();
        assert_eq!(dns.len(), 16);
        assert!(dns.iter().all(|r| {
            r.conditions.len() == 2
                && r.conditions
                    .iter()
                    .any(|c| matches!(c.data, Data::U16(53 | 853)))
        }));
        let lan: Vec<_> = rs
            .iter()
            .filter(|r| r.permit && r.weight == WEIGHT_PRIVATE_LAN)
            .collect();
        assert_eq!(lan.len(), 8);
        assert!(lan.iter().all(|r| r.conditions.len() == 1));
        assert!(!lan.iter().any(|r| {
            r.conditions
                .iter()
                .any(|c| matches!(c.data,Data::Address(IpAddr::V4(a),_) if a.octets()[0]==100))
        }));
        assert!(lan.iter().all(|r| r.weight < WEIGHT_PHYSICAL_DNS_BLOCK));
    }
    #[test]
    fn link_control_exceptions_do_not_allow_arbitrary_udp_or_icmp() {
        let rs = rules(&Plan::parse(&fixture()).unwrap());
        for r in rs
            .iter()
            .filter(|r| r.permit && r.weight == WEIGHT_LINK_CONTROL)
        {
            assert!(r.conditions.iter().any(|c| c.field == Field::Protocol));
            assert!(r.conditions.iter().any(|c| c.field == Field::LocalPort));
            assert!(r.conditions.iter().any(|c| c.field == Field::RemotePort));
            if r.conditions.iter().any(|c| c.data == Data::U8(58)) {
                assert!(r.conditions.iter().any(|c| c.field == Field::RemoteAddress));
            }
        }
    }
    // Independent traffic scenarios against the declarative plan. This is not
    // a Windows classifier/network proof; the guest tests must prove that part.
    #[derive(Clone)]
    struct Flow {
        layer: Layer,
        remote: IpAddr,
        local: IpAddr,
        remote_port: u16,
        local_port: u16,
        protocol: u8,
        luid: u64,
        app: bool,
        system: bool,
        loopback: bool,
    }
    fn address_in(address: IpAddr, network: IpAddr, prefix: u8) -> bool {
        match (address, network) {
            (IpAddr::V4(a), IpAddr::V4(n)) => {
                let mask = if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                };
                u32::from_be_bytes(a.octets()) & mask == u32::from_be_bytes(n.octets()) & mask
            }
            (IpAddr::V6(a), IpAddr::V6(n)) => {
                let mask = if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                };
                u128::from_be_bytes(a.octets()) & mask == u128::from_be_bytes(n.octets()) & mask
            }
            _ => false,
        }
    }
    fn allows(rs: &[Rule], f: &Flow) -> bool {
        rs.iter()
            .filter(|r| {
                r.layer == f.layer
                    && r.conditions.iter().all(|c| match (&c.field, &c.data) {
                        (Field::Protocol, Data::U8(n)) => f.protocol == *n,
                        (Field::RemotePort, Data::U16(n)) => f.remote_port == *n,
                        (Field::LocalPort, Data::U16(n)) => f.local_port == *n,
                        (Field::RemoteAddress, Data::Address(n, p)) => address_in(f.remote, *n, *p),
                        (Field::LocalAddress, Data::Address(n, p)) => address_in(f.local, *n, *p),
                        (Field::Interface, Data::U64(n)) => f.luid == *n,
                        (Field::App, Data::App) => f.app,
                        (Field::User, Data::System) => f.system,
                        (Field::Flags, Data::U32(n)) => {
                            c.flags_match && *n == FWP_CONDITION_FLAG_IS_LOOPBACK && f.loopback
                        }
                        _ => false,
                    })
            })
            .max_by_key(|r| r.weight)
            .is_some_and(|r| r.permit)
    }
    fn internet(layer: Layer) -> Flow {
        Flow {
            layer,
            remote: "203.0.113.7".parse().unwrap(),
            local: "192.168.1.5".parse().unwrap(),
            remote_port: 8444,
            local_port: 40000,
            protocol: 6,
            luid: 17,
            app: false,
            system: false,
            loopback: false,
        }
    }
    #[test]
    fn physical_traffic_table_only_permits_scoped_exceptions() {
        let p = Plan::parse(&fixture()).unwrap();
        let rs = rules(&p);
        let f = internet(Layer::Connect4);
        assert!(!allows(&rs, &f));
        let mut permitted = f.clone();
        permitted.app = true;
        permitted.system = true;
        assert!(allows(&rs, &permitted));
        for variant in 0..6 {
            let mut x = permitted.clone();
            match variant {
                0 => x.app = false,
                1 => x.system = false,
                2 => x.remote = "203.0.113.8".parse().unwrap(),
                3 => x.remote_port = 443,
                4 => x.protocol = 17,
                5 => x.layer = Layer::Accept4,
                _ => unreachable!(),
            }
            assert!(!allows(&rs, &x), "unscoped endpoint variant{variant}");
        }
        let mut tun = f.clone();
        tun.luid = 42;
        assert!(allows(&rs, &tun));
        tun.layer = Layer::Accept4;
        assert!(allows(&rs, &tun));
        let mut lo = f.clone();
        lo.remote = "127.0.0.1".parse().unwrap();
        assert!(!allows(&rs, &lo));
        lo.loopback = true;
        assert!(allows(&rs, &lo));
        for layer in [
            Layer::Connect4,
            Layer::Connect6,
            Layer::Accept4,
            Layer::Accept6,
        ] {
            let mut x = f.clone();
            x.layer = layer;
            if matches!(layer, Layer::Connect6 | Layer::Accept6) {
                x.remote = "2001:db8::7".parse().unwrap();
                x.local = "2001:db8::8".parse().unwrap();
            }
            assert!(!allows(&rs, &x));
            x.luid = 42;
            assert!(allows(&rs, &x));
        }
        for layer in [
            Layer::BootIn4,
            Layer::BootIn6,
            Layer::BootOut4,
            Layer::BootOut6,
        ] {
            let mut x = permitted.clone();
            x.layer = layer;
            x.luid = 42;
            x.loopback = true;
            assert!(!allows(&rs, &x));
        }
    }
    #[test]
    fn lan_opt_in_cannot_override_physical_dns_blocks() {
        let mut v = fixture();
        v["allow_private_lan"] = json!(true);
        let rs = rules(&Plan::parse(&v).unwrap());
        for address in ["10.1.2.3", "172.16.2.3", "192.168.1.2", "fd00::123"] {
            for inbound in [false, true] {
                let mut f = internet(if address.contains(':') {
                    if inbound {
                        Layer::Accept6
                    } else {
                        Layer::Connect6
                    }
                } else if inbound {
                    Layer::Accept4
                } else {
                    Layer::Connect4
                });
                f.remote = address.parse().unwrap();
                f.remote_port = 443;
                assert!(allows(&rs, &f));
                for port in [53, 853] {
                    for protocol in [6, 17] {
                        f.remote_port = if inbound { 443 } else { port };
                        f.local_port = if inbound { port } else { 40000 };
                        f.protocol = protocol;
                        assert!(
                            !allows(&rs, &f),
                            "physical DNS permitted: {address}:{port}/{protocol}"
                        );
                        let mut tun = f.clone();
                        tun.luid = 42;
                        assert!(allows(&rs, &tun));
                    }
                }
            }
        }
        for address in [
            "100.64.0.1",
            "100.127.255.254",
            "172.32.0.1",
            "169.254.1.2",
            "2001:db8::7",
        ] {
            let mut f = internet(if address.contains(':') {
                Layer::Connect6
            } else {
                Layer::Connect4
            });
            f.remote = address.parse().unwrap();
            assert!(!allows(&rs, &f));
        }
    }
}
