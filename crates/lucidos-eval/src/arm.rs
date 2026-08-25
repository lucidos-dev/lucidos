//! Which arm a workspace is, and the one preference row that makes it so.
//!
//! This module is the whole seam between the harness and ADR 0085's context
//! mode. Everything else asks it two questions: what preference row does this
//! arm seed, and does the engine under test know that key. Nothing else in the
//! crate names the preference.
//!
//! **The engine may not carry the flag.** [`FlagAvailability`] makes that gap a
//! refusal with the key's name in it, rather than a null result that reads as a
//! pass. See `manipulation::preflight`.

use std::fmt;

use lucidos_engine::core::preference_catalog::{self, PrefValue};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The workspace preference that turns the self-curated context mode on.
///
/// The seeding digest excludes this key and the two beside it, and nothing
/// else. The two arms can then be proved byte-identical everywhere they are
/// not (I1).
pub const CONTEXT_MODE_PREFERENCE_KEY: &str = "self_curated_context_mode";

/// How old a result gets before a sweep may take it.
pub const EXPIRE_AFTER_ROUNDS_KEY: &str = "self_curated_context_expire_after_rounds";

/// How often the sweep runs.
pub const SWEEP_EVERY_ROUNDS_KEY: &str = "self_curated_context_sweep_every_rounds";

/// The variable that pins how old a result may get before a sweep takes it.
pub const EXPIRE_AFTER_ROUNDS_VAR: &str = "LUCIDOS_EVAL_EXPIRE_AFTER_ROUNDS";

/// The variable that pins how often the sweep runs.
pub const SWEEP_EVERY_ROUNDS_VAR: &str = "LUCIDOS_EVAL_SWEEP_EVERY_ROUNDS";

/// Every key an arm may differ on, and nothing else.
///
/// The seeding digest excludes exactly these. Excluding fewer than an arm
/// writes fails I1 before the first prompt, naming a mismatch the harness
/// itself created.
pub const ARM_PREFERENCE_KEYS: [&str; 3] = [
    CONTEXT_MODE_PREFERENCE_KEY,
    EXPIRE_AFTER_ROUNDS_KEY,
    SWEEP_EVERY_ROUNDS_KEY,
];

/// The schedule a lean arm runs at.
///
/// Both numbers are provisional, so they live beside the mode's own key rather
/// than in the binary. Two arms swept at different values are two different
/// designs, which is why `guidance_hash` covers the rendered prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepPins {
    pub expire_after_rounds: usize,
    pub sweep_every_rounds: usize,
}

impl Default for SweepPins {
    /// The engine's own defaults, read from the engine. A pair copied here
    /// would keep reporting the shipped default long after it moved.
    fn default() -> Self {
        Self {
            expire_after_rounds: lucidos_engine::engine::DEFAULT_EXPIRE_AFTER_ROUNDS,
            sweep_every_rounds: lucidos_engine::engine::DEFAULT_SWEEP_EVERY_ROUNDS,
        }
    }
}

impl SweepPins {
    /// The schedule this run measures, from the environment or the defaults.
    ///
    /// Same knob shape as every other pin in this harness: a sweep is started
    /// by setting a variable, never by editing a constant.
    ///
    /// Fallible, because a pin the engine will not take is not a pin. It falls
    /// back. The arm then sweeps at the engine's own schedule, while every row
    /// of the run carries the number the operator wrote.
    pub fn from_env() -> Fallible<SweepPins> {
        let default = Self::default();
        Ok(SweepPins {
            expire_after_rounds: checked(
                EXPIRE_AFTER_ROUNDS_VAR,
                EXPIRE_AFTER_ROUNDS_KEY,
                written(EXPIRE_AFTER_ROUNDS_VAR).as_deref(),
                default.expire_after_rounds,
            )?,
            sweep_every_rounds: checked(
                SWEEP_EVERY_ROUNDS_VAR,
                SWEEP_EVERY_ROUNDS_KEY,
                written(SWEEP_EVERY_ROUNDS_VAR).as_deref(),
                default.sweep_every_rounds,
            )?,
        })
    }
}

