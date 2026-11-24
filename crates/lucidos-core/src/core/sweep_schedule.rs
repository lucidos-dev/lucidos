//! Schedule for expiring curated tool results.

use super::prefs;

/// A ceiling on both numbers, so a mistyped preference cannot make
/// [`SweepSchedule::leaves_at`] walk forever.
const MAX_SCHEDULE_ROUNDS: usize = 1_000;

/// When results leave: an expiry age and how often the sweep runs.
///
/// **The clear-out is a schedule, not a per-round drop.** Removing a pair from
/// the middle of the array invalidates every cached byte after the cut, and
/// both message-tier breakpoints sit behind it. Paying that on every round
/// costs about ten times what it saves. So nine rounds in ten are pure appends,
/// and the tenth takes everything past expiry at once.
///
/// On the defaults an item lives 6 to 15 rounds, averaging the ten decision 1
/// asks for. The panel states each item's exact remainder, so the variable
/// lifetime never reaches the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepSchedule {
    pub expire_after_rounds: usize,
    pub sweep_every_rounds: usize,
}

impl Default for SweepSchedule {
    /// The catalog defaults of the two schedule preferences.
    fn default() -> Self {
        Self::new(
            prefs::SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS
                .default_number()
                .round() as usize,
            prefs::SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS
                .default_number()
                .round() as usize,
        )
    }
}

impl SweepSchedule {
    /// Both numbers are provisional, so the eval sweeps them without a rebuild.
    /// A zero interval would divide by zero, and either number unbounded would
    /// leave [`Self::leaves_at`] walking, so both are clamped here.
    pub fn new(expire_after_rounds: usize, sweep_every_rounds: usize) -> Self {
        Self {
            expire_after_rounds: expire_after_rounds.min(MAX_SCHEDULE_ROUNDS),
            sweep_every_rounds: sweep_every_rounds.clamp(1, MAX_SCHEDULE_ROUNDS),
        }
    }

    /// Whether the pass runs at the top of this round.
    pub fn is_sweep_round(self, round: usize) -> bool {
        round > 0 && round.is_multiple_of(self.sweep_every_rounds)
    }

    /// The next round the pass runs on, strictly after `round`.
    pub(crate) fn next_sweep_after(self, round: usize) -> usize {
        (round / self.sweep_every_rounds + 1) * self.sweep_every_rounds
    }

    /// How many rounds until the next sweep. The panel states this once.
    pub fn rounds_to_next_sweep(self, round: usize) -> usize {
        self.next_sweep_after(round).saturating_sub(round)
    }

    /// Whether an item first seen on `first_seen` is old enough to go.
    pub fn is_past_expiry(self, first_seen: usize, round: usize) -> bool {
        round.saturating_sub(first_seen) > self.expire_after_rounds
    }

    /// The round an item's pair leaves on, given it survived `round`.
    pub fn leaves_at(self, first_seen: usize, round: usize) -> usize {
        let mut at = self.next_sweep_after(round);
        while !self.is_past_expiry(first_seen, at) {
            at += self.sweep_every_rounds;
        }
        at
    }

    /// How many rounds an item has left, counted from `round`.
    pub fn leaves_in(self, first_seen: usize, round: usize) -> usize {
        self.leaves_at(first_seen, round).saturating_sub(round)
    }
}
