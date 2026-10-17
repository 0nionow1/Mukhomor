//! Private, supervised Tor transport. No relay or exit functionality is enabled.
//! The supervisor supplies secrets over stdin and retains it for the lifetime of
//! the session. EOF terminates the helper, including an in-progress bootstrap.
use arti_client::{TorClient, config::TorClientConfigBuilder};
use async_trait::async_trait;
use serde::Deserialize;
use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tor_rtcompat::{NetStreamProvider, Runtime, RuntimeSubstExt, tokio::PreferredRuntime};
#[path = "../../native/src/tor_options.rs"]
mod tor_options;
mod bridges;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Init {
    state_dir: String,
    cache_dir: String,
    username: String,
    password: String,
    /// Linux physical interface name; Windows physical interface index.
    interface: String,
    #[serde(default)]
    tor_options: tor_options::TorOptions,
}

#[derive(Clone)]
struct PhysicalTcp {
    base: PreferredRuntime,
    interface: String,
    direct: bool,
}

// Override only TCP creation: TLS, clocks and scheduling remain upstream Arti.
// A binding failure is fatal for that connection. Never retry unbound.
#[async_trait]
impl NetStreamProvider for PhysicalTcp {
    type Stream = <PreferredRuntime as NetStreamProvider>::Stream;
    type Listener = <PreferredRuntime as NetStreamProvider>::Listener;
    type ConnectOptions = <PreferredRuntime as NetStreamProvider>::ConnectOptions;
    type ListenOptions = <PreferredRuntime as NetStreamProvider>::ListenOptions;

    async fn connect(&self, addr: &SocketAddr, _: &Self::ConnectOptions) -> io::Result<Self::Stream> {
        if addr.ip().is_loopback() { return self.base.connect(addr, &Default::default()).await; }
        if !self.direct { return Err(io::Error::other("Direct Tor connections are disabled while using bridges")); }
        let socket = if addr.is_ipv4() {
            tokio::net::TcpSocket::new_v4()?
        } else {
            tokio::net::TcpSocket::new_v6()?
        };
        #[cfg(not(windows))]
        let socket_ref = socket2::SockRef::from(&socket);
        #[cfg(target_os = "linux")]
        {
            socket_ref.bind_device(Some(self.interface.as_bytes()))?;
            // Matches the controller's persistent nftables exception. Never
            // retry without the mark or without binding to the physical NIC.
            socket_ref.set_mark(0x4d544f52)?;
        }
        #[cfg(windows)]
        {
            let index = self.interface.parse::<u32>().ok().and_then(std::num::NonZeroU32::new)
                .ok_or_else(|| io::Error::other("physical interface required"))?;
            if addr.is_ipv4() {
                bind_windows(&socket, index.get(), true)?;
            } else {
                bind_windows(&socket, index.get(), false)?;
            }
        }
        #[cfg(target_os = "android")]
        let _ = (&socket_ref, &self.interface); // VpnService excludes our UID.
        Ok(socket.connect(*addr).await?.into())
    }

    async fn listen(&self, addr: &SocketAddr, options: &Self::ListenOptions) -> io::Result<Self::Listener> {
        self.base.listen(addr, options).await
    }
}

#[cfg(windows)]
fn bind_windows(tcp: &tokio::net::TcpSocket, index: u32, ipv4: bool) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::*;
    let value = if ipv4 { index.to_be() } else { index };
    let result = unsafe { setsockopt(tcp.as_raw_socket() as SOCKET, if ipv4 { IPPROTO_IP } else { IPPROTO_IPV6 }, if ipv4 { IP_UNICAST_IF } else { IPV6_UNICAST_IF }, (&value as *const u32).cast(), 4) };
    if result == SOCKET_ERROR { Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() })) } else { Ok(()) }
}

