//! Open-file limit handling: the reason a single machine can't scale and the
//! number each native worker uses to advertise its capacity to the fleet.

/// File descriptors kept free for the runtime, sync connection, DNS, logs...
const RESERVED_FDS: u64 = 64;

/// Raise the soft `RLIMIT_NOFILE` as far as the OS allows and return the
/// resulting soft limit.
#[cfg(unix)]
pub fn raise_fd_limit() -> u64 {
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return 1024;
        }
        // The hard limit may say "unlimited" while the kernel enforces a per-process
        // ceiling (kern.maxfilesperproc on macOS, fs.nr_open on Linux).
        let hard = lim.rlim_max.min(os_max_files());
        for want in [hard, 1_048_576, 262_144, 65_536, 24_576, 10_240] {
            if want <= lim.rlim_cur || want > hard {
                continue;
            }
            let attempt = libc::rlimit { rlim_cur: want, rlim_max: hard };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &attempt) == 0 {
                lim.rlim_cur = want;
                break;
            }
        }
        (lim.rlim_cur as u64).min(os_max_files() as u64)
    }
}

#[cfg(target_os = "macos")]
fn os_max_files() -> libc::rlim_t {
    let mut v: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let name = c"kern.maxfilesperproc";
    let ok = unsafe {
        libc::sysctlbyname(name.as_ptr(), &mut v as *mut _ as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0)
    } == 0;
    if ok && v > 0 { v as libc::rlim_t } else { 10_240 }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn os_max_files() -> libc::rlim_t {
    std::fs::read_to_string("/proc/sys/fs/nr_open")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1_048_576)
}

#[cfg(not(unix))]
pub fn raise_fd_limit() -> u64 {
    // Windows sockets aren't bounded by a CRT file limit in the same way.
    16_384
}

/// Concurrent virtual users (one connection each) a worker can safely hold.
pub fn capacity_from_fd_limit(soft_limit: u64) -> u32 {
    soft_limit.saturating_sub(RESERVED_FDS).clamp(1, u32::MAX as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_reserves_headroom() {
        assert_eq!(capacity_from_fd_limit(256), 192);
        assert_eq!(capacity_from_fd_limit(10), 1);
    }

    #[test]
    fn raising_never_lowers() {
        let raised = raise_fd_limit();
        assert!(raised >= 256, "raised={raised}");
        #[cfg(unix)]
        assert!(raised <= os_max_files() as u64, "raised={raised} beyond kernel max");
    }
}
