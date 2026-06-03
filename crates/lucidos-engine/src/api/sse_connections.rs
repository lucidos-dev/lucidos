//! Live count of open SSE connections to `GET /api/v1/events`.
//!
//! The PresenceCheck fan-out (`system-knowhow/notifications.md` §3) needs to
//! know whether any page is connected and therefore able to pong. The old gate
//! was `device_presence` heartbeat freshness, but iOS suspends the 30s
//! heartbeat timer while the PWA is foregrounded, so the row aged past the
//! 120s window even though the EventSource stayed open — the engine then
//! skipped the PresenceCheck and fired an OS push on top of an active page
//! ("push while the app was foreground for a while"). This counter is the
//! ground truth: it increments when a page opens the SSE stream and decrements
//! when the stream is dropped (client disconnects), so it can't go stale while
//! a page is connected.
//!
//! Transient by design — purely in-memory, reset to zero on engine restart
//! (every page re-opens its EventSource on load).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared, cloneable handle to the live SSE-connection count. Held on
/// `LucidosEngine`; the SSE handler clones it to register a connection and the
/// push fan-out reads it to gate the PresenceCheck.
#[derive(Default, Clone)]
pub struct SseConnectionCounter {
    count: Arc<AtomicUsize>,
}

impl SseConnectionCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new SSE connection. The returned guard decrements the count
    /// on drop — tie it to the SSE response stream's lifetime so the count
    /// tracks open connections exactly (the stream is dropped when the client
    /// disconnects).
    #[must_use]
    pub fn connect(&self) -> SseConnectionGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        SseConnectionGuard {
            count: self.count.clone(),
        }
    }

    /// Number of SSE connections currently open.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

/// RAII guard returned by [`SseConnectionCounter::connect`]. Decrements the
/// live count on drop — the SSE stream being dropped means the client
/// disconnected.
pub struct SseConnectionGuard {
    count: Arc<AtomicUsize>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_increments_and_drop_decrements() {
        let counter = SseConnectionCounter::new();
        assert_eq!(counter.count(), 0);
        let g1 = counter.connect();
        assert_eq!(counter.count(), 1);
        let g2 = counter.connect();
        assert_eq!(counter.count(), 2);
        drop(g1);
        assert_eq!(counter.count(), 1, "dropping one connection decrements by one");
        drop(g2);
        assert_eq!(counter.count(), 0, "dropping the last connection returns to zero");
    }

    #[test]
    fn clones_share_the_same_count() {
        // The SSE handler and the push fan-out hold separate clones of the
        // same counter; a connection registered through one must be visible
        // through the other.
        let counter = SseConnectionCounter::new();
        let fanout_view = counter.clone();
        let _g = counter.connect();
        assert_eq!(fanout_view.count(), 1);
    }
}
