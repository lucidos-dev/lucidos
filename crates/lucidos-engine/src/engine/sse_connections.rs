use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct SseConnectionCounter {
    count: Arc<AtomicUsize>,
}

impl SseConnectionCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// INVARIANT: Register a new SSE connection. The returned guard decrements the count
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

    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

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
        assert_eq!(
            counter.count(),
            1,
            "dropping one connection decrements by one"
        );
        drop(g2);
        assert_eq!(
            counter.count(),
            0,
            "dropping the last connection returns to zero"
        );
    }

    #[test]
    fn clones_share_the_same_count() {
        let counter = SseConnectionCounter::new();
        let fanout_view = counter.clone();
        let _g = counter.connect();
        assert_eq!(fanout_view.count(), 1);
    }
}
