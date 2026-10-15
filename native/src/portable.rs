use mukhomor_ui::AppWindow;
use serde_json::Value;
use std::{path::{Path, PathBuf}, sync::Arc};

pub type Transport = Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>;

#[cfg(target_os = "linux")]
pub struct Instance {
    _lock: std::fs::File,
    listener: std::os::unix::net::UnixListener,
    socket: PathBuf,
}
#[cfg(target_os = "linux")]
impl Instance {
    pub fn acquire(root: &Path, startup: bool) -> Result<Option<Self>, String> {
        use std::{fs::OpenOptions, io::Write, os::unix::{fs::OpenOptionsExt, net::{UnixListener, UnixStream}}};
        let lock=OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(root.join("ui.lock")).map_err(|e|e.to_string())?;
        let socket=root.join("ui.sock");
        if lock.try_lock().is_err() {
            if !startup {
                // Give the first process time to bind after acquiring the lock.
                for _ in 0..20 {
                    if let Ok(mut connection)=UnixStream::connect(&socket) { let _=connection.write_all(b"A"); return Ok(None); }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                return Err("Mukhomor is already starting; try opening it again".into());
            }
            return Ok(None);
        }
        if socket.exists() { std::fs::remove_file(&socket).map_err(|e|e.to_string())?; }
        let listener=UnixListener::bind(&socket).map_err(|e|e.to_string())?;
        listener.set_nonblocking(true).map_err(|e|e.to_string())?;
        Ok(Some(Self{_lock:lock,listener,socket}))
    }
    pub fn activation_requested(&self) -> bool {
        use std::io::Read;
        if let Ok((mut client,_))=self.listener.accept() {
            let _=client.set_read_timeout(Some(std::time::Duration::from_millis(10)));
            let mut byte=[0];return client.read_exact(&mut byte).is_ok() && byte==[b'A'];
        }
        false
    }
}
#[cfg(target_os = "linux")]
impl Drop for Instance { fn drop(&mut self) { let _=std::fs::remove_file(&self.socket); } }

#[cfg(target_os = "linux")]
pub fn data_root() -> Result<PathBuf, String> {
    let args: Vec<String> = std::env::args().collect();
    let path = if let Some(p) = args.windows(2).find(|p| p[0] == "--root") {
        PathBuf::from(&p[1])
    } else if let Some(home) = std::env::var_os("XDG_DATA_HOME").filter(|p| Path::new(p).is_absolute()) {
        PathBuf::from(home).join("mukhomor")
    } else {
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not defined")?).join(".local/share/mukhomor")
    };
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(target_os = "linux")]
pub fn transport(root: &Path) -> Result<Transport, String> {
    use std::{io::{BufRead, BufReader, Read, Write}, process::{Command, Stdio}, sync::{Mutex, mpsc}};
    struct Process { child: std::process::Child, input: std::process::ChildStdin, output: mpsc::Receiver<Result<Value, String>> }
    impl Drop for Process {
        fn drop(&mut self) {
            // Request a graceful TUN cleanup, then bound shutdown. OS removes the
            // TUN on process death; no permanent resolver files are modified.
            let _ = self.input.write_all(b"{\"action\":\"Exit\"}\n");
            let _ = self.input.flush();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() { return; }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            let _ = self.child.kill(); let _ = self.child.wait();
        }
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let backend = executable.parent().ok_or("Missing executable directory")?.join("mukhomor-controller");
    let mut child = Command::new(&backend).arg("--root").arg(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("Cannot start {}: {e}",backend.display()))?;
    let input=child.stdin.take().unwrap(); let mut pipe=BufReader::new(child.stdout.take().unwrap());
    let (sender, output)=mpsc::sync_channel(1);
    std::thread::spawn(move || loop {
        let result=(|| {
            let mut reply=Vec::new();
            pipe.by_ref().take(4*1024*1024+1).read_until(b'\n', &mut reply).map_err(|e|e.to_string())?;
            if reply.last()!=Some(&b'\n') || reply.len()>4*1024*1024 { return Err("Controller exited or returned an invalid frame".into()); }
            serde_json::from_slice(&reply).map_err(|_|"Invalid controller response".into())
        })();
        let failed=result.is_err();
        if sender.send(result).is_err() || failed { break; }
    });
    let process=Arc::new(Mutex::new(Process{child,input,output}));
    Ok(Arc::new(move |request| {
        let mut p=process.lock().map_err(|_| "Controller lock failed")?;
        let bytes=serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len()>4*1024*1024 { return Err("Request exceeds size limit".into()); }
        p.input.write_all(&bytes).and_then(|_| p.input.write_all(b"\n")).and_then(|_| p.input.flush()).map_err(|e| e.to_string())?;
        let seconds=if request["action"] == "Update" { 120 } else { 45 };
        p.output.recv_timeout(std::time::Duration::from_secs(seconds)).map_err(|_|"VPN controller stopped responding; restart Mukhomor".to_owned())?
    }))
}

#[cfg(target_os = "android")]
pub fn transport(_: &Path) -> Result<Transport, String> {
    Ok(Arc::new(|request| crate::android::call("request", &request.to_string()).and_then(|s| serde_json::from_str(&s).map_err(|e|e.to_string()))))
}

pub fn configure_window(ui: &AppWindow, locale: &crate::i18n::Locale) {
    #[cfg(target_os = "linux")]
    {
        ui.set_startup_label(locale.text("Запуск при входе в Linux").as_ref().into());
        ui.set_startup_description(locale.text("Запускать приложение при входе в систему. Автоподключение VPN настраивается отдельно.").as_ref().into());
    }
    #[cfg(target_os = "android")]
    {
        ui.set_mobile(true);
        ui.set_startup_label(locale.text("Постоянный VPN").as_ref().into());
        ui.set_startup_description(locale.text("Постоянный VPN и блокировка соединений без VPN включаются в системных настройках Android.").as_ref().into());
    }
}

pub fn autostart_executable() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    { std::env::current_exe().ok() }
    #[cfg(target_os = "android")]
    { None }
}

pub fn choose_file(_ui: &AppWindow, save: bool, preset: bool) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd=std::process::Command::new("zenity"); cmd.arg("--file-selection").arg("--title=Mukhomor");
        if save { cmd.args(["--save", "--confirm-overwrite", "--filename=Mukhomor-preset.json"]); }
        if preset { cmd.arg("--file-filter=JSON | *.json"); }
        let out=cmd.output().ok()?;
        out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
    }
    #[cfg(target_os = "android")]
    { crate::android::call("chooseFile", if save {"save"} else if preset {"preset"} else {"profile"}).ok().filter(|s|!s.is_empty()).map(PathBuf::from) }
}

