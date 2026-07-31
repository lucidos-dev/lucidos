//! Capacity policy + admission decision for the *Thread Queue*.
//!
//! The decision logic is a pure function on [`CapacityPolicy`] so it can be
//! unit-tested without an engine, a database, or tokio.

use serde::{Deserialize, Serialize};

/// Capacity bucket a queued spawn counts against. Wire / DB strings are the
/// kebab-case names (`event-trigger`, `cron`, `sub-thread`, `coding-agent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadQueueKind {
    /// Trigger fired by a matching domain/thread event.
    EventTrigger,
    /// Trigger fired by its cron schedule (including missed-grace catch-up).
    Cron,
    /// Chat sub-thread spawned by an agent (`run_thread` tool, agent-mode
    /// chat POSTs without Claude Code).
    SubThread,
    /// Coding-agent thread spawned by an agent (`run_coding_agent` tool, agent-mode
    /// chat POSTs with Claude Code).
    CodingAgent,
}

impl ThreadQueueKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EventTrigger => "event-trigger",
            Self::Cron => "cron",
            Self::SubThread => "sub-thread",
            Self::CodingAgent => "coding-agent",
        }
    }
}

/// What happens when a trigger's queue hits `max_queued_per_trigger`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverflowPolicy {
    /// Drop the oldest queued entry of that trigger (and notify) to make room.
    DropOldest,
    /// Pause the trigger (and notify); queued entries wait for manual resume.
    PauseTrigger,
}

/// The *capacity policy*: configurable caps governing how many threads run
/// concurrently and how deep a single trigger's backlog may grow.
///
/// One shared pool gates BOTH background spawns and user-initiated work
/// (chat / user-typed coding-agent threads). User-initiated work is
/// *prioritized* — it jumps ahead of background in the drain and ignores the
/// per-kind / per-trigger caps — but it is *not exempt*: it counts against
/// `max_concurrent_total` and queues when the pool is genuinely full.
/// `reserved_background` keeps that priority from starving background.
///
/// Stored event-sourced: the latest persisted `CapacityPolicyChanged` event
/// IS the policy; absence means [`CapacityPolicy::default`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapacityPolicy {
    /// Max concurrently running threads across ALL kinds — background spawns
    /// AND user-initiated work. The hard ceiling everything queues behind.
    pub max_concurrent_total: usize,
    /// Per-kind concurrency caps (background kinds only — user-initiated work
    /// is not bucketed by kind).
    pub max_concurrent_event_trigger: usize,
    pub max_concurrent_cron: usize,
    pub max_concurrent_sub_thread: usize,
    pub max_concurrent_coding_agent: usize,
    /// Max concurrent runs of one trigger. 1 preserves strict per-trigger
    /// FIFO (a trigger's fires execute in arrival order).
    pub max_concurrent_per_trigger: usize,
    /// Hard ceiling on one trigger's queued backlog; hitting it applies
    /// `overflow`.
    pub max_queued_per_trigger: usize,
    /// Slots background spawns can always *reclaim* ahead of user-initiated
    /// work, so user priority can't starve triggers/cron. When a slot frees
    /// and background is below this floor with work waiting, background takes
    /// it before any user waiter; above the floor, user waiters win. Set to 0
    /// for pure user priority (background can be fully starved). Effective
    /// value is clamped to `max_concurrent_total`.
    pub reserved_background: usize,
    pub overflow: OverflowPolicy,
}

impl Default for CapacityPolicy {
    fn default() -> Self {
        Self {
            max_concurrent_total: 32,
            max_concurrent_event_trigger: 8,
            max_concurrent_cron: 8,
            max_concurrent_sub_thread: 16,
            max_concurrent_coding_agent: 24,
            max_concurrent_per_trigger: 1,
            max_queued_per_trigger: 25,
            reserved_background: 8,
            overflow: OverflowPolicy::DropOldest,
        }
    }
}

