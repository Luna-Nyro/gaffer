//! Getting the DMX universe out of the computer: finding USB-DMX interfaces, talking to them,
//! and the output thread that sends a frame about 40 times a second and reconnects when the
//! interface is unplugged and back. Three kinds of interface:
//! - Enttec DMX USB Pro protocol ("smart" boxes that do the DMX timing themselves),
//! - Open DMX: the cheapest cables, a plain USB-serial chip (FTDI FT232R, WCH CH340, Silicon Labs
//!   CP210x, Prolific) wired to an RS-485 driver, where the computer makes the DMX timing,
//! - uDMX: small Anyma-style dongles driven by USB control transfers (no serial port).

use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const UNIVERSE: usize = 512;
/// Frames per second sent to the interface.
pub const FPS: f64 = 40.0;

/// How the interface wants its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Pick from what the interface says it is.
    Auto,
    /// Enttec DMX USB Pro protocol (Enttec Pro, DMXking ultraDMX, most "smart" interfaces).
    Pro,
    /// A plain USB-serial chip where the computer does the DMX timing (Enttec Open DMX, cheap
    /// FTDI/CH340/CP2102 "USB to DMX512" cables).
    Open,
    /// Anyma uDMX and clones (USB 16c0:05dc).
    Udmx,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Auto => "auto",
            Kind::Pro => "pro",
            Kind::Open => "open",
            Kind::Udmx => "udmx",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Auto => "Automatic",
            Kind::Pro => "Enttec Pro protocol",
            Kind::Open => "Open DMX (computer timing)",
            Kind::Udmx => "uDMX",
        }
    }

    pub fn from_key(s: &str) -> Kind {
        match s {
            "pro" => Kind::Pro,
            "open" => Kind::Open,
            "udmx" => Kind::Udmx,
            _ => Kind::Auto,
        }
    }
}

/// A serial port that might be a DMX interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub path: String,
    pub vendor: String,
    pub product_id: String,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    /// What it looks like, if we can tell.
    pub guess: Option<Kind>,
}

impl Port {
    pub fn label(&self) -> String {
        let name = [self.manufacturer.as_str(), self.product.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
        let dev = if self.path.starts_with(UDMX_PREFIX) { "USB".to_string() } else { Path::new(&self.path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() };
        if name.is_empty() { dev } else { format!("{name} ({dev})") }
    }

    pub fn looks_like_dmx(&self) -> bool {
        self.guess.is_some()
    }
}

/// Port paths of uDMX dongles start with this (they have no /dev/tty node).
pub const UDMX_PREFIX: &str = "udmx:";

/// USB-serial chips that the cheap "USB to DMX512" cables are built from.
const SERIAL_CHIPS: &[&str] = &["0403", "1a86", "10c4", "067b"]; // FTDI, WCH CH340/CH9102, Silicon Labs CP210x, Prolific

/// What the IDs and strings a USB device reports say about it.
pub fn guess_kind(vendor: &str, product_id: &str, manufacturer: &str, product: &str) -> Option<Kind> {
    let text = format!("{manufacturer} {product}").to_lowercase();
    let vendor = vendor.to_ascii_lowercase();
    if vendor == "16c0" && product_id.eq_ignore_ascii_case("05dc") && (text.contains("udmx") || text.contains("anyma")) {
        return Some(Kind::Udmx);
    }
    if text.contains("usb pro") || text.contains("usbpro") || text.contains("dmx512 pro") || text.contains("ultradmx") || text.contains("dmxking") || text.contains("rdm pro") {
        return Some(Kind::Pro);
    }
    if text.contains("open dmx") || text.contains("opendmx") {
        return Some(Kind::Open);
    }
    let chip = SERIAL_CHIPS.contains(&vendor.as_str());
    if text.contains("dmx") || text.contains("enttec") {
        // A bare serial chip that just calls itself "DMX" is a cheap cable, not a Pro.
        return Some(if chip { Kind::Open } else { Kind::Pro });
    }
    if chip {
        return Some(Kind::Open); // most cheap "USB DMX" cables report the chip's own name
    }
    None
}

/// USB serial ports, DMX-looking ones first. `sys` is normally "/sys".
pub fn list_ports_in(sys: &Path, dev: &Path) -> Vec<Port> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys.join("class/tty")) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("ttyUSB") || name.starts_with("ttyACM")) {
            continue;
        }
        let Ok(mut dir) = std::fs::canonicalize(e.path().join("device")) else { continue };
        let mut usb = None;
        for _ in 0..6 {
            if dir.join("idVendor").exists() {
                usb = Some(dir.clone());
                break;
            }
            if !dir.pop() {
                break;
            }
        }
        let read = |f: &str| usb.as_ref().and_then(|d| std::fs::read_to_string(d.join(f)).ok()).map(|s| s.trim().to_string()).unwrap_or_default();
        let (vendor, product_id, manufacturer, product) = (read("idVendor"), read("idProduct"), read("manufacturer"), read("product"));
        let guess = guess_kind(&vendor, &product_id, &manufacturer, &product);
        out.push(Port { path: dev.join(&name).to_string_lossy().into_owned(), product_id, serial: read("serial"), vendor, manufacturer, product, guess });
    }
    // uDMX dongles: plain USB devices, found by their IDs.
    if let Ok(rd) = std::fs::read_dir(sys.join("bus/usb/devices")) {
        for e in rd.flatten() {
            let d = e.path();
            let read = |f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
            let (vendor, product_id, manufacturer, product) = (read("idVendor"), read("idProduct"), read("manufacturer"), read("product"));
            if guess_kind(&vendor, &product_id, &manufacturer, &product) != Some(Kind::Udmx) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            out.push(Port { path: format!("{UDMX_PREFIX}{name}"), vendor, product_id, serial: read("serial"), manufacturer, product, guess: Some(Kind::Udmx) });
        }
    }
    out.sort_by(|a, b| b.looks_like_dmx().cmp(&a.looks_like_dmx()).then(a.path.cmp(&b.path)));
    out
}

