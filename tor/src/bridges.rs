use crate::{Init, tor_options::TorOptions};
use arti_client::config::{
    BridgeConfigBuilder, TorClientConfigBuilder, pt::TransportConfigBuilder,
};
use std::{
    io::{self, BufRead, Write},
    process::{Child, Stdio},
};

// Anonymous stdin handles form a process ownership chain. If Arti is killed,
// the PT helper receives EOF and exits; no orphan with egress remains running.
pub struct Transport(Child, std::sync::Arc<std::sync::atomic::AtomicBool>);

pub fn prepare_options(init: &mut Init) -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    init.tor_options = init.tor_options.clone().validate().map_err(io::Error::other)?;
    if init.tor_options.mode != "direct" {
        // Keep each explicitly selected bridge set's guard state. Reordering
        // lines is equivalent; changing sets must not strand an old guard with
        // a now-unavailable PT channel. Returning to a set reuses its state.
        let mut lines = init.tor_options.lines();
        lines.sort_unstable();
        let identity = format!("{}\n{}", init.tor_options.mode, lines.join("\n"));
        let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
        init.state_dir = std::path::Path::new(&init.state_dir).join("bridges-v1").join(digest).to_string_lossy().into();
    }
    Ok(())
}

fn transport_secret(init: &Init, builder: &TorClientConfigBuilder) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;
    // Arti persists PT settings as part of the guard's channel identity. A new
    // token each launch makes the saved bridge appear removed, preventing reuse.
    // Keep this token in the same checked private directory as the guard state;
    // the user-facing SOCKS credentials still change for every session.
    let directory = builder.build()?.fs_mistrust().verifier().make_secure_dir(&init.state_dir)?;
    let name = "pt-auth-v1";
    let path = directory.join(name)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if init.username.len() != 64 || !init.username.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(io::Error::other("TOR_PT_SECRET_INVALID").into());
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = directory.open(name, &options)?;
            file.write_all(init.username.as_bytes())?;
            file.sync_all()?;
        }
        Err(error) => return Err(error.into()),
        Ok(info) if !info.file_type().is_file() => return Err(io::Error::other("TOR_PT_SECRET_INVALID").into()),
        Ok(_) => (),
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    let mut token = String::new();
    directory.open(name, &options)?.take(65).read_to_string(&mut token)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("TOR_PT_SECRET_INVALID").into());
    }
    Ok(token)
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.1.store(true, std::sync::atomic::Ordering::Release);
        self.0.stdin.take();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn configure(
    init: &Init,
    builder: &mut TorClientConfigBuilder,
) -> Result<Option<Transport>, Box<dyn std::error::Error>> {
    let options = init
        .tor_options
        .clone()
        .validate()
        .map_err(io::Error::other)?;
    if options.mode == "direct" {
        return Ok(None);
    }
    let token = transport_secret(init, builder)?;
    // A circuit request must outlive the bounded 90s PT handshake.
    builder.circuit_timing().request_timeout(std::time::Duration::from_secs(120));
    builder.stream_timeouts().connect_timeout(std::time::Duration::from_secs(30));
    let mut lines = options.lines();
    lines.sort_unstable();
    let mut bridges = Vec::new();
    for line in &lines {
        let b: BridgeConfigBuilder = line
            .parse()
            .map_err(|_| io::Error::other("TOR_BRIDGE_INVALID"))?;
        b.build()
            .map_err(|_| io::Error::other("TOR_BRIDGE_INVALID"))?;
        bridges.push(b);
    }
    let exe = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "mukhomor-pt.exe"
    } else if cfg!(target_os = "android") {
        "libmukhomor_pt.so"
    } else {
        "mukhomor-pt"
    };
    let path = exe
        .parent()
        .ok_or_else(|| io::Error::other("TOR_PT_MISSING"))?
        .join(name);
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(io::Error::other("TOR_PT_MISSING").into());
    }
    let mut command = std::process::Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut child = Transport(command.spawn()?, stopping.clone());
    let frame = serde_json::json!({"state_dir":init.state_dir,"interface":init.interface,"token":token,"mode":options.transport_mode(),"bridges":lines});
    let input = child
        .0
        .stdin
        .as_mut()
        .ok_or_else(|| io::Error::other("TOR_PT_PIPE"))?;
    serde_json::to_writer(&mut *input, &frame)?;
    input.write_all(b"\n")?;
    input.flush()?;
    let output = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("TOR_PT_PIPE"))?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        use std::io::Read;
        let mut reader = std::io::BufReader::new(output.take(4097));
        let result = reader.read_line(&mut line).map(|_| line);
        let _ = send.send(result);
        // PT failure must invalidate Arti readiness, not leave a green connected UI.
        let mut byte = [0];
        let _ = reader.read(&mut byte);
        if !stopping.load(std::sync::atomic::Ordering::Acquire) {
            std::process::exit(1);
        }
    });
    let line = receive.recv_timeout(std::time::Duration::from_secs(20))??;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ready {
        ports: Vec<u16>,
    }
    let ready: Ready = serde_json::from_str(&line)?;
    if ready.ports.len() != bridges.len() || ready.ports.iter().any(|p| *p == 0) {
        return Err(io::Error::other("TOR_PT_REPLY").into());
    }
    for (index, (mut bridge, port)) in bridges.into_iter().zip(ready.ports).enumerate() {
        let protocol = format!("mukhomor{index}");
        bridge.transport(&protocol);
        bridge.settings().clear();
        bridge.push_setting("mukhomor-token", &token);
        builder.bridges().bridges().push(bridge);
        let mut transport = TransportConfigBuilder::default();
        transport
            .protocols(vec![protocol.parse()?])
            .proxy_addr(([127, 0, 0, 1], port).into());
        builder.bridges().transports().push(transport);
    }
    // Explicitly require bridges: a blocked bridge never falls back to public relays.
    builder
        .bridges()
        .enabled(arti_client::config::BoolOrAuto::Explicit(true));
    Ok(Some(child))
}

