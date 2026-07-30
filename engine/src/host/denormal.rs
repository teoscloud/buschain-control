//! FTZ/DAZ around rack process (x86_64).

/// RAII scope that enables flush-to-zero / denormals-are-zero on x86_64.
pub struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    prev_mxcsr: u32,
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
        #[cfg(not(target_arch = "x86_64"))]
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
    }
}
