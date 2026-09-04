//! Host CPU / plugin binary architecture helpers (ELF + VST3 Contents dirs).

use std::path::Path;

/// ELF `e_machine` for the running process.
pub fn host_elf_machine() -> u16 {
    #[cfg(target_arch = "x86_64")]
    {
        62 // EM_X86_64
    }
    #[cfg(target_arch = "aarch64")]
    {
        183 // EM_AARCH64
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// VST3 Linux Contents subdirectory for this host (Steinberg layout).
pub fn host_vst3_contents_subdir() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "Contents/x86_64-linux"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "Contents/aarch64-linux"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "Contents/x86_64-linux"
    }
}

/// Human-readable host arch label for logs / errors.
pub fn host_arch_label() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "unknown"
    }
}

/// Read ELF `e_machine` from a file; `None` if not a recognizable ELF.
pub fn elf_machine(path: &Path) -> Option<u16> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 20];
    f.read_exact(&mut hdr).ok()?;
    if hdr[0..4] != [0x7f, b'E', b'L', b'F'] {
        return None;
    }
    // e_machine at offset 18 (EI_NIDENT=16 + e_type=2)
    Some(u16::from_le_bytes([hdr[18], hdr[19]]))
}

/// True if `path` is an ELF matching the host, or not an ELF (leave to dlopen).
pub fn elf_matches_host(path: &Path) -> bool {
    match elf_machine(path) {
        Some(m) => m == host_elf_machine(),
        None => true,
    }
}

/// True if a VST3 bundle directory has a host-arch Linux binary.
pub fn vst3_bundle_has_host_binary(bundle: &Path) -> bool {
    let host_dir = bundle.join(host_vst3_contents_subdir());
    if dir_has_matching_so(&host_dir) {
        return true;
    }
    // Top-level .so in the bundle (rare); require matching ELF.
    if let Ok(rd) = std::fs::read_dir(bundle) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("so") && elf_matches_host(&p) {
                return true;
            }
        }
    }
    false
}

fn dir_has_matching_so(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("so") && elf_matches_host(&p) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_machine_nonzero() {
        assert_ne!(host_elf_machine(), 0);
    }

    #[test]
    fn contents_subdir_matches_cfg() {
        let sub = host_vst3_contents_subdir();
        assert!(sub.starts_with("Contents/"));
        assert!(sub.ends_with("-linux"));
    }
}
