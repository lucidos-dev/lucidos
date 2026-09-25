//! One reading of the kernel's memory pressure, and what it may decide.
//!
//! Free RAM and the macOS compressor size both read "danger" on a healthy
//! host, and one critical kernel reading means nothing on its own. The e2e
//! memory guard learned each of those the hard way (ADRs 0175 to 0182).
//!
//! So a reading answers two questions. *Elevated* may open an episode alone:
//! on macOS the kernel says warn or worse AND swap is really in use. *Raised*
//! only explains an episode that slowness already opened (ADR 0283).

/// A sample that the host could read. Each variant exists only on the OS that
/// produces it, and under test on every OS, so a macOS build holds no Linux
/// reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PressureReading {
    #[cfg(any(target_os = "macos", test))]
    MacOs {
        /// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical.
        level: i32,
        swap_used_bytes: u64,
        ram_bytes: u64,
    },
    #[cfg(any(target_os = "linux", test))]
    Linux {
        /// `some avg60` from `/proc/pressure/memory`: the share of the last
        /// minute in which at least one task stalled on memory, in percent.
        some_avg60: f64,
    },
}

#[cfg(any(target_os = "macos", test))]
const MACOS_WARN: i32 = 2;
#[cfg(any(target_os = "macos", test))]
const SWAP_FLOOR_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[cfg(any(target_os = "linux", test))]
const PSI_SOME_AVG60_FLOOR: f64 = 10.0;

impl PressureReading {
    /// Strong enough to open an episode with no slow engine at all.
    pub fn is_elevated(self) -> bool {
        match self {
            #[cfg(any(target_os = "macos", test))]
            Self::MacOs {
                level,
                swap_used_bytes,
                ram_bytes,
            } => level >= MACOS_WARN && swap_used_bytes >= SWAP_FLOOR_BYTES.max(ram_bytes / 10),
            #[cfg(any(target_os = "linux", test))]
            Self::Linux { some_avg60 } => some_avg60 >= PSI_SOME_AVG60_FLOOR,
        }
    }

    /// Strong enough to name memory as the reason Lucidos is slow. The macOS
    /// level flaps on an idle host (ADR 0182), so it never opens one alone.
    pub fn is_raised(self) -> bool {
        match self {
            #[cfg(any(target_os = "macos", test))]
            Self::MacOs { level, .. } => level >= MACOS_WARN,
            #[cfg(any(target_os = "linux", test))]
            Self::Linux { .. } => self.is_elevated(),
        }
    }
}

/// Read this host, or `None` when it cannot be measured.
pub fn read() -> Option<PressureReading> {
    platform::read()
}

/// `avg60` from the `some` line of `/proc/pressure/memory`.
#[cfg(any(target_os = "linux", test))]
fn parse_psi_some_avg60(text: &str) -> Option<f64> {
    text.lines()
        .find_map(|line| line.strip_prefix("some "))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg60="))?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::PressureReading;
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};

    pub fn read() -> Option<PressureReading> {
        let level: i32 = sysctl(c"kern.memorystatus_vm_pressure_level")?;
        let swap: libc::xsw_usage = sysctl(c"vm.swapusage")?;
        let ram_bytes: u64 = sysctl(c"hw.memsize")?;
        Some(PressureReading::MacOs {
            level,
            swap_used_bytes: swap.xsu_used,
            ram_bytes,
        })
    }

    /// A fixed-size sysctl value, or `None` if the name is unknown or the
    /// kernel returned a different size.
    fn sysctl<T: Copy>(name: &CStr) -> Option<T> {
        let mut value = MaybeUninit::<T>::uninit();
        let mut len = size_of::<T>();
        // SAFETY: the buffer is `len` bytes, and the kernel writes at most that.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        // SAFETY: success with the full size means every byte was written.
        (rc == 0 && len == size_of::<T>()).then(|| unsafe { value.assume_init() })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::PressureReading;

    pub fn read() -> Option<PressureReading> {
        let text = std::fs::read_to_string("/proc/pressure/memory").ok()?;
        Some(PressureReading::Linux {
            some_avg60: super::parse_psi_some_avg60(&text)?,
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn read() -> Option<super::PressureReading> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn mac(level: i32, swap_gb: f64, ram_gb: u64) -> PressureReading {
        PressureReading::MacOs {
            level,
            swap_used_bytes: (swap_gb * GB as f64) as u64,
            ram_bytes: ram_gb * GB,
        }
    }

    #[test]
    fn a_swapping_16gb_mac_counts_as_elevated() {
        assert!(mac(2, 8.4, 16).is_elevated());
        assert!(mac(4, 8.4, 16).is_elevated());
    }

    #[test]
    fn critical_with_no_swap_does_not_count() {
        // The idle-host reading from ADR 0182.
        assert!(!mac(4, 0.0, 48).is_elevated());
    }

    #[test]
    fn heavy_swap_under_normal_pressure_does_not_count() {
        assert!(!mac(1, 12.0, 16).is_elevated());
    }

    #[test]
    fn the_swap_floor_scales_with_ram() {
        assert!(mac(2, 2.0, 16).is_elevated());
        assert!(!mac(2, 1.9, 16).is_elevated());
        assert!(!mac(2, 4.0, 48).is_elevated());
        assert!(mac(2, 4.8, 48).is_elevated());
    }

    #[test]
    fn a_mac_is_raised_at_warn_whatever_its_swap() {
        assert!(mac(2, 0.0, 48).is_raised());
        assert!(mac(4, 0.0, 48).is_raised());
        assert!(!mac(1, 12.0, 16).is_raised());
    }

    #[test]
    fn linux_is_raised_exactly_when_elevated() {
        for some_avg60 in [0.0, 9.99, 10.0, 40.0] {
            let reading = PressureReading::Linux { some_avg60 };
            assert_eq!(reading.is_raised(), reading.is_elevated());
        }
    }

    #[test]
    fn linux_psi_counts_at_ten_percent() {
        assert!(PressureReading::Linux { some_avg60: 10.0 }.is_elevated());
        assert!(!PressureReading::Linux { some_avg60: 9.99 }.is_elevated());
    }

    #[test]
    fn psi_parsing_reads_the_some_line() {
        let text = "some avg10=1.00 avg60=12.50 avg300=3.00 total=123\n\
                    full avg10=0.00 avg60=40.00 avg300=0.00 total=9\n";
        assert_eq!(parse_psi_some_avg60(text), Some(12.5));
        assert_eq!(parse_psi_some_avg60("full avg60=40.00\n"), None);
        assert_eq!(parse_psi_some_avg60(""), None);
    }

    /// Linux is left out: a container or an old kernel may lack PSI.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_is_readable() {
        assert!(read().is_some());
    }
}