pub fn validate_input() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin().take(32769).read_to_end(&mut bytes)?;
    if bytes.len() > 32768 {
        return Err(io::Error::other("Tor bridge list exceeds 8 KiB").into());
    }
    let options: TorOptions = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Invalid Tor bridge configuration"))?;
    let options = options.validate().map_err(io::Error::other)?;
    for line in options.lines() {
        let b: BridgeConfigBuilder = line
            .parse()
            .map_err(|_| io::Error::other("Invalid Tor bridge line"))?;
        b.build()
            .map_err(|_| io::Error::other("Invalid Tor bridge line"))?;
    }
    println!("{}", serde_json::json!({"options":options}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_sets_keep_distinct_stable_state() {
        let mut init = Init { state_dir: "/private/state".into(), cache_dir: "/private/cache".into(), username: "a".repeat(64),
            password: "b".repeat(64), interface: "7".into(), tor_options: TorOptions { mode: "snowflake".into(), bridges: String::new() } };
        prepare_options(&mut init).unwrap();
        let snowflake = init.state_dir.clone();
        init.state_dir = "/private/state".into();
        init.username = "c".repeat(64);
        prepare_options(&mut init).unwrap();
        assert_eq!(init.state_dir, snowflake);
        init.state_dir = "/private/state".into();
        init.tor_options = TorOptions::default();
        prepare_options(&mut init).unwrap();
        assert_eq!(init.state_dir, "/private/state");
        assert_ne!(init.state_dir, snowflake);
    }
    #[test]
    fn pt_identity_survives_sessions_and_rejects_damaged_secret() {
        let directory = std::env::temp_dir().join(format!("mukhomor-pt-secret-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut init = Init { state_dir: directory.to_string_lossy().into(), cache_dir: directory.join("cache").to_string_lossy().into(),
            username: "a".repeat(64), password: "b".repeat(64), interface: "7".into(), tor_options: Default::default() };
        let builder = TorClientConfigBuilder::from_directories(&init.state_dir, &init.cache_dir);
        assert_eq!(transport_secret(&init, &builder).unwrap(), "a".repeat(64));
        init.username = "c".repeat(64);
        assert_eq!(transport_secret(&init, &builder).unwrap(), "a".repeat(64));
        let path = directory.join("pt-auth-v1");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&path, b"damaged").unwrap();
        assert!(transport_secret(&init, &builder).is_err());
        #[cfg(unix)] {
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(directory.join("outside"), &path).unwrap();
            assert!(transport_secret(&init, &builder).is_err());
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
