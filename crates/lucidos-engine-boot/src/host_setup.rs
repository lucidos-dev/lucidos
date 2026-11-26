use lucidos_core::log_legacy;

pub fn prepare() {
    raise_fd_limit();

    log_legacy!(@Engine, "[Startup] Lucidos Engine starting...");

    #[cfg(unix)]
    {
        // SAFETY: getppid / getpgrp / getsid are async-signal-safe and take no
        // pointer arguments, so calling them is well-defined.
        let ppid = unsafe { libc::getppid() };
        let pgid = unsafe { libc::getpgrp() };
        let sid = unsafe { libc::getsid(0) };
        log_legacy!(@Engine, "[Startup] ppid={} pgid={} sid={}", ppid, pgid, sid);
    }

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
