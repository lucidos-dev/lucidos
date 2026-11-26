use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadQueueKind {
    EventTrigger,
    Cron,
    SubThread,
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

    pub fn is_trigger_fire(&self) -> bool {
        matches!(self, Self::EventTrigger | Self::Cron)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverflowPolicy {
    DropOldest,
    PauseTrigger,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapacityPolicy {
    pub max_concurrent_total: usize,
    pub max_concurrent_event_trigger: usize,
    pub max_concurrent_cron: usize,
    pub max_concurrent_sub_thread: usize,
    pub max_concurrent_coding_agent: usize,
    pub max_concurrent_per_trigger: usize,
    pub max_queued_per_trigger: usize,
    pub reserved_background: usize,
    pub overflow: OverflowPolicy,
    pub max_event_trigger_depth: u32,
    pub max_concurrent_children_per_thread: usize,
}

pub const DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD: usize = 10;

pub const DEFAULT_MAX_EVENT_TRIGGER_DEPTH: u32 = 5;

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
            max_event_trigger_depth: DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
            max_concurrent_children_per_thread: DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD,
        }
    }
}

impl CapacityPolicy {
    pub fn invalid_reason(&self) -> Option<&'static str> {
        if self.max_queued_per_trigger == 0 {
            return Some("max_queued_per_trigger must be at least 1");
        }
        if self.max_event_trigger_depth == 0 {
            return Some("max_event_trigger_depth must be at least 1");
        }
        if self.max_concurrent_children_per_thread == 0 {
            return Some("max_concurrent_children_per_thread must be at least 1");
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AdmissionCounts {
    pub background_active: usize,
    pub user_active: usize,
    pub kind_active: usize,
    pub trigger_active: usize,
    pub trigger_queued: usize,
    pub user_queued: usize,
}

impl AdmissionCounts {
    pub fn total_active(&self) -> usize {
        self.background_active + self.user_active
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionDecision {
    Admit,
    Queue,
    Overflow,
    /// INVARIANT: A cron fire of this trigger is already active or queued — the new fire
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

    pub fn effective_reserved_background(&self) -> usize {
        self.reserved_background.min(self.max_concurrent_total)
    }

    pub fn user_can_admit(&self, total_active: usize) -> bool {
        total_active < self.max_concurrent_total
    }

    /// INVARIANT: Decide admission for a NEW background submission. `has_trigger` is true
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
        // INVARIANT: Cron coalescing: a cron fire carries no distinct payload (just
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
    fn a_policy_stored_before_the_child_cap_reads_the_default() {
        let stored = serde_json::json!({ "max_concurrent_total": 4 });
        let read: CapacityPolicy = serde_json::from_value(stored).unwrap();
        assert_eq!(read.max_concurrent_total, 4);
        assert_eq!(
            read.max_concurrent_children_per_thread,
            DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD
        );
    }

    #[test]
    fn a_child_cap_of_zero_cannot_be_stored() {
        assert_eq!(policy().invalid_reason(), None);
        let zero = CapacityPolicy {
            max_concurrent_children_per_thread: 0,
            ..policy()
        };
        assert_eq!(
            zero.invalid_reason(),
            Some("max_concurrent_children_per_thread must be at least 1")
        );
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
        assert!(p.user_can_admit(p.max_concurrent_total - 1));
        assert!(!p.user_can_admit(p.max_concurrent_total));
    }

    #[test]
    fn user_ignores_kind_and_trigger_caps() {
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
        let d = policy().decide_submit(AdmissionCounts::default(), ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn cron_coalesces_when_one_active() {
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
        let counts = AdmissionCounts {
            trigger_queued: 1,
            ..Default::default()
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::Cron, true);
        assert_eq!(d, AdmissionDecision::Coalesce);
    }

    #[test]
    fn event_trigger_does_not_coalesce() {
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
            trigger_active: 1,
            trigger_queued: 1,
            user_active: 0,
            user_queued: 0,
        };
        let d = policy().decide_submit(counts, ThreadQueueKind::SubThread, false);
        assert_eq!(d, AdmissionDecision::Admit);
    }

    #[test]
    fn background_yields_free_slot_to_waiting_user_above_floor() {
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
        assert_eq!(p.effective_reserved_background(), 4);
    }

    #[test]
    fn queues_behind_own_triggers_backlog_even_with_free_capacity() {
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
        let p: CapacityPolicy =
            serde_json::from_str(r#"{ "max_concurrent_total": 2, "overflow": "pause-trigger" }"#)
                .unwrap();
        assert_eq!(p.max_concurrent_total, 2);
        assert_eq!(p.overflow, OverflowPolicy::PauseTrigger);
        assert_eq!(
            p.max_concurrent_per_trigger,
            CapacityPolicy::default().max_concurrent_per_trigger
        );
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

    #[test]
    fn a_policy_written_before_the_ceiling_existed_takes_the_default() {
        let legacy = serde_json::json!({
            "max_concurrent_total": 16,
            "max_concurrent_event_trigger": 6,
            "max_concurrent_cron": 6,
            "max_concurrent_sub_thread": 8,
            "max_concurrent_coding_agent": 12,
            "max_concurrent_per_trigger": 1,
            "max_queued_per_trigger": 25,
            "reserved_background": 4,
            "overflow": "drop-oldest"
        });
        let policy: CapacityPolicy = serde_json::from_value(legacy).expect("legacy policy loads");
        assert_eq!(
            policy.max_event_trigger_depth, DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
            "an absent ceiling means the default, never 0"
        );
        assert_eq!(policy.max_concurrent_total, 16, "the rest is untouched");
    }

    #[test]
    fn a_raised_ceiling_survives_the_persistence_round_trip() {
        let policy = CapacityPolicy {
            max_event_trigger_depth: 9,
            ..CapacityPolicy::default()
        };
        let back: CapacityPolicy =
            serde_json::from_value(serde_json::to_value(&policy).unwrap()).unwrap();
        assert_eq!(back.max_event_trigger_depth, 9);
    }
}