/// One schedule pin, as the engine would have to accept it.
///
/// `raw` is what the operator wrote, or `None` for an unset variable. Unset is
/// the engine's own default, which is in range by construction. Anything else
/// has to be a whole number of rounds inside the catalog's range.
fn checked(
    env_var: &str,
    catalog_key: &str,
    raw: Option<&str>,
    fallback: usize,
) -> Fallible<usize> {
    let Some(raw) = raw else {
        return Ok(fallback);
    };
    // A build missing the key states no range, so a round count's widest.
    // `FlagAvailability` refuses such a build by name, before the first prompt.
    let (min, max) = catalog_range(catalog_key).unwrap_or((1, usize::MAX));
    match raw.parse::<usize>() {
        Ok(rounds) if (min..=max).contains(&rounds) => Ok(rounds),
        _ => Err(bad_pin(env_var, raw, min, max)),
    }
}

/// The range the engine's own catalog accepts for a schedule key.
///
/// Read rather than restated, so a widened bound reaches the harness with the
/// engine that widened it.
fn catalog_range(catalog_key: &str) -> Option<(usize, usize)> {
    match preference_catalog::lookup(catalog_key)?.value {
        PrefValue::Number { min, max } if min >= 0.0 && max >= min => {
            Some((min.ceil() as usize, max.floor() as usize))
        }
        _ => None,
    }
}

/// Why a schedule pin was refused, naming the variable and the range.
fn bad_pin(
    env_var: &str,
    raw: &str,
    min: usize,
    max: usize,
) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "bad_schedule_pin: {env_var} is set to `{raw}`, and a schedule is a whole number of \
         rounds from {min} to {max}. The engine's own preference catalog states that range, \
         so the arm's preference row would be rejected. The arm would sweep at the engine's \
         default while every result claimed this pin. Set a value in range, or unset \
         {env_var} to run at the default."
    )
    .into()
}

/// What an operator actually wrote, treating unset and blank alike.
///
/// `env::var` answers `Ok("")` for an exported-but-empty variable, and a bare
/// `LUCIDOS_EVAL_SWEEP_EVERY_ROUNDS=` means "I set no schedule".
fn written(env_var: &str) -> Option<String> {
    std::env::var(env_var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    /// Post-ADR-0086 behaviour with nothing removed.
    Control,
    /// Context mode on: memory recall and the conversation history go from
    /// round 2.
    Lean,
}

impl Arm {
    pub const BOTH: [Arm; 2] = [Arm::Control, Arm::Lean];

    pub fn as_str(self) -> &'static str {
        match self {
            Arm::Control => "control",
            Arm::Lean => "lean",
        }
    }

    pub fn parse(s: &str) -> Option<Arm> {
        match s.trim().to_ascii_lowercase().as_str() {
            "control" => Some(Arm::Control),
            "lean" => Some(Arm::Lean),
            _ => None,
        }
    }

    /// The preference rows this arm seeds, in order.
    ///
    /// The control arm writes nothing at all rather than writing `false`. An
    /// absent key and a false one mean the same thing to the engine, and an
    /// absent one keeps the digest exclusion honest: the rows of difference
    /// between the arms exist in one arm only.
    ///
    /// Plural, because the schedule rides beside the mode. An arm that carries
    /// the flag and not the two numbers would run at the engine's defaults
    /// while the run believed it was sweeping.
    pub fn preference_rows(self, sweep: SweepPins) -> Vec<(&'static str, String)> {
        match self {
            Arm::Control => Vec::new(),
            Arm::Lean => vec![
                (CONTEXT_MODE_PREFERENCE_KEY, "true".to_string()),
                (
                    EXPIRE_AFTER_ROUNDS_KEY,
                    sweep.expire_after_rounds.to_string(),
                ),
                (SWEEP_EVERY_ROUNDS_KEY, sweep.sweep_every_rounds.to_string()),
            ],
        }
    }

    /// Whether every round must carry a context panel.
    ///
    /// What the manipulation check asserts, per ADR 0087 decision 10. ADR 0109
    /// retires the ledger the check used to read, and the panel is a stricter
    /// oracle: a lean round renders one even with nothing addressable in it,
    /// because it always states the budget. So there is no round the flag can
    /// be silently inert on.
    pub fn expects_a_context_panel(self) -> bool {
        match self {
            Arm::Control => false,
            Arm::Lean => true,
        }
    }
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the engine under test knows about [`ARM_PREFERENCE_KEYS`].
///
/// Resolved by reading the engine's own preference catalog, so the answer
/// comes from the build being measured rather than from this crate's opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagAvailability {
    /// The catalog carries every key. The lean arm can be exercised.
    Present,
    /// A key is missing, so a lean run would measure something other than what
    /// it reports. See [`missing_flag_message`].
    Missing,
}

