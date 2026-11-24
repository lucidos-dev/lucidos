use lucidos_core::log_legacy;

/// Prepare the host for normal engine startup after loading the database environment.
pub fn prepare() {
    // macOS defaults to 256, too low for concurrent coding-agent sessions, SSE
    // streams, the DB pool and static file serving.
    raise_fd_limit();

    log_legacy!(@Engine, "[Startup] Lucidos Engine starting...");

    // Log the supervisor chain for post-mortems. The log prefix includes our PID.
    #[cfg(unix)]
    {
        // SAFETY: getppid / getpgrp / getsid are async-signal-safe and take no
        // pointer arguments, so calling them is well-defined.
        let ppid = unsafe { libc::getppid() };
        let pgid = unsafe { libc::getpgrp() };
        let sid = unsafe { libc::getsid(0) };
        log_legacy!(@Engine, "[Startup] ppid={} pgid={} sid={}", ppid, pgid, sid);
    }

    // Prepend the common user-install bin dirs before anything spawns or probes
    // a tool. A packaged engine inherits its service manager's minimal PATH,
    // which ENOENTs bare-name tools that resolve fine in a dev shell.
    lucidos_core::core::user_path::augment_process_path();

    git_preflight();
    python_preflight();
}

fn raise_fd_limit() {
    use std::mem::MaybeUninit;
    unsafe {
        let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) == 0 {
            let mut rlim = rlim.assume_init();
            if rlim.rlim_cur < 8192 {
                rlim.rlim_cur = rlim.rlim_max.min(8192);
                libc::setrlimit(libc::RLIMIT_NOFILE, &rlim);
            }
        }
    }
}

/// One-time `git --version` boot preflight.
///
/// Coding agents, Apply and worktrees require Git. Chat can still start without it.
/// On macOS, launchd may resolve Git to the Command Line Tools shim even when
/// the tools are missing. Warn so a chat-only install can start without a respawn loop.
fn git_preflight() {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            log_legacy!(@Engine,
                "[Startup] git: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => log_legacy!(@Engine,
            "[Startup] WARNING: `git --version` exited {} — coding agents, Apply, and repo \
             operations will fail. On macOS install the Command Line Tools: `xcode-select --install`",
            out.status
        ),
        Err(e) => log_legacy!(@Engine,
            "[Startup] WARNING: `git` not found ({e}) — coding agents, Apply, and repo \
             operations will fail. On macOS install the Command Line Tools: `xcode-select --install`"
        ),
    }
}

/// One-time `python3 --version` boot preflight, [`git_preflight`]'s sibling.
///
/// `python3` backs the `run_python` tool and nothing else, so a missing
/// interpreter is a loud boot warning rather than a fatal exit. Same
/// Command-Line-Tools shim story as git.
fn python_preflight() {
    match std::process::Command::new("python3")
        .arg("--version")
        .output()
    {
        Ok(out) if out.status.success() => {
            log_legacy!(@Engine,
                "[Startup] python3: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => log_legacy!(@Engine,
            "[Startup] WARNING: `python3 --version` exited {} — the run_python tool will fail. \
             On macOS install the Command Line Tools: `xcode-select --install`",
            out.status
        ),
        Err(e) => log_legacy!(@Engine,
            "[Startup] WARNING: `python3` not found ({e}) — the run_python tool will fail. \
             On macOS install the Command Line Tools: `xcode-select --install`"
        ),
    }
}
