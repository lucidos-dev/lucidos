//! Which arm a workspace is, and the preference rows that make it so.
//!
//! An arm has two dimensions: ADR 0085's context mode and ADR 0362's memory
//! module. This module is the whole seam between the harness and both.
//! Everything else asks it two questions: what preference rows does this arm
//! seed, and does the engine under test know those keys. Nothing else in the
//! crate names the preferences.
//!
//! **The engine may not carry a flag.** [`FlagAvailability`] makes that gap a
//! refusal with the key's name in it, rather than a null result that reads as a
//! pass. See `manipulation::preflight`.

use std::fmt;

use lucidos_engine::core::prefs::{self, PrefValue};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The workspace preference that turns the self-curated context mode on.
///
/// The seeding digest excludes the [`ARM_PREFERENCE_KEYS`] and nothing else.
/// The arms can then be proved byte-identical everywhere they are not (I1).
pub const CONTEXT_MODE_PREFERENCE_KEY: &str = prefs::SELF_CURATED_CONTEXT_MODE.key();

/// The workspace preference that picks the memory module (ADR 0362).
pub const MEMORY_MODULE_PREFERENCE_KEY: &str = prefs::MEMORY_MODULE.key();

/// How old a result gets before a sweep may take it.
pub const EXPIRE_AFTER_ROUNDS_KEY: &str = prefs::SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS.key();

/// How often the sweep runs.
pub const SWEEP_EVERY_ROUNDS_KEY: &str = prefs::SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS.key();

/// The variable that pins how old a result may get before a sweep takes it.
pub const EXPIRE_AFTER_ROUNDS_VAR: &str = "LUCIDOS_EVAL_EXPIRE_AFTER_ROUNDS";

/// The variable that pins how often the sweep runs.
pub const SWEEP_EVERY_ROUNDS_VAR: &str = "LUCIDOS_EVAL_SWEEP_EVERY_ROUNDS";

/// Every key an arm may differ on, and nothing else.
///
/// The seeding digest excludes exactly these. Excluding fewer than an arm
/// writes fails I1 before the first prompt, naming a mismatch the harness
/// itself created.
pub const ARM_PREFERENCE_KEYS: [&str; 4] = [
    CONTEXT_MODE_PREFERENCE_KEY,
    EXPIRE_AFTER_ROUNDS_KEY,
    SWEEP_EVERY_ROUNDS_KEY,
    MEMORY_MODULE_PREFERENCE_KEY,
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
    /// The engine's own defaults, read from its preference catalog. A pair
    /// copied here would keep reporting the shipped default long after it moved.
    fn default() -> Self {
        Self {
            expire_after_rounds: prefs::SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS.default_number()
                as usize,
            sweep_every_rounds: prefs::SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS.default_number()
                as usize,
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
    match prefs::lookup(catalog_key)?.value {
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

/// ADR 0085's context mode, one dimension of an arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextMode {
    /// Post-ADR-0086 behaviour with nothing removed.
    Control,
    /// Context mode on: memory recall and the conversation history go from
    /// round 2.
    Lean,
}

/// ADR 0362's memory module, the other dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryModule {
    /// The history summariser, memory recall and memory search. The engine's
    /// default, so a Classic arm seeds no row for it.
    Classic,
    /// Summary trees and memory views in place of history and recall.
    Tree,
}

impl MemoryModule {
    /// The `memory_module` value the engine reads.
    pub fn as_pref(self) -> &'static str {
        match self {
            MemoryModule::Classic => "classic",
            MemoryModule::Tree => "tree",
        }
    }
}

/// One arm: a context mode and a memory module.
///
/// Its name is its identity in every result row, workspace and database. A
/// Classic arm keeps the name it had before the memory module became a
/// dimension. Its rows, its database and its seeded preferences therefore stay
/// byte-identical (ADR 0362 I2). A Tree arm adds `-tree`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct Arm {
    pub context: ContextMode,
    pub memory: MemoryModule,
}

impl Arm {
    pub const CONTROL: Arm = Arm::of(ContextMode::Control, MemoryModule::Classic);
    pub const LEAN: Arm = Arm::of(ContextMode::Lean, MemoryModule::Classic);
    pub const CONTROL_TREE: Arm = Arm::of(ContextMode::Control, MemoryModule::Tree);
    pub const LEAN_TREE: Arm = Arm::of(ContextMode::Lean, MemoryModule::Tree);
    pub const ALL: [Arm; 4] = [Arm::CONTROL, Arm::LEAN, Arm::CONTROL_TREE, Arm::LEAN_TREE];

    const fn of(context: ContextMode, memory: MemoryModule) -> Arm {
        Arm { context, memory }
    }