impl FlagAvailability {
    /// Every key an arm writes must exist, not just the mode's own.
    ///
    /// A build carrying the mode but not the two schedule keys seeds two rows
    /// the engine ignores. It sweeps at the engine's defaults, while the run
    /// stamps every result with the schedule it believed it pinned. That is
    /// the null-that-reads-as-a-pass this check exists to refuse.
    pub fn from_catalog_keys<S: AsRef<str>>(keys: &[S]) -> Self {
        if Self::missing_keys(keys).is_empty() {
            FlagAvailability::Present
        } else {
            FlagAvailability::Missing
        }
    }

    /// The arm keys this catalog does not carry, in declaration order.
    pub fn missing_keys<S: AsRef<str>>(keys: &[S]) -> Vec<&'static str> {
        ARM_PREFERENCE_KEYS
            .into_iter()
            .filter(|wanted| !keys.iter().any(|k| k.as_ref() == *wanted))
            .collect()
    }
}

/// The refusal text every caller prints when a key is absent.
///
/// One string, so the sentence is the same whichever command reached it, and so
/// the key is never spelled by hand a second time.
pub fn missing_flag_message() -> String {
    format!(
        "context_mode_flag_missing: the engine's preference catalog is missing \
         at least one of `{}`. So ADR 0085's context mode is not implemented in \
         this build, or not at the schedule this run pins. The lean arm would \
         run identically to the control arm, or at the engine's own defaults, \
         and the result would be a null that reads as a pass. Land ADR 0085 \
         before running either arm.",
        ARM_PREFERENCE_KEYS.join("`, `")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_lean_arm_seeds_preference_rows() {
        assert!(Arm::Control
            .preference_rows(SweepPins::default())
            .is_empty());
        assert_eq!(
            Arm::Lean.preference_rows(SweepPins::default()),
            vec![
                (CONTEXT_MODE_PREFERENCE_KEY, "true".to_string()),
                (EXPIRE_AFTER_ROUNDS_KEY, "5".to_string()),
                (SWEEP_EVERY_ROUNDS_KEY, "10".to_string()),
            ]
        );
    }

    /// Every key an arm writes must be excluded from the digest, or the run
    /// dies on a difference the harness put there itself.
    #[test]
    fn every_seeded_key_is_one_the_digest_excludes() {
        for (key, _) in Arm::Lean.preference_rows(SweepPins::default()) {
            assert!(
                ARM_PREFERENCE_KEYS.contains(&key),
                "{key} is seeded but not excluded from the seed digest"
            );
        }
    }

    #[test]
    fn a_swept_arm_seeds_the_values_it_was_given() {
        let rows = Arm::Lean.preference_rows(SweepPins {
            expire_after_rounds: 3,
            sweep_every_rounds: 7,
        });
        assert!(rows.contains(&(EXPIRE_AFTER_ROUNDS_KEY, "3".to_string())));
        assert!(rows.contains(&(SWEEP_EVERY_ROUNDS_KEY, "7".to_string())));
    }

    #[test]
    fn a_catalog_without_the_key_reports_missing() {
        let catalog = ["timezone", "language", "chat_model"];
        assert_eq!(
            FlagAvailability::from_catalog_keys(&catalog),
            FlagAvailability::Missing
        );
    }

    #[test]
    fn a_catalog_with_every_key_reports_present() {
        let mut catalog = vec!["timezone"];
        catalog.extend(ARM_PREFERENCE_KEYS);
        assert_eq!(
            FlagAvailability::from_catalog_keys(&catalog),
            FlagAvailability::Present
        );
    }

    /// The mode's own key alone is not enough. Seeding a schedule the engine
    /// has no key for runs the arm at the engine's defaults, and the run would
    /// report the numbers it seeded.
    #[test]
    fn the_mode_key_alone_reports_missing() {
        let catalog = ["timezone", CONTEXT_MODE_PREFERENCE_KEY];
        assert_eq!(
            FlagAvailability::from_catalog_keys(&catalog),
            FlagAvailability::Missing
        );
        assert_eq!(
            FlagAvailability::missing_keys(&catalog),
            vec![EXPIRE_AFTER_ROUNDS_KEY, SWEEP_EVERY_ROUNDS_KEY]
        );
    }

    #[test]
    fn the_refusal_names_every_key_it_needs() {
        let message = missing_flag_message();
        for key in ARM_PREFERENCE_KEYS {
            assert!(message.contains(key), "the refusal never names `{key}`");
        }
    }

    /// The catalog is where the accepted range comes from, so a spec that
    /// stopped being a number would leave the parser guessing.
    #[test]
    fn the_catalog_states_a_range_for_both_schedule_keys() {
        for key in [EXPIRE_AFTER_ROUNDS_KEY, SWEEP_EVERY_ROUNDS_KEY] {
            let (min, max) = catalog_range(key)
                .unwrap_or_else(|| panic!("{key} is no longer a numeric preference"));
            assert!(min >= 1, "{key} accepts {min} rounds, which is no schedule");
            assert!(max > min, "{key} accepts one value only");
        }
    }

    #[test]
    fn an_unset_pin_is_the_engines_own_default() {
        let resolved = checked(EXPIRE_AFTER_ROUNDS_VAR, EXPIRE_AFTER_ROUNDS_KEY, None, 5);
        assert_eq!(resolved.unwrap(), 5);
    }

    #[test]
    fn a_pin_inside_the_range_is_taken() {
        let resolved = checked(
            SWEEP_EVERY_ROUNDS_VAR,
            SWEEP_EVERY_ROUNDS_KEY,
            Some("7"),
            10,
        );
        assert_eq!(resolved.unwrap(), 7);
    }

    /// The defect this replaces: every one of these read as the default, and
    /// the run then reported a schedule nothing had swept at.
    #[test]
    fn a_pin_the_engine_would_reject_fails_the_run() {
        for raw in ["0", "1001", "five", "-1", "3.5", "10rounds"] {
            let resolved = checked(
                EXPIRE_AFTER_ROUNDS_VAR,
                EXPIRE_AFTER_ROUNDS_KEY,
                Some(raw),
                5,
            );
            assert!(resolved.is_err(), "`{raw}` was taken as a schedule");
        }
    }

    /// An operator reading the refusal has to be able to fix it without
    /// opening the engine's catalog.
    #[test]
    fn the_refusal_names_the_variable_and_the_range() {
        let message = checked(
            SWEEP_EVERY_ROUNDS_VAR,
            SWEEP_EVERY_ROUNDS_KEY,
            Some("0"),
            10,
        )
        .unwrap_err()
        .to_string();
        let (min, max) = catalog_range(SWEEP_EVERY_ROUNDS_KEY).expect("a numeric preference");
        assert!(message.contains("bad_schedule_pin"), "{message}");
        assert!(message.contains(SWEEP_EVERY_ROUNDS_VAR), "{message}");
        assert!(
            message.contains(&format!("from {min} to {max}")),
            "{message}"
        );
    }

    #[test]
    fn arm_round_trips_through_its_wire_name() {
        for arm in Arm::BOTH {
            assert_eq!(Arm::parse(arm.as_str()), Some(arm));
        }
        assert_eq!(Arm::parse("nolean"), None);
    }
}
