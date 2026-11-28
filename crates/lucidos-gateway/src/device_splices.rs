//! The live socket splices of each paired device, so revoking a device closes
//! them (ADR 0356 decision 8).
//!
//! INVARIANT: No splice outlives its device's revocation. A splice registers
//! first and then re-checks pairing. A revoke removes the device first and
//! then aborts. Whichever runs second sees the other.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::task::AbortHandle;

struct LiveSplice {
    abort: AbortHandle,
    remote_pairing: Option<(String, String)>,
}

#[derive(Default)]
pub struct DeviceSplices {
    live: Mutex<HashMap<String, HashMap<u64, LiveSplice>>>,
    next_id: AtomicU64,
}

/// Removes its splice from the map when dropped.
pub struct SpliceRegistration {
    splices: Arc<DeviceSplices>,
    device_id: String,
    id: u64,
}

impl Drop for SpliceRegistration {
    fn drop(&mut self) {
        if let Ok(mut live) = self.splices.live.lock() {
            if let Some(handles) = live.get_mut(&self.device_id) {
                handles.remove(&self.id);
                if handles.is_empty() {
                    live.remove(&self.device_id);
                }
            }
        }
    }
}

impl DeviceSplices {
    /// Track a splice for `device_id`, then abort it at once if the device is
    /// no longer paired.
    pub fn register(
        self: &Arc<Self>,
        device_id: &str,
        remote_pairing: Option<(&str, &str)>,
        splice: AbortHandle,
        still_paired: impl FnOnce(&str) -> bool,
    ) -> SpliceRegistration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut live) = self.live.lock() {
            live.entry(device_id.to_owned()).or_default().insert(
                id,
                LiveSplice {
                    abort: splice.clone(),
                    remote_pairing: remote_pairing
                        .map(|(name, id)| (name.to_owned(), id.to_owned())),
                },
            );
        }
        if !still_paired(device_id) {
            splice.abort();
        }
        SpliceRegistration {
            splices: self.clone(),
            device_id: device_id.to_owned(),
            id,
        }
    }

    /// Abort every splice of a device that was just revoked.
    pub fn abort_device(&self, device_id: &str) -> usize {
        let handles = self
            .live
            .lock()
            .ok()
            .and_then(|mut live| live.remove(device_id))
            .unwrap_or_default();
        for handle in handles.values() {
            handle.abort.abort();
        }
        handles.len()
    }

    /// Abort the relayed devices of an ended gateway pairing.
    pub fn abort_pairing(&self, pairing_id: &str) {
        if let Ok(mut live) = self.live.lock() {
            let prefix = crate::relay::namespaced_device_id(pairing_id, "");
            live.retain(|device, handles| {
                if !device.starts_with(&prefix) {
                    return true;
                }
                for handle in handles.values() {
                    handle.abort.abort();
                }
                false
            });
        }
    }

    /// Close the outgoing sockets before attempting to unpair at the peer.
    pub fn abort_remote(&self, name: &str, pairing_id: &str) {
        if let Ok(mut live) = self.live.lock() {
            live.retain(|_, handles| {
                handles.retain(|_, splice| {
                    let removed = splice
                        .remote_pairing
                        .as_ref()
                        .is_some_and(|(remote, id)| remote == name && id == pairing_id);
                    if removed {
                        splice.abort.abort();
                    }
                    !removed
                });
                !handles.is_empty()
            });
        }
    }

    #[cfg(test)]
    pub fn count(&self, device_id: &str) -> usize {
        self.live
            .lock()
            .map(|live| live.get(device_id).map_or(0, HashMap::len))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pending_task() -> tokio::task::JoinHandle<()> {
        tokio::spawn(std::future::pending())
    }

    #[tokio::test]
    async fn revoking_a_device_aborts_only_its_splices() {
        let splices = Arc::new(DeviceSplices::default());
        let phone = pending_task();
        let laptop = pending_task();
        let _phone = splices.register(
            "phone",
            Some(("cloud", "old-pairing")),
            phone.abort_handle(),
            |_| true,
        );
        let _laptop = splices.register("laptop", None, laptop.abort_handle(), |_| true);
        assert_eq!(splices.abort_device("phone"), 1);
        let ended = tokio::time::timeout(Duration::from_secs(1), phone).await;
        assert!(ended.expect("the splice ends").unwrap_err().is_cancelled());
        assert!(
            !laptop.is_finished(),
            "another device's splice keeps running"
        );
        assert_eq!(splices.count("laptop"), 1);
    }

    #[tokio::test]
    async fn retiring_a_remote_pairing_aborts_only_its_splices() {
        let splices = Arc::new(DeviceSplices::default());
        let forgotten = pending_task();
        let other = pending_task();
        let direct = pending_task();
        let _forgotten = splices.register(
            "phone",
            Some(("cloud", "old-pairing")),
            forgotten.abort_handle(),
            |_| true,
        );
        let _other = splices.register(
            "phone",
            Some(("cloud", "new-pairing")),
            other.abort_handle(),
            |_| true,
        );
        let _direct = splices.register("phone", None, direct.abort_handle(), |_| true);

        splices.abort_remote("cloud", "old-pairing");

        let ended = tokio::time::timeout(Duration::from_secs(1), forgotten).await;
        assert!(ended.expect("the splice ends").unwrap_err().is_cancelled());
        assert!(!other.is_finished());
        assert!(!direct.is_finished());
        assert_eq!(splices.abort_device("phone"), 2);
    }

    #[tokio::test]
    async fn a_splice_registered_after_its_revoke_is_aborted_at_once() {
        let splices = Arc::new(DeviceSplices::default());
        let late = pending_task();
        let _late = splices.register(
            "phone",
            Some(("cloud", "old-pairing")),
            late.abort_handle(),
            |_| false,
        );
        let ended = tokio::time::timeout(Duration::from_secs(1), late).await;
        assert!(ended.expect("the splice ends").unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn an_ended_splice_leaves_the_map() {
        let splices = Arc::new(DeviceSplices::default());
        let task = pending_task();
        let registration = splices.register("phone", None, task.abort_handle(), |_| true);
        assert_eq!(splices.count("phone"), 1);
        drop(registration);
        assert_eq!(splices.count("phone"), 0);
        assert_eq!(splices.abort_device("phone"), 0);
        task.abort();
    }
}