    pub fn as_str(self) -> &'static str {
        match (self.context, self.memory) {
            (ContextMode::Control, MemoryModule::Classic) => "control",
            (ContextMode::Lean, MemoryModule::Classic) => "lean",
            (ContextMode::Control, MemoryModule::Tree) => "control-tree",
            (ContextMode::Lean, MemoryModule::Tree) => "lean-tree",
        }
    }

    pub fn parse(s: &str) -> Option<Arm> {
        let wanted = s.trim().to_ascii_lowercase();
        Arm::ALL.into_iter().find(|arm| arm.as_str() == wanted)
    }

    /// The preference rows this arm seeds, in order.
    ///
    /// A control arm writes no mode row rather than writing `false`, and a
    /// Classic arm writes no module row. An absent key and the default mean
    /// the same thing to the engine. An absent one keeps the digest exclusion
    /// honest: each row of difference between two arms exists in one arm only.
    ///
    /// Plural, because the schedule rides beside the mode. An arm that carries
    /// the flag and not the two numbers would run at the engine's defaults
    /// while the run believed it was sweeping.
    pub fn preference_rows(self, sweep: SweepPins) -> Vec<(&'static str, String)> {
        let mut rows = match self.context {
            ContextMode::Control => Vec::new(),
            ContextMode::Lean => vec![
                (CONTEXT_MODE_PREFERENCE_KEY, "true".to_string()),
                (
                    EXPIRE_AFTER_ROUNDS_KEY,
                    sweep.expire_after_rounds.to_string(),
                ),
                (SWEEP_EVERY_ROUNDS_KEY, sweep.sweep_every_rounds.to_string()),
            ],
        };
        if self.memory == MemoryModule::Tree {
            rows.push((
                MEMORY_MODULE_PREFERENCE_KEY,
                MemoryModule::Tree.as_pref().to_string(),
            ));
        }
        rows
    }

    /// Whether every round must carry a context panel.
    ///
    /// What the manipulation check asserts, per ADR 0087 decision 10. ADR 0109
    /// retires the ledger the check used to read, and the panel is a stricter
    /// oracle: a lean round renders one even with nothing addressable in it,
    /// because it always states the budget. So there is no round the flag can
    /// be silently inert on.
    pub fn expects_a_context_panel(self) -> bool {
        self.context == ContextMode::Lean
    }

    /// Whether every turn must take the Tree path. The engine runs Classic
    /// until its ready flag sets, so a Tree arm checks readiness itself.
    pub fn expects_tree_memory(self) -> bool {
        self.memory == MemoryModule::Tree
    }
}

impl From<Arm> for String {
    fn from(arm: Arm) -> String {
        arm.as_str().to_string()
    }
}

impl TryFrom<String> for Arm {
    type Error = String;

