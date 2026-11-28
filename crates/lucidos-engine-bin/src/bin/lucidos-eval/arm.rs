use std::fmt;

use lucidos_engine::core::prefs::{self, PrefValue};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const CONTEXT_MODE_PREFERENCE_KEY: &str = prefs::SELF_CURATED_CONTEXT_MODE.key();

pub const MEMORY_MODULE_PREFERENCE_KEY: &str = prefs::MEMORY_MODULE.key();

pub const EXPIRE_AFTER_ROUNDS_KEY: &str = prefs::SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS.key();

pub const SWEEP_EVERY_ROUNDS_KEY: &str = prefs::SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS.key();

pub const EXPIRE_AFTER_ROUNDS_VAR: &str = "LUCIDOS_EVAL_EXPIRE_AFTER_ROUNDS";

pub const SWEEP_EVERY_ROUNDS_VAR: &str = "LUCIDOS_EVAL_SWEEP_EVERY_ROUNDS";

pub const ARM_PREFERENCE_KEYS: [&str; 4] = [
    CONTEXT_MODE_PREFERENCE_KEY,
    EXPIRE_AFTER_ROUNDS_KEY,
    SWEEP_EVERY_ROUNDS_KEY,
    MEMORY_MODULE_PREFERENCE_KEY,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepPins {
    pub expire_after_rounds: usize,
    pub sweep_every_rounds: usize,
}

impl Default for SweepPins {
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

fn checked(
    env_var: &str,
    catalog_key: &str,
    raw: Option<&str>,
    fallback: usize,
) -> Fallible<usize> {
    let Some(raw) = raw else {
        return Ok(fallback);
    };
    let (min, max) = catalog_range(catalog_key).unwrap_or((1, usize::MAX));
    match raw.parse::<usize>() {
        Ok(rounds) if (min..=max).contains(&rounds) => Ok(rounds),
        _ => Err(bad_pin(env_var, raw, min, max)),
    }
}

fn catalog_range(catalog_key: &str) -> Option<(usize, usize)> {
    match prefs::lookup(catalog_key)?.value {
        PrefValue::Number { min, max } if min >= 0.0 && max >= min => {
            Some((min.ceil() as usize, max.floor() as usize))
        }
        _ => None,
    }
}

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

fn written(env_var: &str) -> Option<String> {
    std::env::var(env_var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextMode {
    Control,
    Lean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryModule {
    Classic,
    Tree,
}

impl MemoryModule {
    pub fn as_pref(self) -> &'static str {
        match self {
            MemoryModule::Classic => "classic",
            MemoryModule::Tree => "tree",
        }
    }
}

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
        let module = self.memory.as_pref();
        if module != prefs::MEMORY_MODULE_WHILE_UNSET {
            rows.push((MEMORY_MODULE_PREFERENCE_KEY, module.to_string()));
        }
        rows
    }

    pub fn expects_a_context_panel(self) -> bool {
        self.context == ContextMode::Lean
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagAvailability {
    Present,
    Missing,
}

impl FlagAvailability {
    pub fn from_catalog_keys<S: AsRef<str>>(keys: &[S]) -> Self {
        if Self::missing_keys(keys).is_empty() {
            FlagAvailability::Present
        } else {
            FlagAvailability::Missing
        }
    }

    pub fn missing_keys<S: AsRef<str>>(keys: &[S]) -> Vec<&'static str> {
        ARM_PREFERENCE_KEYS
            .into_iter()
            .filter(|wanted| !keys.iter().any(|k| k.as_ref() == *wanted))
            .collect()
    }
}

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

    fn seeded_module(arm: Arm) -> String {
        arm.preference_rows(SweepPins::default())
            .into_iter()
            .find(|(key, _)| *key == MEMORY_MODULE_PREFERENCE_KEY)
            .map_or_else(
                || prefs::MEMORY_MODULE_WHILE_UNSET.to_string(),
                |(_, value)| value,
            )
    }

    #[test]
    fn every_arm_seeds_the_module_it_names() {
        for arm in Arm::ALL {
            assert_eq!(seeded_module(arm), arm.memory.as_pref(), "{arm}");
        }
    }

    #[test]
    fn twin_arms_differ_by_the_module_row_alone() {
        let without_module = |arm: Arm| -> Vec<(&'static str, String)> {
            arm.preference_rows(SweepPins::default())
                .into_iter()
                .filter(|(key, _)| *key != MEMORY_MODULE_PREFERENCE_KEY)
                .collect()
        };
        assert!(without_module(Arm::CONTROL).is_empty());
        assert!(without_module(Arm::CONTROL_TREE).is_empty());
        assert_eq!(without_module(Arm::LEAN), lean_rows());
        assert_eq!(without_module(Arm::LEAN_TREE), lean_rows());
        for (classic, tree) in [
            (Arm::CONTROL, Arm::CONTROL_TREE),
            (Arm::LEAN, Arm::LEAN_TREE),
        ] {
            let rows = |arm: Arm| arm.preference_rows(SweepPins::default()).len();
            assert_eq!(
                rows(classic).abs_diff(rows(tree)),
                1,
                "{classic} and {tree}"
            );
        }
    }

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
