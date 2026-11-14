//! The Tokio runtime the engine runs on.

/// Worker-thread stack size for the Tokio runtime.
///
/// The engine polls deeply-nested async chains on a single worker thread. A
/// trigger fire descends the scheduler, the Thread Queue executor, the agentic
/// loop and a tool, all as one un-spawned future. Tokio's default 2 MiB worker
/// stack once overflowed on a deeper chain and aborted the engine.
///
/// 16 MiB is reserved virtual address space committed lazily per touched page,
/// so the headroom is effectively free. Shrinking it gains nothing.
pub const WORKER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Build the multi-threaded Tokio runtime the engine runs on. What
/// `#[tokio::main]` produces, plus a larger worker-thread stack (see
/// [`WORKER_THREAD_STACK_SIZE`]). Shared with the tests, so they exercise the
/// exact production config.
pub fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .build()
}

#[cfg(test)]
mod tests {
    use super::{build_runtime, WORKER_THREAD_STACK_SIZE};

    /// Recurse with a large live stack frame per level, emulating the engine's
    /// deep un-spawned async poll chain. `#[inline(never)]`, `black_box` and
    /// reading `buf` AFTER the recursive call together defeat tail-call and
    /// dead-store optimization, so every level genuinely consumes stack.
    #[inline(never)]
    fn consume_stack(depth: usize) -> u64 {
        let mut buf = [0u8; 64 * 1024]; // 64 KiB of stack per frame
        let idx = depth % buf.len();
        buf[idx] = depth as u8;
        std::hint::black_box(&buf);
        let deeper = if depth == 0 {
            0
        } else {
            consume_stack(depth - 1)
        };
        deeper.wrapping_add(buf[idx] as u64)
    }

    /// The engine's runtime must give its bounded-but-deep poll chain real
    /// headroom. See [`super::WORKER_THREAD_STACK_SIZE`].
    #[test]
    fn worker_stack_holds_deep_nested_async_chain() {
        // Must comfortably exceed tokio's 2 MiB default. Checked at compile
        // time, because the invariant is over a const.
        const {
            assert!(
                WORKER_THREAD_STACK_SIZE >= 8 * 1024 * 1024,
                "worker stack must exceed tokio's 2 MiB default with headroom"
            );
        }

        // A chain needing ~4 MiB of stack, on a worker thread of the
        // *production* runtime builder. On a 2 MiB stack this aborts the
        // process; on the configured stack it completes cleanly.
        let rt = build_runtime().expect("runtime builds");
        let sum = rt.block_on(async {
            tokio::spawn(async { consume_stack(64) })
                .await
                .expect("deep-stack task joins without overflowing the worker stack")
        });
        let expected: u64 = (0..=64).sum();
        assert_eq!(
            sum, expected,
            "deep recursion ran every level to completion"
        );
    }
}