    fn try_from(name: String) -> Result<Arm, String> {
        Arm::parse(&name).ok_or_else(|| format!("{name} is not an arm"))
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
    /// The catalog carries every key. Every arm can be exercised.
    Present,
    /// A key is missing, so an arm seeding it would measure something other
    /// than what it reports. See [`missing_flag_message`].
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
        "arm_flag_missing: the engine's preference catalog is missing at least \
         one of `{}`. So this build lacks ADR 0085's context mode, its schedule, \
         or ADR 0362's memory module. The engine ignores a key it does not know. \
         A lean arm would then run as control, or a tree arm as classic, and the \
         result would be a null that reads as a pass. Land the missing key \
         before running any arm.",
        ARM_PREFERENCE_KEYS.join("`, `")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lean_rows() -> Vec<(&'static str, String)> {
        let defaults = SweepPins::default();
        vec![
            (CONTEXT_MODE_PREFERENCE_KEY, "true".to_string()),
            (
                EXPIRE_AFTER_ROUNDS_KEY,
                defaults.expire_after_rounds.to_string(),
            ),
            (
                SWEEP_EVERY_ROUNDS_KEY,
                defaults.sweep_every_rounds.to_string(),
            ),
        ]
    }

    /// I2: the Classic arms seed exactly what they seeded before the memory
    /// module was a dimension.
    #[test]
    fn only_the_lean_arm_seeds_preference_rows() {
        assert!(Arm::CONTROL
            .preference_rows(SweepPins::default())
            .is_empty());
        assert_eq!(Arm::LEAN.preference_rows(SweepPins::default()), lean_rows());
    }

    /// A Tree arm is its Classic twin plus one row, so the two differ by the
    /// module and nothing else.
    #[test]
    fn a_tree_arm_adds_the_module_row_to_its_classic_twin() {
        let module = (MEMORY_MODULE_PREFERENCE_KEY, "tree".to_string());
        assert_eq!(
            Arm::CONTROL_TREE.preference_rows(SweepPins::default()),
            vec![module.clone()]
        );
        let mut lean_tree = lean_rows();
        lean_tree.push(module);
        assert_eq!(
            Arm::LEAN_TREE.preference_rows(SweepPins::default()),
            lean_tree
        );
    }

    /// The value the Tree arm seeds is one the engine's catalog accepts, read
    /// from the catalog rather than restated.
    #[test]
    fn both_module_values_are_ones_the_catalog_accepts() {
        let spec = prefs::lookup(MEMORY_MODULE_PREFERENCE_KEY)
            .expect("the catalog declares the memory module");
        let PrefValue::Enum(options) = spec.value else {
            panic!("the memory module is no longer an enum preference");
        };
        for module in [MemoryModule::Classic, MemoryModule::Tree] {
            assert!(
                options.contains(&module.as_pref()),
                "{} is not a memory module the engine accepts",
                module.as_pref()
            );
        }
    }

    /// Every key an arm writes must be excluded from the digest, or the run
    /// dies on a difference the harness put there itself.
    #[test]
    fn every_seeded_key_is_one_the_digest_excludes() {
        for arm in Arm::ALL {
            for (key, _) in arm.preference_rows(SweepPins::default()) {
                assert!(
                    ARM_PREFERENCE_KEYS.contains(&key),
                    "{key} is seeded by the {arm} arm but not excluded from the seed digest"
                );
            }
        }
    }

    /// I2: a Classic arm's name is its name from before, in every row and
    /// database that carries it.
    #[test]
    fn the_classic_arms_keep_their_wire_names() {
        assert_eq!(serde_json::to_string(&Arm::CONTROL).unwrap(), "\"control\"");
        assert_eq!(serde_json::to_string(&Arm::LEAN).unwrap(), "\"lean\"");
        assert_eq!(
            serde_json::to_string(&Arm::LEAN_TREE).unwrap(),
            "\"lean-tree\""
        );
        let read: Arm = serde_json::from_str("\"control-tree\"").unwrap();
        assert_eq!(read, Arm::CONTROL_TREE);
        assert!(serde_json::from_str::<Arm>("\"tree\"").is_err());
    }

    #[test]
    fn a_swept_arm_seeds_the_values_it_was_given() {
        let rows = Arm::LEAN.preference_rows(SweepPins {
            expire_after_rounds: 3,
            sweep_every_rounds: 7,
        });
        assert!(rows.contains(&(EXPIRE_AFTER_ROUNDS_KEY, "3".to_string())));
        assert!(rows.contains(&(SWEEP_EVERY_ROUNDS_KEY, "7".to_string())));
    }

    #[test]
    fn a_catalog_without_the_key_reports_missing() {
        let catalog = [
            prefs::TIMEZONE.key(),
            prefs::LANGUAGE.key(),
            prefs::CHAT_MODEL.key(),
        ];
        assert_eq!(
            FlagAvailability::from_catalog_keys(&catalog),
            FlagAvailability::Missing
        );
    }

    #[test]
    fn a_catalog_with_every_key_reports_present() {
        let mut catalog = vec![prefs::TIMEZONE.key()];
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
        let catalog = [prefs::TIMEZONE.key(), CONTEXT_MODE_PREFERENCE_KEY];
        assert_eq!(
            FlagAvailability::from_catalog_keys(&catalog),
            FlagAvailability::Missing
        );
        assert_eq!(
            FlagAvailability::missing_keys(&catalog),
            vec![
                EXPIRE_AFTER_ROUNDS_KEY,
                SWEEP_EVERY_ROUNDS_KEY,
                MEMORY_MODULE_PREFERENCE_KEY
            ]
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
        let default = SweepPins::default().expire_after_rounds;
        let resolved = checked(
            EXPIRE_AFTER_ROUNDS_VAR,
            EXPIRE_AFTER_ROUNDS_KEY,
            None,
            default,
        );
        assert_eq!(resolved.unwrap(), default);
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
        for arm in Arm::ALL {
            assert_eq!(Arm::parse(arm.as_str()), Some(arm));
        }
        assert_eq!(Arm::parse("nolean"), None);
    }

    /// The panel follows the context mode alone, and Tree the module alone.
    #[test]
    fn each_gate_reads_its_own_dimension() {
        for arm in Arm::ALL {
            assert_eq!(
                arm.expects_a_context_panel(),
                arm.context == ContextMode::Lean
            );
            assert_eq!(arm.expects_tree_memory(), arm.memory == MemoryModule::Tree);
        }
    }
}