fn read_init() -> io::Result<Init> {
    let input = io::stdin();
    let mut locked = input.lock();
    let mut bytes = Vec::new();
    // A bounded read also rejects a supervisor that never terminates its frame.
    std::io::Read::take(&mut locked, 16385).read_until(b'\n', &mut bytes)?;
    if bytes.len() > 16384 || bytes.last() != Some(&b'\n') {
        return Err(io::Error::other("invalid initialization"));
    }
    let init: Init = serde_json::from_slice(&bytes)?;
    if !(32..=128).contains(&init.username.len()) || !(32..=128).contains(&init.password.len())
        || !init.username.is_ascii() || !init.password.is_ascii()
        || !std::path::Path::new(&init.state_dir).is_absolute()
        || !std::path::Path::new(&init.cache_dir).is_absolute()
        || (cfg!(not(target_os = "android")) && init.interface.is_empty())
    {
        return Err(io::Error::other("invalid initialization"));
    }
    drop(locked);
    std::thread::spawn(|| {
        let mut byte = [0];
        // Any further input, error or EOF closes this one-session transport.
        let _ = std::io::Read::read(&mut io::stdin(), &mut byte);
        std::process::exit(0);
    });
    Ok(init)
}

async fn handshake(s: &mut TcpStream, init: &Init) -> io::Result<(String, u16)> {
    if s.read_u8().await? != 5 { return Err(io::Error::other("SOCKS version")); }
    let count = s.read_u8().await? as usize;
    let mut methods = vec![0; count];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&2) {
        s.write_all(&[5, 255]).await?;
        return Err(io::Error::other("authentication required"));
    }
    s.write_all(&[5, 2]).await?;
    if s.read_u8().await? != 1 { return Err(io::Error::other("authentication version")); }
    let count = s.read_u8().await? as usize;
    let mut user = vec![0; count];
    s.read_exact(&mut user).await?;
    let count = s.read_u8().await? as usize;
    let mut pass = vec![0; count];
    s.read_exact(&mut pass).await?;
    // Credentials are random per session, never derived from a user password.
    let valid = constant_time_equal(&user, init.username.as_bytes())
        & constant_time_equal(&pass, init.password.as_bytes());
    s.write_all(&[1, if valid { 0 } else { 1 }]).await?;
    if !valid { return Err(io::Error::other("authentication failed")); }
    let mut header = [0; 4];
    s.read_exact(&mut header).await?;
    if header[0] != 5 || header[1] != 1 || header[2] != 0 {
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(io::Error::other("only CONNECT is supported"));
    }
    let host = match header[3] {
        1 => { let mut ip = [0; 4]; s.read_exact(&mut ip).await?; std::net::Ipv4Addr::from(ip).to_string() }
        4 => { let mut ip = [0; 16]; s.read_exact(&mut ip).await?; std::net::Ipv6Addr::from(ip).to_string() }
        3 => {
            let count = s.read_u8().await? as usize;
            let mut name = vec![0; count]; s.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(io::Error::other)?
        }
        _ => return Err(io::Error::other("address type")),
    };
    let port = s.read_u16().await?;
    if host.is_empty() || host.bytes().any(|b| b <= 32 || b >= 127) || port == 0 {
        return Err(io::Error::other("destination"));
    }
    Ok((host, port))
}

fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    let mut different = a.len() ^ b.len();
    for (i, byte) in b.iter().enumerate() {
        different |= usize::from(a.get(i).copied().unwrap_or(0) ^ byte);
    }
    different == 0
}