/// Live counts the admission decision is made against. Plain data so the
/// decision stays a pure function.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmissionCounts {
    /// Active background spawns, all kinds (the queue's admitted set).
    pub background_active: usize,
    /// Active user-initiated responses (chat / user-typed coding agents).
    /// They share the pool but are tracked separately so background
    /// admission can respect the reserved floor.
    pub user_active: usize,
    /// Active background spawns of the candidate's kind.
    pub kind_active: usize,
    /// Active background spawns of the candidate's trigger (0 for non-trigger).
    pub trigger_active: usize,
    /// Queued entries of the candidate's trigger (0 for non-trigger kinds).
    pub trigger_queued: usize,
    /// User-initiated waiters in line. A new background spawn yields a free
    /// slot to them — unless background is still below its reserved floor.
    pub user_queued: usize,
}

impl AdmissionCounts {
    /// Everything occupying the shared pool right now — background + user.
    pub fn total_active(&self) -> usize {
        self.background_active + self.user_active
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Capacity allows it — run now.
    Admit,
    /// Over capacity — enqueue and run when capacity frees.
    Queue,
    /// The trigger's queue is at its hard ceiling — apply [`OverflowPolicy`].
    Overflow,
    /// A cron fire of this trigger is already active or queued — the new fire
    /// is redundant (cron fires carry no distinct payload), so it is dropped
    /// instead of queued. Cron only; event triggers keep FIFO.
    Coalesce,
}

impl CapacityPolicy {
    pub fn cap_for_kind(&self, kind: ThreadQueueKind) -> usize {
        match kind {
            ThreadQueueKind::EventTrigger => self.max_concurrent_event_trigger,
            ThreadQueueKind::Cron => self.max_concurrent_cron,
            ThreadQueueKind::SubThread => self.max_concurrent_sub_thread,
            ThreadQueueKind::CodingAgent => self.max_concurrent_coding_agent,
        }
    }

    /// Background spawns the reserved floor protects for reclaim ahead of
    /// user-initiated work. Clamped to the global ceiling (a floor larger
    /// than the pool is meaningless).
    pub fn effective_reserved_background(&self) -> usize {
        self.reserved_background.min(self.max_concurrent_total)
    }

    /// User-initiated work admits whenever the shared pool has a free slot.
    /// It is prioritized, not bucketed — it ignores per-kind / per-trigger
    /// caps and the reserved floor (that floor only governs background
    /// *reclaim priority* on the drain, never blocks a user from a free
    /// slot). It still respects the hard ceiling, so a person queues when the
    /// pool is genuinely full.
    pub fn user_can_admit(&self, total_active: usize) -> bool {
        total_active < self.max_concurrent_total
    }

    /// Decide admission for a NEW background submission. `has_trigger` is true
    /// for trigger-bound entries (event-trigger / cron kinds).
    ///
    /// A submission queues behind any already-queued entries of the same
    /// trigger even when capacity is free — that preserves per-trigger FIFO
    /// across the submit/drain race. The global ceiling counts user-initiated
    /// work too (`counts.total_active()`), so background backs off under user
    /// load.
    pub fn decide_submit(
        &self,
        counts: AdmissionCounts,
        kind: ThreadQueueKind,
        has_trigger: bool,
    ) -> AdmissionDecision {
        // Cron coalescing: a cron fire carries no distinct payload (just
        // `trigger_id`), so a trigger never needs more than one pending fire.
        // If one is already active OR queued, the new fire is redundant — drop
        // it instead of stacking a duplicate (the restart-storm pile-up fix).
        // Cron only; event triggers carry a per-fire `event_payload` and keep
        // strict FIFO.
        if kind == ThreadQueueKind::Cron
            && (counts.trigger_active >= 1 || counts.trigger_queued >= 1)
        {
            return AdmissionDecision::Coalesce;
        }
        let must_queue = (has_trigger && counts.trigger_queued > 0)
            || counts.total_active() >= self.max_concurrent_total
            || counts.kind_active >= self.cap_for_kind(kind)
            || (has_trigger && counts.trigger_active >= self.max_concurrent_per_trigger)
            // Yield a free slot to waiting user-initiated work (it has drain
            // priority) — UNLESS background is still below its reserved floor,
            // where background reclaims ahead of users.
            || (counts.user_queued > 0
                && counts.background_active >= self.effective_reserved_background());
        if !must_queue {
            return AdmissionDecision::Admit;
        }
        if has_trigger && counts.trigger_queued >= self.max_queued_per_trigger {
            return AdmissionDecision::Overflow;
        }
        AdmissionDecision::Queue
    }

