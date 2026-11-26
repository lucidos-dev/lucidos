use tokio::sync::watch;

/// Holds coding-agent process starts until the API can serve their hooks.
#[derive(Clone)]
pub struct SessionSpawnGate(watch::Sender<bool>);

impl Default for SessionSpawnGate {
    fn default() -> Self {
        Self(watch::Sender::new(true))
    }
}

impl SessionSpawnGate {
    pub fn hold(&self) {
        self.0.send_replace(false);
    }

    pub fn open(&self) {
        self.0.send_replace(true);
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub async fn wait(&self) {
        let mut readiness = self.subscribe();
        if *readiness.borrow_and_update() {
            return;
        }
        log_legacy!("[AgentSession] Waiting for the API listener before spawning");
        let _ = readiness.wait_for(|open| *open).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_gate_is_open_and_held_waiters_wake_on_open() {
        let gate = SessionSpawnGate::default();
        let fresh = gate.wait();
        tokio::pin!(fresh);
        assert!(futures::poll!(fresh).is_ready());

        gate.hold();
        let first = gate.wait();
        let second = gate.wait();
        tokio::pin!(first, second);
        assert!(futures::poll!(&mut first).is_pending());
        assert!(futures::poll!(&mut second).is_pending());
        gate.open();
        assert!(futures::poll!(first).is_ready());
        assert!(futures::poll!(second).is_ready());
    }
}
