//! FTZ/DAZ (x86) / FPCR FZ (aarch64) around rack process.

/// RAII scope that enables flush-to-zero for denormals on supported arches.
pub struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    prev_mxcsr: u32,
    #[cfg(target_arch = "aarch64")]
    prev_fpcr: u64,
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn get_mxcsr() -> u32 {
    let mut mxcsr = 0u32;
    unsafe {
        core::arch::asm!(
            "stmxcsr [{ptr}]",
            ptr = in(reg) &mut mxcsr,
            options(nostack, preserves_flags)
        );
    }
    mxcsr
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn set_mxcsr(mxcsr: u32) {
    let mut val = mxcsr;
    unsafe {
        core::arch::asm!(
            "ldmxcsr [{ptr}]",
            ptr = in(reg) &mut val,
            options(nostack, preserves_flags)
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn get_fpcr() -> u64 {
    let fpcr: u64;
    unsafe {
        core::arch::asm!(
            "mrs {fpcr}, fpcr",
            fpcr = out(reg) fpcr,
            options(nomem, nostack, preserves_flags)
        );
    }
    fpcr
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn set_fpcr(fpcr: u64) {
    unsafe {
        core::arch::asm!(
            "msr fpcr, {fpcr}",
            fpcr = in(reg) fpcr,
            options(nomem, nostack, preserves_flags)
        );
    }
}

impl DenormalGuard {
    #[inline]
    pub fn enter() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let prev = get_mxcsr();
            // FTZ bit 15, DAZ bit 6
            set_mxcsr(prev | 0x8040);
            Self { prev_mxcsr: prev }
        }
        #[cfg(target_arch = "aarch64")]
        {
            let prev = get_fpcr();
            // FZ bit 24 — flush denormals to zero (IEEE default: gradual underflow).
            // FZ16 bit 19 — same for half-precision when present.
            set_fpcr(prev | (1 << 24) | (1 << 19));
            Self { prev_fpcr: prev }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self {}
        }
    }
}

impl Drop for DenormalGuard {
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            set_mxcsr(self.prev_mxcsr);
        }
        #[cfg(target_arch = "aarch64")]
        {
            set_fpcr(self.prev_fpcr);
        }
    }
}