    /// Decide admission for the HEAD entry of its trigger's line during a
    /// drain pass. Identical to [`Self::decide_submit`] minus the
    /// queue-behind-your-own-trigger rule (this entry IS the front of that
    /// line) and minus overflow (draining never grows the queue). The global
    /// ceiling counts user-initiated work too.
    pub fn decide_drain(
        &self,
        counts: AdmissionCounts,
        kind: ThreadQueueKind,
        has_trigger: bool,
    ) -> AdmissionDecision {
        let must_wait = counts.total_active() >= self.max_concurrent_total
            || counts.kind_active >= self.cap_for_kind(kind)
            || (has_trigger && counts.trigger_active >= self.max_concurrent_per_trigger);
        if must_wait {
            AdmissionDecision::Queue
        } else {
            AdmissionDecision::Admit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> CapacityPolicy {
        CapacityPolicy::default()
    }

    #[test]
    fn admits_when_everything_is_free() {
        let d = policy().decide_submit(
            AdmissionCounts::default(),
            ThreadQueueKind::EventTrigger,
            true,
        );
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn queues_at_global_cap() {
        let counts = AdmissionCounts {
            background_active: policy().max_concurrent_total,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn user_load_counts_toward_the_global_cap_for_background() {
        // The whole point of the unified pool: user-initiated work occupies
        // the ceiling, so background queues even with zero background running.
        let counts = AdmissionCounts {
            background_active: 1,
            user_active: policy().max_concurrent_total - 1,
            ..Default::default()
        };
        assert_eq!(counts.total_active(), policy().max_concurrent_total);
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn user_admits_while_pool_has_a_free_slot_then_queues_at_ceiling() {
        let p = policy();
        // One free slot left (background+user = max-1) → a person still runs.
        assert!(p.user_can_admit(p.max_concurrent_total - 1));
        // Pool genuinely full → even a person queues.
        assert!(!p.user_can_admit(p.max_concurrent_total));
    }

    #[test]
    fn user_ignores_kind_and_trigger_caps() {
        // User-initiated work is prioritized, not bucketed: per-kind /
        // per-trigger saturation is irrelevant — only the hard ceiling gates
        // it. (user_can_admit takes only the total for exactly this reason.)
        let p = policy();
        assert!(p.user_can_admit(0));
        assert!(p.user_can_admit(p.max_concurrent_total - 1));
    }

    #[test]
    fn queues_at_kind_cap_even_when_global_is_free() {
        let counts = AdmissionCounts {
            background_active: 2,
            kind_active: policy().max_concurrent_coding_agent,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::CodingAgent, false);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn queues_at_per_trigger_cap() {
        // Default per-trigger concurrency is 1 — a second fire of the same
        // trigger queues even though every other cap has headroom.
        let counts = AdmissionCounts {
            background_active: 1,
            kind_active: 1,
            trigger_active: 1,
            trigger_queued: 0,
            user_active: 0,
            user_queued: 0,
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::EventTrigger, true);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn cron_first_fire_admits() {
        // No active, none queued for this cron trigger — the first fire runs.
        let d = policy().decide_submit(AdmissionCounts::default(), ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn cron_coalesces_when_one_active() {
        // A fire of this cron trigger is already running — a new one is
        // redundant (cron fires carry no distinct payload) and coalesces.
        let counts = AdmissionCounts {
            background_active: 1,
            kind_active: 1,
            trigger_active: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Coalesce);
    }

    #[test]
    fn cron_coalesces_when_one_queued() {
        // One already waiting for this cron trigger — the new fire collapses
        // into it rather than deepening the backlog.
        let counts = AdmissionCounts {
            trigger_queued: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Coalesce);
    }

    #[test]
    fn event_trigger_does_not_coalesce() {
        // Event-trigger fires carry a distinct `event_payload` each — they keep
        // strict FIFO and must NEVER coalesce, even with one active and queued.
        let counts = AdmissionCounts {
            background_active: 1,
            kind_active: 1,
            trigger_active: 1,
            trigger_queued: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::EventTrigger, true);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn per_trigger_cap_ignored_for_non_trigger_kinds() {
        let counts = AdmissionCounts {
            background_active: 1,
            kind_active: 1,
            trigger_active: 1, // nonsensical for has_trigger=false; must be ignored
            trigger_queued: 1,
            user_active: 0,
            user_queued: 0,
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn background_yields_free_slot_to_waiting_user_above_floor() {
        // Background is at its reserved floor and a user is waiting: a new
        // background spawn must queue so the freeing slot goes to the user.
        let counts = AdmissionCounts {
            background_active: policy().effective_reserved_background(),
            user_active: 0,
            user_queued: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn background_reclaims_below_floor_even_with_waiting_user() {
        // Below the reserved floor, background reclaims ahead of a waiting
        // user — the floor is background's starvation guard.
        let counts = AdmissionCounts {
            background_active: policy().effective_reserved_background() - 1,
            user_active: 0,
            user_queued: 5,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn reserved_background_defaults_and_clamps_to_ceiling() {
        assert_eq!(policy().effective_reserved_background(), 8);
        let p = CapacityPolicy {
            max_concurrent_total: 4,
            reserved_background: 99,
            ..CapacityPolicy::default()
        };
        // A floor larger than the pool is meaningless — clamp it.
        assert_eq!(p.effective_reserved_background(), 4);
    }

    #[test]
    fn queues_behind_own_triggers_backlog_even_with_free_capacity() {
        // FIFO per trigger: while older fires of this trigger wait in the
        // queue, a new fire must line up behind them — admitting it would
        // reorder the trigger's fires.
        let counts = AdmissionCounts {
            trigger_queued: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::EventTrigger, true);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn overflows_at_per_trigger_queue_ceiling() {
        let counts = AdmissionCounts {
            trigger_active: 1,
            trigger_queued: policy().max_queued_per_trigger,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::EventTrigger, true);
        assert_eq!(d, AdmissionDecision::Overflow);
    }

    #[test]
    fn drain_head_entry_not_blocked_by_its_own_backlog() {
        // During drain the candidate IS the front of its trigger's line —
        // the queue-behind rule must not apply or the queue would deadlock.
        let counts = AdmissionCounts {
            trigger_queued: 5,
            ..Default::default()
        };
        let d = policy().decide_drain(counts, ThreadQueueKind::EventTrigger, true);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn drain_respects_concurrency_caps() {
        let counts = AdmissionCounts {
            trigger_active: policy().max_concurrent_per_trigger,
            ..Default::default()
        };
        let d = policy().decide_drain(counts, ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Queue);
    }

    #[test]
    fn policy_serde_roundtrip_and_partial_deserialization() {
        // `#[serde(default)]` lets a PUT with a subset of fields (or an old
        // persisted CapacityPolicyChanged payload after a field is added)
        // deserialize with defaults for the rest.
        let p: CapacityPolicy =
            serde_json::from_str(r#"{ "max_concurrent_total": 2, "overflow": "pause-trigger" }"#)
                .unwrap();
        assert_eq!(p.max_concurrent_total, 2);
        assert_eq!(p.overflow, OverflowPolicy::PauseTrigger);
        assert_eq!(
            p.max_concurrent_per_trigger,
            CapacityPolicy::default().max_concurrent_per_trigger
        );
        // A pre-reserved_background persisted payload deserializes with the
        // default floor (serde(default)), so the field is back-compatible.
        assert_eq!(
            p.reserved_background,
            CapacityPolicy::default().reserved_background
        );
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["overflow"], "pause-trigger");
        let back: CapacityPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn kind_wire_names_are_kebab_case() {
        assert_eq!(
            serde_json::to_value(ThreadQueueKind::EventTrigger).unwrap(),
            "event-trigger"
        );
        assert_eq!(
            serde_json::to_value(ThreadQueueKind::CodingAgent).unwrap(),
            "coding-agent"
        );
        assert_eq!(ThreadQueueKind::SubThread.as_str(), "sub-thread");
        assert_eq!(ThreadQueueKind::Cron.as_str(), "cron");
    }
}
