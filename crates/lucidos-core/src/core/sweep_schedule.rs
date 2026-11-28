use super::prefs;

const MAX_SCHEDULE_ROUNDS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepSchedule {
    pub expire_after_rounds: usize,
    pub sweep_every_rounds: usize,
}

impl Default for SweepSchedule {
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
    /// INVARIANT: Both numbers are provisional, so the eval sweeps them without a rebuild.
    /// A zero interval would divide by zero, and either number unbounded would
    /// leave [`Self::leaves_at`] walking, so both are clamped here.
    pub fn new(expire_after_rounds: usize, sweep_every_rounds: usize) -> Self {
        Self {
            expire_after_rounds: expire_after_rounds.min(MAX_SCHEDULE_ROUNDS),
            sweep_every_rounds: sweep_every_rounds.clamp(1, MAX_SCHEDULE_ROUNDS),
        }
    }

    pub fn is_sweep_round(self, round: usize) -> bool {
        round > 0 && round.is_multiple_of(self.sweep_every_rounds)
    }

    pub(crate) fn next_sweep_after(self, round: usize) -> usize {
        (round / self.sweep_every_rounds + 1) * self.sweep_every_rounds
    }

    pub fn rounds_to_next_sweep(self, round: usize) -> usize {
        self.next_sweep_after(round).saturating_sub(round)
    }

    pub fn is_past_expiry(self, first_seen: usize, round: usize) -> bool {
        round.saturating_sub(first_seen) > self.expire_after_rounds
    }

    pub fn leaves_at(self, first_seen: usize, round: usize) -> usize {
        let mut at = self.next_sweep_after(round);
        while !self.is_past_expiry(first_seen, at) {
            at += self.sweep_every_rounds;
        }
        at
    }

    pub fn leaves_in(self, first_seen: usize, round: usize) -> usize {
        self.leaves_at(first_seen, round).saturating_sub(round)
    }
}
