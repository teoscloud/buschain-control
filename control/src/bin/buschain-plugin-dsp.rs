//! Per-track sandboxed DSP helper (P6).
//!
//! Hosts one plugin, processes SHM stereo rings, accepts control/MIDI over a unix socket.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use buschain_engine::{AudioProcessor, ClapInstance, MidiEvent, Vst3Instance};

fn main() {
    if let Err(e) = run() {
        eprintln!("buschain-plugin-dsp: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut shm = String::new();
    let mut sock = String::new();
    let mut path = String::new();
    let mut key = String::new();
    let mut format = String::from("clap");
    let mut rate = 48_000u32;
    let mut block = 256usize;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--shm" => shm = args.next().unwrap_or_default(),
            "--sock" => sock = args.next().unwrap_or_default(),
            "--path" => path = args.next().unwrap_or_default(),
            "--key" => key = args.next().unwrap_or_default(),
            "--format" => format = args.next().unwrap_or_default(),
            "--rate" => rate = args.next().and_then(|s| s.parse().ok()).unwrap_or(rate),
            "--block" => block = args.next().and_then(|s| s.parse().ok()).unwrap_or(block),
            _ => {}
        }
    }
    if shm.is_empty() || sock.is_empty() || path.is_empty() {
        bail!("usage: buschain-plugin-dsp --shm NAME --sock PATH --path PLUGIN --key ID --format clap|vst3");
    }

    let bytes = std::mem::size_of::<ShmHeader>() + block * 4 * 4;
    let file = open_shm(&shm, bytes)?;
    let mut map = unsafe { memmap2::MmapMut::map_mut(&file)? };
    let hdr = unsafe { &*(map.as_ptr() as *const ShmHeader) };

    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).context("bind ctrl sock")?;
    listener.set_nonblocking(true)?;

    let mut proc = load_processor(&format, &path, &key, rate, block as u32)?;
    let mut stream: Option<UnixStream> = None;
    let mut last_seq = 0u32;
    let mut ctrl_buf = Vec::new();

    while hdr.alive.load(Ordering::Relaxed) != 0 {
        if stream.is_none() {
            if let Ok((s, _)) = listener.accept() {
                let _ = s.set_nonblocking(true);
                stream = Some(s);
            }
        }
        if let Some(ref mut s) = stream {
            let mut buf = [0u8; 512];
            match s.read(&mut buf) {
                Ok(0) => stream = None,
                Ok(n) => {
                    ctrl_buf.extend_from_slice(&buf[..n]);
                    while let Some(pos) = ctrl_buf.iter().position(|&b| b == b'\n') {
                        let line = String::from_utf8_lossy(&ctrl_buf[..pos]).to_string();
                        ctrl_buf.drain(..=pos);
                        handle_ctrl_line(&mut proc, line.trim());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => stream = None,
            }
        }

        let seq = hdr.seq_in.load(Ordering::Acquire);
        if seq != last_seq {
            let n = (hdr.frames.load(Ordering::Acquire) as usize).min(block);
            let base =
                unsafe { map.as_mut_ptr().add(std::mem::size_of::<ShmHeader>()) as *mut f32 };
            let (in_l, in_r, out_l, out_r) = unsafe {
                (
                    std::slice::from_raw_parts(base, n),
                    std::slice::from_raw_parts(base.add(block), n),
                    std::slice::from_raw_parts_mut(base.add(block * 2), n),
                    std::slice::from_raw_parts_mut(base.add(block * 3), n),
                )
            };
            out_l.copy_from_slice(in_l);
            out_r.copy_from_slice(in_r);
            proc.process(out_l, out_r);
            hdr.seq_out.store(seq, Ordering::Release);
            last_seq = seq;
        } else {
            thread::sleep(Duration::from_micros(100));
        }
    }
    Ok(())
}

fn handle_ctrl_line(proc: &mut Box<dyn AudioProcessor>, line: &str) {
    if line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("P") => {
            let idx: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let val: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            proc.set_control(idx, val);
        }
        Some("N") => {
            let channel = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let note = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let velocity = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            proc.feed_midi(&[MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            }]);
        }
        Some("F") => {
            let channel = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let note = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let velocity = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            proc.feed_midi(&[MidiEvent::NoteOff {
                channel,
                note,
                velocity,
            }]);
        }
        Some("C") => {
            let channel = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let controller = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let value = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            proc.feed_midi(&[MidiEvent::Cc {
                channel,
                controller,
                value,
            }]);
        }
        _ => {}
    }
}

fn load_processor(
    format: &str,
    path: &str,
    key: &str,
    rate: u32,
    block: u32,
) -> Result<Box<dyn AudioProcessor>> {
    match format {
        "clap" => Ok(Box::new(ClapInstance::load(path, key, None, rate, block)?)),
        "vst3" => Ok(Box::new(Vst3Instance::load(path, key, None, rate, block)?)),
        other => bail!("unsupported sandbox format {other}"),
    }
}

#[repr(C)]
struct ShmHeader {
    magic: AtomicU32,
    seq_in: AtomicU32,
    seq_out: AtomicU32,
    frames: AtomicU32,
    alive: AtomicU32,
    _pad: AtomicU32,
}

fn open_shm(name: &str, size: usize) -> Result<std::fs::File> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::FromRawFd;
        let cname = std::ffi::CString::new(name.trim_start_matches('/'))?;
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDWR, 0o600) };
        if fd >= 0 {
            return Ok(unsafe { std::fs::File::from_raw_fd(fd) });
        }
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR unset — refuse temp-dir shm fallback"))?;
    let path = base.join(format!(
        "buschain-{}.shm",
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    ));
    let f = OpenOptions::new().read(true).write(true).open(&path)?;
    let _ = size;
    Ok(f)
}
