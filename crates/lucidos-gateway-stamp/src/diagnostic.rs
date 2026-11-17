//! Identity and integrity of one diagnostic delivery, independent of chat calls.

use crate::Claim;

pub const HEADER_DIAGNOSTIC_ORIGIN: &str = "x-lucidos-diagnostic-origin";
pub const HEADER_DIAGNOSTIC_DELIVERY: &str = "x-lucidos-diagnostic-delivery";
pub const HEADER_DIAGNOSTIC_DIGEST: &str = "x-lucidos-diagnostic-digest";

/// Validated header-safe identity. Construction alone authenticates nothing;
/// each hop must verify its signature before using the origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticDelivery {
    origin: String,
    delivery_key: String,
    body_digest: String,
}

impl DiagnosticDelivery {
    pub fn new(origin: &str, delivery_key: &str, body_digest: &str) -> Option<Self> {
        let identity =
            |s: &str| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic());
        (identity(origin)
            && identity(delivery_key)
            && body_digest.len() == 64
            && body_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self {
            origin: origin.into(),
            delivery_key: delivery_key.into(),
            body_digest: body_digest.into(),
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn delivery_key(&self) -> &str {
        &self.delivery_key
    }
    pub fn body_digest(&self) -> &str {
        &self.body_digest
    }

    pub fn claim<'a>(&'a self, target_workspace: &'a str) -> Claim<'a> {
        Claim::DiagnosticDelivery {
            target_workspace,
            origin: &self.origin,
            delivery_key: &self.delivery_key,
            body_digest: &self.body_digest,
        }
    }

    pub fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            (HEADER_DIAGNOSTIC_ORIGIN, self.origin.clone()),
            (HEADER_DIAGNOSTIC_DELIVERY, self.delivery_key.clone()),
            (HEADER_DIAGNOSTIC_DIGEST, self.body_digest.clone()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Claim, StampKey};

    #[test]
    fn diagnostic_stamps_bind_purpose_target_origin_delivery_and_body() {
        let key = StampKey::generate().unwrap();
        let delivery = DiagnosticDelivery::new("source", "once", &"a".repeat(64)).unwrap();
        let stamp = key.sign(delivery.claim("target"), 1000);
        assert!(key.verify(&stamp, delivery.claim("target"), 1000));
        assert!(!key.verify(&stamp, delivery.claim("other-target"), 1000));
        for other in [
            DiagnosticDelivery::new("other-source", "once", &delivery.body_digest).unwrap(),
            DiagnosticDelivery::new("source", "twice", &delivery.body_digest).unwrap(),
            DiagnosticDelivery::new("source", "once", &"b".repeat(64)).unwrap(),
        ] {
            assert!(!key.verify(&stamp, other.claim("target"), 1000));
        }
        let chat = Claim::WorkspaceCall {
            target_workspace: "target",
            caller_workspace: "source",
            caller_thread_id: "once",
            verdict: &delivery.body_digest,
        };
        assert!(!key.verify(&key.sign(chat, 1000), delivery.claim("target"), 1000));
        assert!(!key.verify(&stamp, chat, 1000));
    }

    #[test]
    fn malformed_diagnostic_identity_is_refused() {
        for (origin, delivery, digest) in [
            ("", "once", "a".repeat(64)),
            ("source\nforged", "once", "a".repeat(64)),
            ("source", "", "a".repeat(64)),
            ("source", "once\0twice", "a".repeat(64)),
            ("source", "once", "A".repeat(64)),
            ("source", "once", "a".repeat(63)),
        ] {
            assert!(DiagnosticDelivery::new(origin, delivery, &digest).is_none());
        }
    }
}
