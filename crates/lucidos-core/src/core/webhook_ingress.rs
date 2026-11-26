//! INVARIANT: What an ingress probe found, and how it is judged.
//!
//! The vocabulary lives here rather than in the scheduler because the two
//! `WebhookIngress*` events carry it on the wire. A workspace trigger codes
//! against these exact strings, so `docs/adr/0143-webhook-ingress-probed-per-address-family.md`
//! pins them.
//!
//! Everything in this module is pure. The scheduler does the network work and
//! hands the results here to be judged.

use serde::{Deserialize, Serialize};

/// INVARIANT: Which address family an answer came back on.
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
    pub const BOTH: [Family; 2] = [Family::Ipv4, Family::Ipv6];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Healthy,
    IngressUnreachable,
    BackendUnreachable,
    RouteMissing,
    UnexpectedResponder,
    LocalStackUnavailable,
    LocalEgressBlocked,
}

impl Stage {
    pub fn measured_the_ingress(self) -> bool {
        !matches!(
            self,
            Stage::LocalStackUnavailable | Stage::LocalEgressBlocked
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Healthy,
    Degraded,
    NotProbed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressProbe {
    pub address: String,
    pub family: Family,
    pub stage: Stage,
    pub status: Option<u16>,
    pub detail: Option<String>,
}

/// INVARIANT: One family's reading. Both families always appear, so a reader can tell
/// "healthy" from "never asked".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyVerdict {
    pub family: Family,
    pub verdict: Verdict,
    pub healthy: usize,
    pub total: usize,
}

/// INVARIANT: Read a status code as a stage.
///
/// 401 is the healthy answer, because an unsigned probe is exactly what a hook
/// must turn away. A 2xx means something else is listening.
pub fn classify_status(status: u16) -> Stage {
    match status {
        401 => Stage::Healthy,
        502..=504 => Stage::BackendUnreachable,
        404 => Stage::RouteMissing,
        _ => Stage::UnexpectedResponder,
    }
}

pub fn nothing_answered(addresses: &[AddressProbe], family: Family) -> bool {
    let mut of_family = addresses.iter().filter(|a| a.family == family).peekable();
    of_family.peek().is_some() && of_family.all(|a| a.stage == Stage::IngressUnreachable)
}

/// INVARIANT: Judge each family from the per-address results.
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

fn verdict_of(families: &[FamilyVerdict], family: Family) -> Verdict {
    families
        .iter()
        .find(|f| f.family == family)
        .map_or(Verdict::NotProbed, |f| f.verdict)
}

/// INVARIANT: The families judged degraded, in canonical order.
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

pub const STRIKES_BEFORE_DEGRADED: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Declare,
    Recover,
}

/// INVARIANT: Decide from this cycle's reading, the previous one, and what was declared.
///
/// The caller reads `declared` from the events table each cycle rather than
/// holding it in memory. An engine restart therefore cannot emit a second
/// `WebhookIngressDegraded` for an outage the timeline already carries.
///
/// Returns the new strike count with the decision. A changed set of degraded
/// families restarts the count, so IPv6 joining IPv4 is debounced on its own
/// terms rather than inheriting IPv4's strikes.
///
/// Recovery follows [`recovers`].
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

    if recovers(families, declared) {
        return (Decision::Recover, strikes);
    }

    let already_said = declared == Some(observed.as_slice());
    if !observed.is_empty() && strikes >= STRIKES_BEFORE_DEGRADED && !already_said {
        (Decision::Declare, strikes)
    } else {
        (Decision::Nothing, strikes)
    }
}

pub fn recovers(families: &[FamilyVerdict], declared: Option<&[Family]>) -> bool {
    declared.is_some_and(|down| {
        down.iter()
            .all(|family| verdict_of(families, *family) == Verdict::Healthy)
    })
}

#[cfg(test)]
#[path = "webhook_ingress_tests.rs"]
mod tests;
