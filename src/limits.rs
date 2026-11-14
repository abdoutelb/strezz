//! Open-file limit handling: the reason a single machine can't scale, and the
//! upper bound on the capacity each native worker advertises to the fleet.

/// File descriptors kept free for the runtime, sync connection, DNS, logs...
const RESERVED_FDS: u64 = 64;

/// Users a native worker offers per CPU core when `--max-users` is not given.
pub const DEFAULT_USERS_PER_CORE: u32 = 1_000;

/// Ceiling on that default. Every user holds a connection to the same target,
/// and one machine can only open as many of those as it has ephemeral ports:
/// 16,384 on macOS and Windows, about 28,000 on Linux.
pub const DEFAULT_MAX_USERS: u32 = 16_000;

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
            let attempt = libc::rlimit {
                rlim_cur: want,
                rlim_max: hard,
            };
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
        libc::sysctlbyname(
            name.as_ptr(),
            &mut v as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if ok && v > 0 {
        v as libc::rlim_t
    } else {
        10_240
    }
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

/// The most concurrent virtual users (one connection each) the open-file
/// limit allows.
pub fn capacity_from_fd_limit(soft_limit: u64) -> u32 {
    soft_limit
        .saturating_sub(RESERVED_FDS)
        .clamp(1, u32::MAX as u64) as u32
}

/// Capacity a native worker advertises.
///
/// The open-file limit is only an upper bound: every user also costs memory
/// (its own HTTP client), CPU and an ephemeral port, and the limit can be in
/// the millions inside a container. So without an explicit `max_users` the
/// worker offers a default sized from the CPU count instead of everything the
/// limit would allow. `max_users` replaces that default and may go above it,
/// up to the open-file limit.
pub fn native_capacity(fd_limit: u64, cores: usize, max_users: Option<u32>) -> u32 {
    let by_fds = capacity_from_fd_limit(fd_limit);
    let wanted = max_users.unwrap_or_else(|| {
        u32::try_from(cores)
            .unwrap_or(u32::MAX)
            .saturating_mul(DEFAULT_USERS_PER_CORE)
            .min(DEFAULT_MAX_USERS)
    });
    by_fds.min(wanted.max(1))
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
    fn default_capacity_follows_cores_not_the_fd_limit() {
        // A laptop: 61,440 descriptors would allow 61,376 users.
        assert_eq!(native_capacity(61_440, 11, None), 11_000);
        // A container with nofile = 1,048,576 on a 4-core host.
        assert_eq!(native_capacity(1_048_576, 4, None), 4_000);
        // Many cores still stay within the ephemeral-port range.
        assert_eq!(native_capacity(1_048_576, 64, None), DEFAULT_MAX_USERS);
        // A low open-file limit wins over the default.
        assert_eq!(native_capacity(256, 8, None), 192);
        assert_eq!(native_capacity(61_440, 0, None), 1);
    }

    #[test]
    fn max_users_overrides_the_default_up_to_the_fd_limit() {
        assert_eq!(native_capacity(61_440, 11, Some(500)), 500);
        assert_eq!(native_capacity(61_440, 11, Some(30_000)), 30_000);
        assert_eq!(native_capacity(61_440, 11, Some(1_000_000)), 61_376);
        assert_eq!(native_capacity(61_440, 11, Some(0)), 1);
    }

    #[test]
    fn raising_never_lowers() {
        let raised = raise_fd_limit();
        assert!(raised >= 256, "raised={raised}");
        #[cfg(unix)]
        assert!(
            raised <= os_max_files() as u64,
            "raised={raised} beyond kernel max"
        );
    }
}
