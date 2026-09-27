//! Native client for the one persistent init. No host scripting runtime required.
use kinakaze_v2_host_win::ProcessHandle;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{Shutdown, TcpStream},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const MAX_FRAME: usize = 1024 * 1024;
pub fn read(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut line = Vec::new();
    reader
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', &mut line)?;
    if line.len() > MAX_FRAME || line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete or oversized session frame",
        ));
    }
    serde_json::from_slice(&line).map_err(io::Error::other)
}
pub fn write(stream: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, value)?;
    stream.write_all(b"\n")?;
    stream.flush()
}
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}
pub fn unhex(text: &str) -> io::Result<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err(io::Error::other("invalid terminal data"));
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let a = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| io::Error::other("invalid terminal data"))?;
            let b = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| io::Error::other("invalid terminal data"))?;
            Ok((a * 16 + b) as u8)
        })
        .collect()
}

pub struct Options {
    pub dist: PathBuf,
    pub root: PathBuf,
    pub manifest: Option<PathBuf>,
    pub command: Vec<String>,
    pub cwd: String,
    pub name: Option<String>,
    pub action: String,
    pub no_tray: bool,
    pub web: Option<String>,
}
impl Options {
    pub fn parse(arguments: Vec<String>) -> Result<Self> {
        let mut out = Self {
            dist: std::env::current_exe()?.parent().unwrap().into(),
            root: PathBuf::new(),
            manifest: None,
            command: vec![],
            cwd: "/".into(),
            name: None,
            action: "run".into(),
            no_tray: false,
            web: None,
        };
        let mut args = arguments.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dist" => out.dist = args.next().ok_or("missing dist")?.into(),
                "--root" => out.root = args.next().ok_or("missing root")?.into(),
                "--rootfs-manifest" => {
                    out.manifest = Some(args.next().ok_or("missing manifest")?.into())
                }
                "--cwd" => out.cwd = args.next().ok_or("missing cwd")?,
                "--name" => out.name = Some(args.next().ok_or("missing name")?),
                "--web" => out.web = Some(args.next().ok_or("missing web address")?),
                "--no-tray" => out.no_tray = true,
                "status" | "stop" | "attach" | "start" => out.action = arg,
                "--" => {
                    out.command.extend(args);
                    break;
                }
                _ => return Err(format!("unknown session argument: {arg}").into()),
            }
        }
        out.dist = out.dist.canonicalize()?;
        if out.root.as_os_str().is_empty() {
            out.root = out.dist.join("rootfs");
        }
        fs::create_dir_all(&out.root)?;
        out.root = out.root.canonicalize()?;
        if let Some(path) = &mut out.manifest {
            *path = path.canonicalize()?;
        }
        Ok(out)
    }
    pub fn control_dir(&self) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.root.hash(&mut hash);
        self.root
            .parent()
            .unwrap()
            .join(format!(".kinakaze-session-{:016x}", hash.finish()))
    }
    pub fn descriptor(&self) -> PathBuf {
        self.control_dir().join("session.json")
    }
}
pub fn connect(options: &Options) -> Result<TcpStream> {
    let bytes = fs::read(options.descriptor())?;
    if bytes.len() > 4096 {
        return Err("invalid desktop descriptor".into());
    }
    let d: Value = serde_json::from_slice(&bytes)?;
    let pid = d["pid"].as_u64().ok_or("invalid init identity")? as u32;
    let pinned = ProcessHandle::open(pid)?;
    if Some(pinned.birth()) != d["birth"].as_u64() || pinned.has_exited()? {
        return Err("stale init descriptor".into());
    }
    let port = d["port"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 65535)
        .ok_or("invalid session port")?;
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse()?,
        Duration::from_secs(2),
    )?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(35)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write(&mut stream, &json!({"role":"client", "token":d["token"]}))?;
    // One byte at a time here avoids losing read-ahead data when transferring ownership.
    let mut response = BufReader::with_capacity(1, stream.try_clone()?);
    let reply = read(&mut response)?;
    if reply["ok"] != true {
        return Err("init authentication failed".into());
    }
    Ok(stream)
}
pub fn ensure(options: &Options) -> Result<TcpStream> {
    if let Ok(stream) = connect(options) {
        return Ok(stream);
    }
    fs::create_dir_all(options.control_dir())?;
    let log_path = options.control_dir().join("init.log");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    // Follow a separate read handle, starting before the daemon can write. Do
    // not give the daemon our console/pipes: it must survive client detachment.
    let mut progress = StartupLog::new(fs::File::open(&log_path)?)?;
    let mut command = Command::new(options.dist.join("init.exe"));
    command
        .arg("daemon")
        .arg("--root")
        .arg(&options.root)
        .arg("--dist")
        .arg(&options.dist);
    if let Some(manifest) = &options.manifest {
        command.arg("--rootfs-manifest").arg(manifest);
    }
    if options.no_tray {
        command.arg("--no-tray");
    }
    if let Some(web) = &options.web {
        command.arg("--web").arg(web);
    }
    // A detached daemon must not inherit the client's original pipe handles.
    // Otherwise communicate()/shell pipelines wait for EOF until the whole
    // environment shuts down, even though the launch client has already exited.
    let inheritance = StandardHandleInheritance::clear();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(0x08000000)
        .spawn()?;
    drop(inheritance);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(600);
    let mut last_progress = started;
    eprintln!("Kinakaze: preparing or reconnecting to the environment...");
    eprintln!("Startup log: {}", log_path.display());
    loop {
        if progress.forward(false)? {
            last_progress = Instant::now();
        }
        if let Ok(stream) = connect(options) {
            progress.forward(true)?;
            eprintln!("Kinakaze: environment ready.");
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            // Another concurrent launcher may own the root lock. Exit 2 means reuse.
            if status.code() != Some(2) {
                progress.forward(true)?;
                return Err(format!("init failed ({status}); see {}", log_path.display()).into());
            }
        }
        if Instant::now() >= deadline {
            progress.forward(true)?;
            return Err(
                format!("environment is still starting; see {}", log_path.display()).into(),
            );
        }
        if last_progress.elapsed() >= Duration::from_secs(5) {
            eprintln!(
                "Kinakaze: still preparing the environment ({}s elapsed)...",
                started.elapsed().as_secs()
            );
            last_progress = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
struct StartupLog {
    file: fs::File,
    pending: Vec<u8>,
}
impl StartupLog {
    fn new(mut file: fs::File) -> io::Result<Self> {
        // Old runs stay on disk but must not replay as this run's progress.
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            pending: Vec::new(),
        })
    }
    fn forward(&mut self, finish: bool) -> io::Result<bool> {
        (&mut self.file)
            .take(MAX_FRAME as u64)
            .read_to_end(&mut self.pending)?;
        // eprintln may append one line in several writes. Keep incomplete UTF-8
        // and lines until the next poll so Windows console output stays valid.
        let end = if finish || self.pending.len() >= MAX_FRAME {
            self.pending.len()
        } else {
            self.pending
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |i| i + 1)
        };
        if end == 0 {
            return Ok(false);
        }
        let mut stderr = io::stderr().lock();
        stderr.write_all(String::from_utf8_lossy(&self.pending[..end]).as_bytes())?;
        stderr.flush()?;
        self.pending.drain(..end);
        Ok(true)
    }
}
struct StandardHandleInheritance(Vec<(isize, u32)>);
impl StandardHandleInheritance {
    fn clear() -> Self {
        use windows_sys::Win32::{Foundation::*, System::Console::*};
        let mut saved = vec![];
        for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            unsafe {
                let handle = GetStdHandle(kind);
                let mut flags = 0;
                if GetHandleInformation(handle, &mut flags) != 0 && flags & HANDLE_FLAG_INHERIT != 0
                {
                    if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0 {
                        saved.push((handle as isize, flags));
                    }
                }
            }
        }
        Self(saved)
    }
}
impl Drop for StandardHandleInheritance {
    fn drop(&mut self) {
        for &(handle, flags) in &self.0 {
            unsafe {
                windows_sys::Win32::Foundation::SetHandleInformation(
                    handle as _,
                    windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
                    flags,
                );
            }
        }
    }
}
pub fn request(stream: &mut TcpStream, value: Value) -> Result<Value> {
    write(stream, &value)?;
    let reply = read(&mut BufReader::with_capacity(1, stream.try_clone()?))?;
    if let Some(error) = reply.get("error") {
        return Err(error
            .as_str()
            .unwrap_or("session request failed")
            .to_owned()
            .into());
    }
    Ok(reply)
}
pub fn open_terminal(options: &Options, name: &str) -> io::Result<()> {
    Command::new(options.dist.join("worker.exe"))
        .arg("session")
        .arg("attach")
        .arg("--root")
        .arg(&options.root)
        .arg("--dist")
        .arg(&options.dist)
        .arg("--name")
        .arg(name)
        .creation_flags(0x10)
        .spawn()?;
    Ok(())
}
pub fn run(arguments: Vec<String>, window: bool) -> Result<i32> {
    let options = Options::parse(arguments)?;
    let mut stream = if matches!(options.action.as_str(), "status" | "stop" | "attach") {
        connect(&options)?
    } else {
        ensure(&options)?
    };
    if options.action == "stop" {
        request(&mut stream, json!({"op":"shutdown"}))?;
        return Ok(0);
    }
    if options.action == "status" {
        println!("{}", request(&mut stream, json!({"op":"status"}))?);
        return Ok(0);
    }
    let state = request(&mut stream, json!({"op":"status"}))?;
    let name = if let Some(name) = &options.name {
        name.clone()
    } else if options.command.is_empty() {
        state["default_terminal"]
            .as_str()
            .ok_or("manifest has no default terminal")?
            .into()
    } else {
        let label: String = options.command[0]
            .rsplit('/')
            .next()
            .unwrap_or("app")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .take(24)
            .collect();
        format!(
            "{}-{}",
            if label.is_empty() { "app" } else { &label },
            &kinakaze_v2_host_win::random_token()?[..8]
        )
    };
    let mut start = json!({"op":"start", "name":name});
    if !options.command.is_empty() {
        start["terminal"] = json!({"name":name, "command":options.command, "cwd":options.cwd, "environment":{}, "autostart":true});
    }
    // Attach is deliberately not start/restart: an exited terminal stays exited.
    if options.action != "attach" {
        request(&mut stream, start)?;
    }
    if options.action == "start" {
        return Ok(0);
    }
    if window {
        open_terminal(&options, &name)?;
        return Ok(0);
    }
    attach(stream, &name)
}