pub fn list_ports() -> Vec<Port> {
    list_ports_in(Path::new("/sys"), Path::new("/dev"))
}

/// One Enttec Pro "Output Only Send DMX Packet" message (label 6): start code 0 + 512 channels.
pub fn encode_pro(frame: &[u8; UNIVERSE]) -> Vec<u8> {
    let len = UNIVERSE + 1;
    let mut v = Vec::with_capacity(len + 5);
    v.extend_from_slice(&[0x7e, 6, (len & 0xff) as u8, (len >> 8) as u8, 0]);
    v.extend_from_slice(frame);
    v.push(0xe7);
    v
}

/// Undoes `encode_pro` (for tests and the loopback check): every complete label-6 frame in `buf`.
pub fn decode_pro(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 5 <= buf.len() {
        if buf[i] != 0x7e {
            i += 1;
            continue;
        }
        let len = buf[i + 2] as usize | (buf[i + 3] as usize) << 8;
        let end = i + 4 + len;
        if end >= buf.len() {
            i += 1; // incomplete, or a stray 0x7e: look further
            continue;
        }
        if buf[end] == 0xe7 && buf[i + 1] == 6 && len >= 1 {
            out.push(buf[i + 4..end].to_vec());
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Opens a serial port raw at any baud rate (250000 for DMX), 8 data bits, `two_stop` stop bits.
pub fn open_serial(path: &str, baud: u32, two_stop: bool) -> std::io::Result<File> {
    let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC).open(path)?;
    set_line(&f, baud, two_stop)?;
    Ok(f)
}

/// Raw 8-bit line at any baud rate.
fn set_line(f: &File, baud: u32, two_stop: bool) -> std::io::Result<()> {
    let fd = f.as_raw_fd();
    unsafe {
        let mut t: libc::termios2 = std::mem::zeroed();
        if libc::ioctl(fd, libc::TCGETS2, &mut t) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        t.c_iflag = 0;
        t.c_oflag = 0;
        t.c_lflag = 0;
        t.c_cflag &= !(libc::CBAUD | libc::CSIZE | libc::PARENB | libc::CSTOPB | libc::CRTSCTS);
        t.c_cflag |= libc::BOTHER | libc::CS8 | libc::CLOCAL | libc::CREAD;
        if two_stop {
            t.c_cflag |= libc::CSTOPB;
        }
        t.c_ispeed = baud;
        t.c_ospeed = baud;
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        if libc::ioctl(fd, libc::TCSETS2, &t) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// How an Open DMX cable makes the DMX "break" that starts each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakMode {
    /// The serial driver's break control (FTDI, CP210x, recent CH340 drivers).
    Ioctl,
    /// For chips whose driver can't: send one 0x00 byte at 57600 baud (156 µs low = break, then
    /// the stop bit = mark after break), then switch back to 250000 baud for the data.
    SlowByte,
}

/// The baud rate whose 0x00 byte makes the break in `BreakMode::SlowByte`.
pub const BREAK_BAUD: u32 = 57_600;

fn drain(f: &File) {
    unsafe {
        libc::tcdrain(f.as_raw_fd());
    }
}

/// One Open DMX frame: break, mark after break, start code 0, 512 channels. Switches `mode` to
/// `SlowByte` for good if the driver has no break control.
pub fn send_open_dmx(f: &mut File, frame: &[u8; UNIVERSE], mode: &mut BreakMode) -> std::io::Result<()> {
    let fd = f.as_raw_fd();
    if *mode == BreakMode::Ioctl {
        // Break (≥ 88 µs), mark after break (≥ 8 µs).
        if unsafe { libc::ioctl(fd, libc::TIOCSBRK) } == 0 {
            std::thread::sleep(Duration::from_micros(110));
            unsafe { libc::ioctl(fd, libc::TIOCCBRK) };
            std::thread::sleep(Duration::from_micros(16));
        } else {
            let e = std::io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::ENOTTY) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => *mode = BreakMode::SlowByte,
                _ => return Err(e),
            }
        }
    }
    if *mode == BreakMode::SlowByte {
        set_line(f, BREAK_BAUD, false)?;
        f.write_all(&[0])?;
        drain(f);
        set_line(f, 250_000, true)?;
    }
    let mut buf = Vec::with_capacity(UNIVERSE + 1);
    buf.push(0);
    buf.extend_from_slice(frame);
    f.write_all(&buf)?;
    drain(f);
    Ok(())
}

// ---- uDMX ---------------------------------------------------------------------------------------

#[repr(C)]
struct CtrlTransfer {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    timeout: u32,
    data: *mut libc::c_void,
}

/// _IOWR('U', 0, struct usbdevfs_ctrltransfer)
const USBDEVFS_CONTROL: u64 = (3 << 30) | ((std::mem::size_of::<CtrlTransfer>() as u64) << 16) | ((b'U' as u64) << 8);

/// The /dev/bus/usb node of a USB device named like "1-1.2" in sysfs.
fn usb_node(sys: &Path, name: &str) -> Option<String> {
    let d = sys.join("bus/usb/devices").join(name);
    let num = |f: &str| std::fs::read_to_string(d.join(f)).ok()?.trim().parse::<u32>().ok();
    Some(format!("/dev/bus/usb/{:03}/{:03}", num("busnum")?, num("devnum")?))
}

/// uDMX "set channel range": vendor request 2, value = channel count, index = first channel.
fn send_udmx(f: &File, frame: &[u8; UNIVERSE]) -> std::io::Result<()> {
    let mut data = *frame;
    let mut c = CtrlTransfer { request_type: 0x40, request: 2, value: UNIVERSE as u16, index: 0, length: UNIVERSE as u16, timeout: 500, data: data.as_mut_ptr().cast() };
    let r = unsafe { libc::ioctl(f.as_raw_fd(), USBDEVFS_CONTROL as _, &mut c) };
    if r < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// Where the output thread sends.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Nowhere: the on-screen stage is the only light.
    Simulator,
    Port { path: String, serial: Option<String>, kind: Kind },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Simulator,
    Connected,
    /// The interface isn't there (unplugged); still trying.
    Waiting,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub link: Link,
    /// Words for the user.
    pub detail: String,
    pub frames: u64,
    /// The port actually in use.
    pub path: Option<String>,
    pub kind: Option<Kind>,
}

struct Shared {
    target: Mutex<Target>,
    status: Mutex<Status>,
    generation: AtomicU64,
    stop: AtomicBool,
}

/// The running output thread. Dropping it stops the thread (after sending one dark frame).
pub struct Output {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

pub type Source = Box<dyn FnMut(f64) -> [u8; UNIVERSE] + Send>;

impl Output {
    /// Starts sending. `source` is asked for each frame with the seconds since start.
    pub fn start(target: Target, source: Source) -> Output {
        let shared = Arc::new(Shared {
            target: Mutex::new(target),
            status: Mutex::new(Status { link: Link::Simulator, detail: "Starting".into(), frames: 0, path: None, kind: None }),
            generation: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        let s2 = shared.clone();
        let thread = std::thread::Builder::new().name("gaffer-dmx".into()).spawn(move || run(s2, source)).ok();
        Output { shared, thread }
    }

    pub fn set_target(&self, t: Target) {
        let mut cur = self.shared.target.lock().unwrap_or_else(|e| e.into_inner());
        if *cur != t {
            *cur = t;
            self.shared.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn target(&self) -> Target {
        self.shared.target.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Reconnect now (after the user plugged something in).
    pub fn retry(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Live {
    file: File,
    kind: Kind,
    path: String,
    brk: BreakMode,
}

fn set_status(sh: &Shared, f: impl FnOnce(&mut Status)) {
    f(&mut sh.status.lock().unwrap_or_else(|e| e.into_inner()));
}

fn friendly(e: &std::io::Error, path: &str) -> String {
    match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) if path.starts_with("/dev/bus/usb") => "No permission to use the uDMX dongle. Run Gaffer's install.sh once (it adds a udev rule), then plug the dongle in again.".into(),
        Some(libc::EACCES) | Some(libc::EPERM) => format!("No permission to use {path}. Add yourself to the “dialout” group and log in again."),
        Some(libc::ENOENT) | Some(libc::ENODEV) | Some(libc::ENXIO) => "Interface unplugged. Waiting for it to come back…".into(),
        Some(libc::EBUSY) => format!("{path} is in use by another program."),
        Some(libc::EIO) => "The interface stopped answering (unplugged?). Retrying…".into(),
        _ => format!("{path}: {e}"),
    }
}

fn connect(target: &Target) -> Result<Live, (Link, String)> {
    let Target::Port { path, serial, kind } = target else { return Err((Link::Simulator, String::new())) };
    // Find it again by serial number if it came back under another name.
    let ports = list_ports();
    let found = serial.as_ref().filter(|s| !s.is_empty()).and_then(|s| ports.iter().find(|p| &p.serial == s));
    let port = found.or_else(|| ports.iter().find(|p| &p.path == path));
    let use_path = port.map(|p| p.path.clone()).unwrap_or_else(|| path.clone());
    let kind = match kind {
        _ if use_path.starts_with(UDMX_PREFIX) => Kind::Udmx,
        Kind::Auto => port.and_then(|p| p.guess).unwrap_or(Kind::Pro),
        k => *k,
    };
    let (opened, use_path) = match kind {
        Kind::Udmx => match usb_node(Path::new("/sys"), use_path.trim_start_matches(UDMX_PREFIX)) {
            Some(node) => (std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(&node), node),
            None => (Err(std::io::Error::from_raw_os_error(libc::ENODEV)), use_path),
        },
        Kind::Open => (open_serial(&use_path, 250_000, true), use_path),
        _ => (open_serial(&use_path, 115_200, false), use_path), // the Pro ignores the host baud rate
    };
    match opened {
        Ok(file) => Ok(Live { file, kind, path: use_path, brk: BreakMode::Ioctl }),
        Err(e) => {
            let link = if matches!(e.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENODEV) | Some(libc::ENXIO)) { Link::Waiting } else { Link::Error };
            Err((link, friendly(&e, &use_path)))
        }
    }
}

fn run(sh: Arc<Shared>, mut source: Source) {
    let t0 = Instant::now();
    let period = Duration::from_secs_f64(1.0 / FPS);
    let mut next = Instant::now();
    let mut live: Option<Live> = None;
    let mut seen_gen = 0;
    let mut retry_at = Instant::now();
    let mut frames = 0u64;
    let mut last_status = Instant::now() - Duration::from_secs(1);
    loop {
        let stopping = sh.stop.load(Ordering::SeqCst);
        let mut frame = source(t0.elapsed().as_secs_f64());
        if stopping {
            frame = [0; UNIVERSE];
        }
        let generation = sh.generation.load(Ordering::SeqCst);
        if generation != seen_gen {
            seen_gen = generation;
            live = None;
            retry_at = Instant::now();
        }
        let target = sh.target.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if target == Target::Simulator {
            frames += 1;
            if last_status.elapsed() > Duration::from_millis(250) {
                last_status = Instant::now();
                set_status(&sh, |s| *s = Status { link: Link::Simulator, detail: "Simulator: no interface, lights show on screen only".into(), frames, path: None, kind: None });
            }
        } else {
            if live.is_none() && Instant::now() >= retry_at {
                match connect(&target) {
                    Ok(l) => {
                        let (path, kind) = (l.path.clone(), l.kind);
                        live = Some(l);
                        set_status(&sh, |s| {
                            *s = Status { link: Link::Connected, detail: format!("Sending to {path}"), frames, path: Some(path), kind: Some(kind) }
                        });
                    }
                    Err((link, detail)) => {
                        retry_at = Instant::now() + Duration::from_secs(1);
                        set_status(&sh, |s| *s = Status { link, detail, frames, path: None, kind: None });
                    }
                }
            }
            if let Some(l) = &mut live {
                let sent = match l.kind {
                    Kind::Open => send_open_dmx(&mut l.file, &frame, &mut l.brk),
                    Kind::Udmx => send_udmx(&l.file, &frame),
                    _ => l.file.write_all(&encode_pro(&frame)),
                };
                match sent {
                    Ok(()) => {
                        frames += 1;
                        if last_status.elapsed() > Duration::from_millis(250) {
                            last_status = Instant::now();
                            set_status(&sh, |s| s.frames = frames);
                        }
                    }
                    Err(e) => {
                        let detail = friendly(&e, &l.path);
                        live = None;
                        retry_at = Instant::now() + Duration::from_secs(1);
                        set_status(&sh, |s| *s = Status { link: Link::Waiting, detail, frames, path: None, kind: None });
                    }
                }
            }
        }
        if stopping {
            return;
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now; // fell behind (machine was asleep): don't try to catch up
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn pro_round_trip() {
        let mut f = [0u8; UNIVERSE];
        f[0] = 255;
        f[511] = 7;
        let p = encode_pro(&f);
        assert_eq!(&p[..5], &[0x7e, 6, 0x01, 0x02, 0]);
        assert_eq!(p.len(), 518);
        assert_eq!(*p.last().unwrap(), 0xe7);
        let mut stream = vec![1, 2, 0x7e];
        stream.extend(&p);
        stream.extend(&p[..100]);
        let got = decode_pro(&stream);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0][0], 0);
        assert_eq!(&got[0][1..], &f[..]);
    }

    #[test]
    fn guesses() {
        assert_eq!(guess_kind("0403", "6001", "ENTTEC", "DMX USB PRO"), Some(Kind::Pro));
        assert_eq!(guess_kind("0403", "6001", "DMXking.com", "ultraDMX Micro"), Some(Kind::Pro));
        assert_eq!(guess_kind("04d8", "fa63", "Eurolite", "USB-DMX512 PRO"), Some(Kind::Pro));
        assert_eq!(guess_kind("0403", "6001", "FTDI", "FT232R USB UART"), Some(Kind::Open));
        assert_eq!(guess_kind("0403", "6001", "FTDI", "USB DMX512"), Some(Kind::Open));
        assert_eq!(guess_kind("1a86", "7523", "", "USB Serial"), Some(Kind::Open));
        assert_eq!(guess_kind("10c4", "ea60", "Silicon Labs", "CP2102 USB to UART Bridge Controller"), Some(Kind::Open));
        assert_eq!(guess_kind("16c0", "05dc", "www.anyma.ch", "uDMX"), Some(Kind::Udmx));
        assert_eq!(guess_kind("16c0", "05dc", "someone", "V-USB thing"), None);
        assert_eq!(guess_kind("2341", "0043", "Arduino", "Uno"), None);
    }

    #[test]
    fn finds_ports_in_sysfs() {
        let root = std::env::temp_dir().join(format!("gaffer-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let usb = root.join("devices/pci0/usb1/1-1");
        let port = usb.join("1-1:1.0/ttyUSB0");
        std::fs::create_dir_all(&port).unwrap();
        for (f, v) in [("idVendor", "0403\n"), ("idProduct", "6001"), ("manufacturer", "ENTTEC"), ("product", "DMX USB PRO"), ("serial", "EN123")] {
            std::fs::write(usb.join(f), v).unwrap();
        }
        std::fs::create_dir_all(root.join("class/tty/ttyUSB0")).unwrap();
        std::os::unix::fs::symlink(&port, root.join("class/tty/ttyUSB0/device")).unwrap();
        std::fs::create_dir_all(root.join("class/tty/tty1")).unwrap();
        let udmx = root.join("bus/usb/devices/1-1.4");
        std::fs::create_dir_all(&udmx).unwrap();
        for (f, v) in [("idVendor", "16c0"), ("idProduct", "05dc"), ("manufacturer", "www.anyma.ch"), ("product", "uDMX"), ("busnum", "1"), ("devnum", "9")] {
            std::fs::write(udmx.join(f), v).unwrap();
        }
        let ports = list_ports_in(&root, Path::new("/dev"));
        let node = usb_node(&root, "1-1.4");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(node.as_deref(), Some("/dev/bus/usb/001/009"));
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[1].path, "udmx:1-1.4");
        assert_eq!(ports[1].guess, Some(Kind::Udmx));
        assert_eq!(ports[1].label(), "www.anyma.ch uDMX (USB)");
        assert_eq!(ports[0].path, "/dev/ttyUSB0");
        assert_eq!(ports[0].serial, "EN123");
        assert_eq!(ports[0].guess, Some(Kind::Pro));
        assert_eq!(ports[0].label(), "ENTTEC DMX USB PRO (ttyUSB0)");
    }

    /// The real output thread writing Enttec Pro packets into a pseudo-terminal.
    #[test]
    fn sends_pro_packets_over_a_pty() {
        let (master, slave) = pty();
        let out = Output::start(
            Target::Port { path: slave.clone(), serial: None, kind: Kind::Pro },
            Box::new(|t| {
                let mut f = [0u8; UNIVERSE];
                f[0] = 200;
                f[9] = (t * 10.0) as u8;
                f
            }),
        );
        let mut master = master;
        let mut got = Vec::new();
        let start = Instant::now();
        let mut buf = [0u8; 4096];
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
        while start.elapsed() < Duration::from_millis(600) {
            if let Ok(n) = master.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let st = out.status();
        drop(out);
        assert_eq!(st.link, Link::Connected, "{st:?}");
        let frames = decode_pro(&got);
        assert!(frames.len() >= 15, "{} frames in 0.6 s", frames.len());
        assert!(frames.iter().all(|f| f.len() == 513 && f[0] == 0 && f[1] == 200));
    }

    fn pty() -> (File, String) {
        unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(m >= 0);
            assert_eq!(libc::grantpt(m), 0);
            assert_eq!(libc::unlockpt(m), 0);
            let mut buf = [0 as libc::c_char; 128];
            assert_eq!(libc::ptsname_r(m, buf.as_mut_ptr(), buf.len()), 0);
            let name = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
            libc::fcntl(m, libc::F_SETFL, libc::O_NONBLOCK);
            (<File as std::os::fd::FromRawFd>::from_raw_fd(m), name)
        }
    }

    fn read_all(f: &mut File) -> Vec<u8> {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        got
    }

    /// Open DMX frames in both break modes: the bytes on the wire are the start code and channels
    /// (plus the one break byte in SlowByte mode).
    #[test]
    fn open_dmx_frames_on_a_pty() {
        let mut frame = [0u8; UNIVERSE];
        frame[0] = 11;
        frame[511] = 22;
        for start in [BreakMode::Ioctl, BreakMode::SlowByte] {
            let (mut master, slave) = pty();
            let mut f = open_serial(&slave, 250_000, true).unwrap();
            let mut mode = start;
            send_open_dmx(&mut f, &frame, &mut mode).unwrap();
            send_open_dmx(&mut f, &frame, &mut mode).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            let got = read_all(&mut master);
            let lead = if mode == BreakMode::SlowByte { 1 } else { 0 };
            let one = lead + 1 + UNIVERSE;
            assert_eq!(got.len(), 2 * one, "{start:?} -> {mode:?}");
            for k in 0..2 {
                let fr = &got[k * one..(k + 1) * one];
                assert!(fr[..lead + 1].iter().all(|&b| b == 0));
                assert_eq!(&fr[lead + 1..], &frame[..]);
            }
        }
    }

    #[test]
    fn udmx_request_layout() {
        assert_eq!(std::mem::size_of::<CtrlTransfer>(), 24);
        assert_eq!(USBDEVFS_CONTROL, 0xc018_5500);
    }

    #[test]
    fn missing_port_waits_and_simulator_counts() {
        let out = Output::start(Target::Port { path: "/dev/ttyUSB-gaffer-none".into(), serial: None, kind: Kind::Auto }, Box::new(|_| [0; UNIVERSE]));
        std::thread::sleep(Duration::from_millis(100));
        let st = out.status();
        assert_eq!(st.link, Link::Waiting, "{st:?}");
        out.set_target(Target::Simulator);
        std::thread::sleep(Duration::from_millis(400));
        let st = out.status();
        assert_eq!(st.link, Link::Simulator);
        assert!(st.frames > 5);
    }
}
