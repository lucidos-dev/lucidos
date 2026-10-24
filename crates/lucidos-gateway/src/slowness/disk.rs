//! Free space on the volumes Lucidos writes to, read so that it can never hang
//! the slowness sampler (ADR 0301).
//!
//! A statfs on a local disk returns at once, full or not. A hung network mount
//! can block it for good. So each read runs on its own thread under a ceiling,
//! and only one read is ever in flight.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Below this much free space, the disk is the reason Lucidos is slow. Far
/// under the engine's cleanup floor, because this claims the disk is the
/// cause, not that it needs tidying.
const CRITICAL_FREE_BYTES: u64 = 2_000_000_000;

/// How long a sample waits for a reading.
const READ_CEILING: Duration = Duration::from_secs(1);

pub fn is_critical(free_bytes: u64) -> bool {
    free_bytes < CRITICAL_FREE_BYTES
}

#[derive(Default)]
pub struct DiskReader {
    in_flight: Arc<AtomicBool>,
}

impl DiskReader {
    /// The least free space among `volumes`. `None` when none could be read in
    /// time, or an earlier read has not returned yet.
    pub fn lowest_free_bytes(&self, volumes: &[PathBuf]) -> Option<u64> {
        self.read_bounded(volumes.to_vec(), READ_CEILING, |volumes| {
            volumes
                .iter()
                .filter_map(|v| fs2::available_space(v).ok())
                .min()
        })
    }

    /// Run `read` on a thread of its own and wait at most `ceiling`. A read that
    /// outlives the ceiling keeps the guard until it returns, so a hung volume
    /// holds one thread, never one per sample.
    fn read_bounded<F>(&self, volumes: Vec<PathBuf>, ceiling: Duration, read: F) -> Option<u64>
    where
        F: FnOnce(&[PathBuf]) -> Option<u64> + Send + 'static,
    {
        if self.in_flight.swap(true, Ordering::AcqRel) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let in_flight = self.in_flight.clone();
        let spawned = std::thread::Builder::new()
            .name("slowness-disk".into())
            .spawn(move || {
                let free = read(&volumes);
                in_flight.store(false, Ordering::Release);
                // The sample may have stopped waiting. Nothing is lost then.
                let _ = tx.send(free);
            });
        if spawned.is_err() {
            self.in_flight.store(false, Ordering::Release);
            return None;
        }
        rx.recv_timeout(ceiling).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    #[test]
    fn critical_means_under_two_gigabytes() {
        assert!(is_critical(0));
        assert!(is_critical(1_999_999_999));
        assert!(!is_critical(2_000_000_000));
    }

    #[test]
    fn a_readable_volume_reports_its_free_space() {
        let dir = tempfile::tempdir().unwrap();
        let free = DiskReader::default().lowest_free_bytes(&[dir.path().to_path_buf()]);
        assert!(free.is_some());
    }

    #[test]
    fn a_missing_volume_reports_nothing() {
        let free = DiskReader::default().lowest_free_bytes(&["/no/such/volume".into()]);
        assert_eq!(free, None);
    }

    #[test]
    fn the_lowest_volume_wins() {
        let reader = DiskReader::default();
        let free = reader.read_bounded(vec![], READ_CEILING, |_| [7, 3, 9].into_iter().min());
        assert_eq!(free, Some(3));
    }

    #[test]
    fn a_hung_read_gives_up_in_time_and_never_piles_up() {
        let reader = DiskReader::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let hung = |calls: Arc<AtomicUsize>| {
            move |_: &[PathBuf]| {
                calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(400));
                Some(1)
            }
        };

        let started = Instant::now();
        let first = reader.read_bounded(vec![], Duration::from_millis(50), hung(calls.clone()));
        assert_eq!(first, None);
        assert!(started.elapsed() < Duration::from_millis(300));

        // The first read still holds the guard, so this one never starts.
        let second = reader.read_bounded(vec![], Duration::from_millis(50), hung(calls.clone()));
        assert_eq!(second, None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Once it returns, reads start again.
        std::thread::sleep(Duration::from_millis(500));
        let third = reader.read_bounded(vec![], READ_CEILING, |_| Some(5));
        assert_eq!(third, Some(5));
    }
}