async fn serve<R: Runtime>(mut socket: TcpStream, init: Arc<Init>, client: Arc<TorClient<R>>) {
    let Ok(Ok(destination)) = tokio::time::timeout(Duration::from_secs(10), handshake(&mut socket, &init)).await else { return };
    match tokio::time::timeout(Duration::from_secs(60), client.connect(destination)).await {
        Ok(Ok(mut stream)) => {
            if socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.is_ok() {
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
            }
        }
        _ => { let _ = socket.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await; }
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() {
    #[cfg(feature = "diagnostics")]
    tracing_subscriber::fmt().with_env_filter("warn,tor_dirmgr=info,tor_chanmgr=debug,tor_guardmgr=debug,tor_ptmgr=debug").with_writer(std::io::stderr).init();
    std::panic::set_hook(Box::new(|_| eprintln!("TOR_TRANSPORT_FAILED")));
    if std::env::args().nth(1).as_deref()==Some("--validate-options") {
        if bridges::validate_input().is_err(){eprintln!("Invalid Tor bridge configuration");std::process::exit(1)}
        return;
    }
    // Intentionally do not log destinations, credentials, guard addresses or
    // private filesystem paths to either the UI or crash logs.
    if let Err(error) = run().await {
        let code = error.to_string();
        eprintln!("{}", match code.as_str() {
            "TOR_INIT_FAILED" | "TOR_RUNTIME_FAILED" | "TOR_CONFIG_FAILED" | "TOR_BOOTSTRAP_FAILED" => code.as_str(),
            _ => "TOR_TRANSPORT_FAILED",
        });
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut init = read_init().map_err(|_| io::Error::other("TOR_INIT_FAILED"))?;
    bridges::prepare_options(&mut init).map_err(|_| io::Error::other("TOR_CONFIG_FAILED"))?;
    let init = Arc::new(init);
    let _ = rustls::crypto::ring::default_provider().install_default();
    let base = PreferredRuntime::current().map_err(|_| io::Error::other("TOR_RUNTIME_FAILED"))?;
    let runtime = base.clone().with_tcp_provider(PhysicalTcp { base, interface: init.interface.clone(), direct: init.tor_options.mode=="direct" });
    let mut builder = TorClientConfigBuilder::from_directories(&init.state_dir, &init.cache_dir);
    let _transport = bridges::configure(&init,&mut builder).map_err(|_|io::Error::other("TOR_CONFIG_FAILED"))?;
    let config = builder.build().map_err(|_| io::Error::other("TOR_CONFIG_FAILED"))?;
    let client = TorClient::with_runtime(runtime).config(config).create_bootstrapped().await.map_err(|_| io::Error::other("TOR_BOOTSTRAP_FAILED"))?;
    // A downloaded directory alone is not a working exit circuit. Do not report
    // ready until a TCP stream has actually been opened through Tor.
    // A cached directory can bootstrap before a bridge channel exists. Allow
    // the 90s PT handshake plus circuit construction before declaring failure.
    let ready_timeout = if init.tor_options.mode == "direct" { 60 } else { 120 };
    drop(tokio::time::timeout(Duration::from_secs(ready_timeout), client.connect(("check.torproject.org",443))).await
        .map_err(|_| io::Error::other("TOR_BOOTSTRAP_FAILED"))?
        .map_err(|_| io::Error::other("TOR_BOOTSTRAP_FAILED"))?);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    println!("{}", serde_json::json!({"ready":true,"port":listener.local_addr()?.port()}));
    io::stdout().flush()?;
    let slots = Arc::new(tokio::sync::Semaphore::new(256));
    loop {
        let (socket, _) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
        let isolated = client.isolated_client();
        let init = init.clone();
        tokio::spawn(async move { let _permit = permit; serve(socket, init, isolated).await; });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_requires_exact_session_secret() {
        assert!(constant_time_equal(b"secret", b"secret"));
        for bad in [b"".as_slice(), b"secre", b"secretx", b"Secret", b"secreu"] {
            assert!(!constant_time_equal(bad, b"secret"));
        }
    }
    #[tokio::test]
    async fn socks_rejects_unauthenticated_clients_and_udp_associate() {
        for authenticated in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let init = Init { state_dir: String::new(), cache_dir: String::new(), username: "a".repeat(64), password: "b".repeat(64), interface: String::new(), tor_options: Default::default() };
            let task = tokio::spawn(async move { let (mut s,_) = listener.accept().await.unwrap(); handshake(&mut s,&init).await });
            let mut s = TcpStream::connect(address).await.unwrap();
            if !authenticated {
                s.write_all(&[5,1,0]).await.unwrap();
                let mut reply=[0;2];s.read_exact(&mut reply).await.unwrap();
                assert_eq!(reply,[5,255]);
            } else {
                s.write_all(&[5,1,2]).await.unwrap();
                let mut reply=[0;2];s.read_exact(&mut reply).await.unwrap();assert_eq!(reply,[5,2]);
                let mut auth=vec![1,64];auth.extend_from_slice(&[b'a';64]);auth.push(64);auth.extend_from_slice(&[b'b';64]);
                s.write_all(&auth).await.unwrap();s.read_exact(&mut reply).await.unwrap();assert_eq!(reply,[1,0]);
                s.write_all(&[5,3,0,1]).await.unwrap();
                let mut rejected=[0;10];s.read_exact(&mut rejected).await.unwrap();assert_eq!(rejected[1],7);
            }
            assert!(task.await.unwrap().is_err());
        }
    }
    #[tokio::test]
    async fn production_runtime_has_a_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _ = PreferredRuntime::current().unwrap();
    }
    #[test]
    fn coarse_system_clock_limits_finish_without_spinning() {
        let (tx,rx)=std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use saturating_time::SaturatingTime;
            let low=std::time::SystemTime::min_value();
            let high=std::time::SystemTime::max_value();
            assert!(low < std::time::SystemTime::UNIX_EPOCH && high > std::time::SystemTime::now());
            assert!(low.checked_sub(Duration::from_micros(1)).is_none());
            assert!(high.checked_add(Duration::from_micros(1)).is_none());
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(2)).expect("SystemTime limit search did not terminate");
    }
    #[tokio::test]
    async fn socks_preserves_http_and_https_after_authentication() {
        for port in [80u16, 8080, 443] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let init = Init { state_dir:String::new(), cache_dir:String::new(), username:"a".repeat(64), password:"b".repeat(64), interface:String::new(), tor_options: Default::default() };
            let task = tokio::spawn(async move { let (mut s,_) = listener.accept().await.unwrap(); handshake(&mut s,&init).await });
            let mut s = TcpStream::connect(address).await.unwrap();
            s.write_all(&[5,1,2]).await.unwrap();
            let mut reply=[0;2];s.read_exact(&mut reply).await.unwrap();assert_eq!(reply,[5,2]);
            let mut auth=vec![1,64];auth.extend_from_slice(&[b'a';64]);auth.push(64);auth.extend_from_slice(&[b'b';64]);
            s.write_all(&auth).await.unwrap();s.read_exact(&mut reply).await.unwrap();assert_eq!(reply,[1,0]);
            let mut request=vec![5,1,0,1,1,1,1,1];request.extend_from_slice(&port.to_be_bytes());
            s.write_all(&request).await.unwrap();
            assert_eq!(task.await.unwrap().unwrap(), ("1.1.1.1".to_owned(),port));
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "explicit public Tor network probe; does not create a TUN or change host routes"]
    async fn live_tor_bootstrap_diagnostics() {
        let _=tracing_subscriber::fmt().with_env_filter(std::env::var("MUKHOMOR_TOR_TEST_LOG").unwrap_or_else(|_|"warn".into())).with_writer(std::io::stderr).try_init();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let base=PreferredRuntime::current().unwrap();
        let interface=std::env::var("MUKHOMOR_TOR_TEST_INTERFACE").expect("test interface required");
        let state=std::path::PathBuf::from(std::env::var("MUKHOMOR_TOR_TEST_STATE").expect("private test state required"));
        let mode=std::env::var("MUKHOMOR_TOR_TEST_MODE").unwrap_or_else(|_|"direct".into());
        let runtime=base.clone().with_tcp_provider(PhysicalTcp{base,interface:interface.clone(),direct:mode=="direct"});
        let mut builder=TorClientConfigBuilder::from_directories(state.join("state"),state.join("cache"));
        let init=Init{state_dir:state.join("state").to_string_lossy().into(),cache_dir:state.join("cache").to_string_lossy().into(),username:"a".repeat(64),password:"b".repeat(64),interface,tor_options:tor_options::TorOptions{mode,bridges:String::new()}};
        let _pt=bridges::configure(&init,&mut builder).unwrap();
        let config=builder.build().unwrap();
        eprintln!("Tor diagnostic: constructing client");
        let client=TorClient::with_runtime(runtime).config(config).create_unbootstrapped_async().await.unwrap();
        eprintln!("Tor diagnostic: client constructed");
        let copy=client.clone();
        let mut bootstrap=tokio::spawn(async move{copy.bootstrap().await});
        for _ in 0..60 {
            tokio::select! {
                result=&mut bootstrap => {
                    result.unwrap().expect("bootstrap failed");
                    let stream=tokio::time::timeout(Duration::from_secs(60),client.connect(("check.torproject.org",443))).await.unwrap().unwrap();
                    drop(stream);eprintln!("PASS: Tor directory and exit TCP stream");
                    std::process::exit(0);
                }
                _=tokio::time::sleep(Duration::from_secs(5)) => {
                    let status=client.bootstrap_status();
                    eprintln!("Tor bootstrap: {:.0}% blockage={:?}",status.as_frac()*100.,status.blocked().map(|b|format!("{:?}: {}",b.kind(),b.message())));
                }
            }
        }
        bootstrap.abort();
        eprintln!("public Tor bootstrap not available in the test network");
        std::process::exit(2);
    }
}
