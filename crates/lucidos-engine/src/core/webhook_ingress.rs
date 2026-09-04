//! What an ingress probe found, and how it is judged.
//!
//! The vocabulary lives here rather than in the scheduler because the two
//! `WebhookIngress*` events carry it on the wire. A workspace trigger codes
//! against these exact strings, so `docs/adr/0143-webhook-ingress-probed-per-address-family.md`
//! pins them.
//!
//! Everything in this module is pure. The scheduler does the network work and
//! hands the results here to be judged.

use serde::{Deserialize, Serialize};

/// Which address family an answer came back on.
///
/// The whole feature turns on this distinction. A dual-stack client prefers
/// IPv6, so one probe of a hostname passes while every IPv4 relay is dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Ipv4,
    Ipv6,
}

impl Family {
    /// Both families, in the order every payload lists them.
    pub const BOTH: [Family; 2] = [Family::Ipv4, Family::Ipv6];
}

/// How far one request got. The stage IS the diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// The full chain answered: TLS terminated, a relay forwarded, the gateway
    /// routed the slug, the engine found the hook, and the verifier refused.
    Healthy,
    /// Nothing answered. This is the outage the feature exists to catch.
    IngressUnreachable,
    /// The ingress is up and what sits behind it is not.
    BackendUnreachable,
    /// Something answered, but the slug or the hook id is wrong.
    RouteMissing,
    /// Something that is not Lucidos is on the other end.
    UnexpectedResponder,
    /// This host has no route for the family, so nothing was sent. Not a
    /// verdict about the ingress, and never reported as degraded.
    LocalStackUnavailable,
    /// This host has a route, and still cannot open the funnel port to
    /// anything. A filtered network, not a dead ingress, and never degraded.
    LocalEgressBlocked,
}

impl Stage {
    /// Did this reading measure the ingress at all?
    ///
    /// Two stages answer no, and both name a fault on this side of the wire.
    /// A request that never left the machine says nothing about what it was
    /// aimed at. Every other stage rests on an answer that came back.
    pub fn measured_the_ingress(self) -> bool {
        !matches!(
            self,
            Stage::LocalStackUnavailable | Stage::LocalEgressBlocked
        )
    }
}

/// What one family's addresses add up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Healthy,
    Degraded,
    NotProbed,
}

/// One address, probed on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressProbe {
    pub address: String,
    pub family: Family,
    pub stage: Stage,
    /// The HTTP status, or `null` when nothing answered.
    pub status: Option<u16>,
    /// A short human line, present only on a failure.
    pub detail: Option<String>,
}

/// One family's reading. Both families always appear, so a reader can tell
/// "healthy" from "never asked".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyVerdict {
    pub family: Family,
    pub verdict: Verdict,
    /// Addresses of this family that answered 401.
    pub healthy: usize,
    /// Addresses of this family that were actually probed.
    pub total: usize,
}

/// Read a status code as a stage.
///
/// 401 is the healthy answer, because an unsigned probe is exactly what a hook
/// must turn away. A 2xx means something else is listening.
pub fn classify_status(status: u16) -> Stage {
    match status {
        401 => Stage::Healthy,
        // The funnel answers 502 when its loopback target is gone. A 503 or 504
        // comes from the hook socket's own limits. Both say the ingress carried
        // the request and the far side could not serve it.
        502..=504 => Stage::BackendUnreachable,
        404 => Stage::RouteMissing,
        _ => Stage::UnexpectedResponder,
    }
}

/// Did every address of this family fail before anything answered?
///
/// The question a local fault has to pass before it can be blamed. One answer
/// of any kind proves this host reaches the port. The failures beside it then
/// belong to the ingress, not to the network under this machine.
pub fn nothing_answered(addresses: &[AddressProbe], family: Family) -> bool {
    let mut of_family = addresses.iter().filter(|a| a.family == family).peekable();
    of_family.peek().is_some() && of_family.all(|a| a.stage == Stage::IngressUnreachable)
}

/// Judge each family from the per-address results.
///
/// A family with no probed address is `not-probed`, never degraded. That covers
/// a host with no IPv6 egress, and a host whose network filters the funnel
/// port. Either would otherwise report a permanent outage of a live ingress.
///
/// This is the ONLY producer of a family verdict, deliberately. Every verdict
/// therefore rests on at least one attempted request, so `degraded` can never
/// be pronounced over zero measurements.
pub fn judge(addresses: &[AddressProbe]) -> Vec<FamilyVerdict> {
    Family::BOTH
        .iter()
        .map(|family| {
            let probed: Vec<&AddressProbe> = addresses
                .iter()
                .filter(|a| a.family == *family && a.stage.measured_the_ingress())
                .collect();
            let healthy = probed.iter().filter(|a| a.stage == Stage::Healthy).count();
            let verdict = if probed.is_empty() {
                Verdict::NotProbed
            } else if healthy == 0 {
                Verdict::Degraded
            } else {
                Verdict::Healthy
            };
            FamilyVerdict {
                family: *family,
                verdict,
                healthy,
                total: probed.len(),
            }
        })
        .collect()
}

/// What one family read this cycle.
fn verdict_of(families: &[FamilyVerdict], family: Family) -> Verdict {
    families
        .iter()
        .find(|f| f.family == family)
        .map_or(Verdict::NotProbed, |f| f.verdict)
}

/// The families judged degraded, in canonical order.
///
/// Canonical because the debounce compares two of these for equality, and the
/// declared set is read back out of a stored event payload.
pub fn degraded_families(families: &[FamilyVerdict]) -> Vec<Family> {
    let mut out: Vec<Family> = families
        .iter()
        .filter(|f| f.verdict == Verdict::Degraded)
        .map(|f| f.family)
        .collect();
    out.sort();
    out
}

/// How many consecutive cycles must see the same failure before it is declared.
///
/// Two, so one lost packet is not an outage. Recovery takes a single success,
/// because being wrong in that direction only costs a re-declaration.
pub const STRIKES_BEFORE_DEGRADED: u32 = 2;

/// What the cycle should emit, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Declare,
    Recover,
}

/// Decide from this cycle's reading, the previous one, and what was declared.
///
/// The caller reads `declared` from the events table each cycle rather than
/// holding it in memory. An engine restart therefore cannot emit a second
/// `WebhookIngressDegraded` for an outage the timeline already carries.
///
/// Returns the new strike count with the decision. A changed set of degraded
/// families restarts the count, so IPv6 joining IPv4 is debounced on its own
/// terms rather than inheriting IPv4's strikes.
///
/// Recovery needs positive evidence: every family the declaration named must
/// have answered this cycle. "Nothing is degraded" is not enough, because a
/// family nobody could probe reports exactly that.
pub fn decide(
    families: &[FamilyVerdict],
    seen_last_cycle: &[Family],
    strikes: u32,
    declared: Option<&[Family]>,
) -> (Decision, u32) {
    let observed = degraded_families(families);
    let strikes = if observed == seen_last_cycle {
        strikes.saturating_add(1)
    } else {
        1
    };

    if let Some(down) = declared {
        let all_back = down
            .iter()
            .all(|family| verdict_of(families, *family) == Verdict::Healthy);
        if all_back {
            return (Decision::Recover, strikes);
        }
    }

    let already_said = declared == Some(observed.as_slice());
    if !observed.is_empty() && strikes >= STRIKES_BEFORE_DEGRADED && !already_said {
        (Decision::Declare, strikes)
    } else {
        (Decision::Nothing, strikes)
    }
}

#[cfg(test)]
#[path = "webhook_ingress_tests.rs"]
mod tests;