pub mod autostart {
    use super::*;
    #[cfg(target_os = "linux")]
    fn path() -> Result<PathBuf,String> {
        let root=std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p|p.is_absolute()).unwrap_or(PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not defined")?).join(".config"));
        Ok(root.join("autostart/mukhomor.desktop"))
    }
    pub fn enabled(_exe: &Path) -> Result<bool,String> {
        #[cfg(target_os = "linux")]
        { Ok(path()?.is_file()) }
        #[cfg(target_os = "android")]
        { Ok(false) }
    }
    pub fn set(exe: &Path, enabled: bool) -> Result<(),String> {
        #[cfg(target_os = "linux")]
        {
            let path=path()?;
            if !enabled { match std::fs::remove_file(path) { Ok(())=>Ok(()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(e.to_string()) } }
            else {
                let s=exe.to_str().ok_or("Executable path is not UTF-8")?;
                if s.contains(['\n','\r','%']) { return Err("Unsupported autostart path".into()); }
                let escaped=s.replace('\\',"\\\\").replace('"',"\\\"").replace('`',"\\`").replace('$',"\\$");
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e|e.to_string())?;
                std::fs::write(path,format!("[Desktop Entry]\nType=Application\nName=Mukhomor\nExec=\"{escaped}\" --startup\nTerminal=false\n")).map_err(|e|e.to_string())
            }
        }
        #[cfg(target_os = "android")]
        { let _=(exe,enabled); Err("Use Android VPN settings".into()) }
    }
}