struct Console {
    input: isize,
    output: isize,
    input_mode: Option<u32>,
    output_mode: Option<u32>,
}
impl Console {
    fn raw() -> Self {
        use windows_sys::Win32::System::Console::*;
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            let (mut im, mut om) = (0, 0);
            let input_mode = (GetConsoleMode(input, &mut im) != 0).then_some(im);
            let output_mode = (GetConsoleMode(output, &mut om) != 0).then_some(om);
            if input_mode.is_some() {
                // With Quick Edit disabled, the inherited Win32 mouse flag
                // makes ConPTY request all mouse motion (1003/1006), even for
                // a plain shell. Only guest VT sequences should request mouse
                // reporting; otherwise readline inserts SGR coordinates.
                SetConsoleMode(
                    input,
                    (im & !(ENABLE_LINE_INPUT
                        | ENABLE_ECHO_INPUT
                        | ENABLE_PROCESSED_INPUT
                        | ENABLE_MOUSE_INPUT
                        | ENABLE_WINDOW_INPUT
                        | ENABLE_QUICK_EDIT_MODE))
                        | ENABLE_VIRTUAL_TERMINAL_INPUT
                        | ENABLE_EXTENDED_FLAGS,
                );
            }
            if output_mode.is_some() {
                SetConsoleMode(
                    output,
                    om | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN,
                );
            }
            let console = Self {
                input: input as isize,
                output: output as isize,
                input_mode,
                output_mode,
            };
            console.reset_input_modes();
            console
        }
    }
    fn reset_input_modes(&self) {
        if self.output_mode.is_some() {
            // VT modes are independent of Get/SetConsoleMode. Start clean and
            // release them on detach/exit, including while a guest TUI remains
            // alive. Replayed guest output restores its requested modes on attach.
            let mut output = io::stdout().lock();
            let _ = output.write_all(b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?2004l");
            let _ = output.flush();
        }
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.reset_input_modes();
        unsafe {
            if let Some(mode) = self.input_mode {
                windows_sys::Win32::System::Console::SetConsoleMode(self.input as _, mode);
            }
            if let Some(mode) = self.output_mode {
                windows_sys::Win32::System::Console::SetConsoleMode(self.output as _, mode);
            }
        }
    }
}
fn dimensions() -> (i16, i16) {
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let mut info = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) != 0 {
            (
                info.srWindow.Bottom - info.srWindow.Top + 1,
                info.srWindow.Right - info.srWindow.Left + 1,
            )
        } else {
            (30, 120)
        }
    }
}
fn read_input(buffer: &mut [u8]) -> io::Result<usize> {
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return io::stdin().read(buffer);
        }
        // Rust's Windows Stdin treats leading Ctrl+Z as EOF. A raw terminal
        // must forward it as VSUSP, alongside Ctrl+C/D and VT key sequences.
        let mut wide = [0u16; 1024];
        let mut count = 0;
        if ReadConsoleW(
            handle,
            wide.as_mut_ptr().cast(),
            wide.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let text = String::from_utf16_lossy(&wide[..count as usize]);
        buffer[..text.len()].copy_from_slice(text.as_bytes());
        Ok(text.len())
    }
}
fn attach(mut stream: TcpStream, name: &str) -> Result<i32> {
    request(&mut stream, json!({"op":"attach", "name":name}))?;
    stream.set_read_timeout(None)?;
    let _console = Console::raw();
    eprintln!("\r\nKinakaze [{name}] — Ctrl+] detaches; the process keeps running.\r");
    let writer = std::sync::Arc::new(std::sync::Mutex::new(stream.try_clone()?));
    let input = writer.clone();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count) = read_input(&mut buffer) {
            if count == 0 {
                break;
            }
            let end = buffer[..count].iter().position(|&b| b == 0x1d);
            let data = &buffer[..end.unwrap_or(count)];
            let mut out = input.lock().unwrap();
            if !data.is_empty()
                && write(&mut *out, &json!({"op":"input", "data":hex(data)})).is_err()
            {
                break;
            }
            if end.is_some() {
                let _ = out.shutdown(Shutdown::Both);
                break;
            }
        }
    });
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = done.clone();
    std::thread::spawn(move || {
        let mut old = (0, 0);
        while !stopped.load(std::sync::atomic::Ordering::Acquire) {
            let size = dimensions();
            if size != old {
                if write(
                    &mut *writer.lock().unwrap(),
                    &json!({"op":"resize", "rows":size.0, "cols":size.1}),
                )
                .is_err()
                {
                    break;
                }
                old = size;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    let mut reader = BufReader::new(stream);
    let mut status = 0;
    while let Ok(event) = read(&mut reader) {
        if event["event"] == "shutdown" {
            eprintln!("\r\nKinakaze: environment shutting down.\r");
            break;
        }
        if let Some(data) = event["data"].as_str() {
            io::stdout().write_all(&unhex(data)?)?;
            io::stdout().flush()?;
        }
        if event["event"] == "exit" {
            status = event["status"].as_i64().unwrap_or(1) as i32;
            break;
        }
        if event["event"] == "error" {
            eprintln!("\r\n{}", event["error"]);
            status = 1;
            break;
        }
    }
    done.store(true, std::sync::atomic::Ordering::Release);
    let _ = reader.get_ref().shutdown(Shutdown::Both);
    Ok(if status < 0 { 128 - status } else { status })
}
