//! Cron slots that passed with no run, and the notice that reports them.
//!
//! A slot older than the grace window never runs. The user still learns about
//! it: one `TriggerSlotsSkipped` and one inbox notice per trigger per gap.

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};
use crate::scheduler::user_tasks::{emit_trigger_notification, TriggerNotification};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// The slots of one gap that no run covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkippedSlots {
    pub(super) first: DateTime<Utc>,
    pub(super) last: DateTime<Utc>,
    pub(super) count: u32,
}

/// What settling the slots after a floor does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotPlan {
    /// No slot passed after the floor.
    Nothing,
    /// The latest slot is within grace. Firing it covers the older ones.
    CatchUp(DateTime<Utc>),
    /// Every slot is past grace, so none of them will run.
    Report(SkippedSlots),
}

/// Plan the slots in `(floor, now]`, merged across every expression.
pub(super) fn plan_missed_slots(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    floor: DateTime<Utc>,
    now: DateTime<Utc>,
    grace: Duration,
) -> SlotPlan {
    let floor_in_tz = floor.with_timezone(&tz);
    // Each expression yields ascending slots. Merging them streams the distinct
    // slots in order, so a per-second schedule never holds a week in memory.
    let mut streams: Vec<_> = schedules
        .iter()
        .map(|schedule| {
            schedule
                .after(&floor_in_tz)
                .map(|slot| slot.with_timezone(&Utc))
                .filter(|slot| *slot > floor)
                .take_while(|slot| *slot <= now)
                .peekable()
        })
        .collect();
    let mut first = None;
    let mut last = None;
    let mut count: u32 = 0;
    while let Some(next) = streams.iter_mut().filter_map(|s| s.peek().copied()).min() {
        for stream in &mut streams {
            stream.next_if_eq(&next);
        }
        first.get_or_insert(next);
        last = Some(next);
        count = count.saturating_add(1);
    }

    let (Some(first), Some(last)) = (first, last) else {
        return SlotPlan::Nothing;
    };
    if now - last < grace {
        return SlotPlan::CatchUp(last);
    }
    SlotPlan::Report(SkippedSlots { first, last, count })
}

/// What the store knows that rules a slot out of a report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SlotFloors {
    pub(super) last_run: Option<DateTime<Utc>>,
    pub(super) created: Option<DateTime<Utc>>,
    pub(super) config_changed: Option<DateTime<Utc>>,
    pub(super) last_reported: Option<DateTime<Utc>>,
    pub(super) restored_at: Option<DateTime<Utc>>,
}

impl SlotFloors {
    /// The instant after which a slot counts as missed.
    ///
    /// Never earlier than the boot-report lookback, so a long-dormant trigger
    /// reports at most a week of slots.
    pub(super) fn floor(
        &self,
        missed_after: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> DateTime<Utc> {
        let lookback = now - Duration::seconds(super::cut_fires::BOOT_REPORT_LOOKBACK_SECS);
        [
            self.last_run,
            self.created,
            self.config_changed,
            self.last_reported,
            self.restored_at,
            missed_after,
        ]
        .into_iter()
        .flatten()
        .fold(lookback, DateTime::max)
    }
}

/// Read the floors for one trigger. An error means the caller reports nothing.
pub(super) async fn load_slot_floors(
    pool: &PgPool,
    trigger_id: &str,
    in_memory_last_run: Option<DateTime<Utc>>,
) -> Result<SlotFloors, Error> {
    let history = crate::triggers::load_trigger_run_history(pool, trigger_id).await?;

    let config_changed: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT MAX(created) FROM events \
         WHERE aggregate = 'trigger' AND aggregate_id = $1 \
           AND event_type IN ('TriggerUpdated', 'TriggerEnabled', 'TriggerDisabled')",
    )
    .bind(trigger_id)
    .fetch_one(pool)
    .await?;

    let reported: Option<(Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT payload->'data'->>'last_slot', created FROM events \
         WHERE aggregate = 'trigger' AND aggregate_id = $1 \
           AND event_type = 'TriggerSlotsSkipped' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_optional(pool)
    .await?;
    let last_reported = reported.map(|(last_slot, created)| {
        crate::triggers::run_history::recorded_run_time(last_slot.as_deref(), created)
    });

    let restored_at = crate::core::snapshot::boundary::read(pool)
        .await?
        .map(|boundary| boundary.restored_at);

    Ok(SlotFloors {
        last_run: history.effective_last_run(in_memory_last_run),
        created: history.created_at,
        config_changed,
        last_reported,
        restored_at,
    })
}

/// The notice title and body for one gap.
fn notice_text(trigger_name: &str, slots: &SkippedSlots, tz: chrono_tz::Tz) -> (String, String) {
    let at = |slot: DateTime<Utc>| {
        slot.with_timezone(&tz)
            .format("%a %-d %b %H:%M")
            .to_string()
    };
    const WHY: &str = "because the computer was asleep or Lucidos was not running.";
    if slots.count == 1 {
        (
            format!("{trigger_name}: scheduled run skipped"),
            format!(
                "The run scheduled for {} did not happen, {WHY}",
                at(slots.last)
            ),
        )
    } else {
        (
            format!("{trigger_name}: {} scheduled runs skipped", slots.count),
            format!(
                "The {} runs scheduled from {} to {} did not happen, {WHY}",
                slots.count,
                at(slots.first),
                at(slots.last)
            ),
        )
    }
}

/// Record the gap, then post its notice. The caller sends the push.
///
/// The record goes first: it is what keeps the next boot from reporting the
/// same gap again, so a failed notice never turns into a repeated one.
pub(super) async fn report_skipped_slots(
    bus: &EventBus,
    trigger_id: &str,
    trigger_name: &str,
    tz: chrono_tz::Tz,
    slots: SkippedSlots,
) -> Result<TriggerNotification, Error> {
    bus.emit(BusEvent::System(SystemEvent::TriggerSlotsSkipped {
        trigger_id: trigger_id.to_string(),
        trigger_name: trigger_name.to_string(),
        first_slot: slots.first,
        last_slot: slots.last,
        count: slots.count,
    }))
    .await?;

    let (title, body) = notice_text(trigger_name, &slots, tz);
    let notice = TriggerNotification::new(&title, &body);
    let tap = Tap::Navigate {
        to: Box::new(NavigateUi {
            target: NavigateTarget::Trigger,
            id: Some(trigger_id.to_string()),
            ..Default::default()
        }),
    };
    emit_trigger_notification(bus, trigger_id, &notice, None, tap).await?;
    Ok(notice)
}

#[cfg(test)]
#[path = "skipped_slots_tests.rs"]
mod tests;
