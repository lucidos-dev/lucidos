//! The *auto-compact trial*: which Claude Code sessions run with
//! `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, and what `SessionStarted` records.
//!
//! A temporary measure: `docs/temporary-measures.md` § "Auto-compact trial".
//! The decision rule lives in
//! `docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The env var Claude Code reads its auto-compact window from.
pub const AUTO_COMPACT_WINDOW_ENV: &str = "CLAUDE_CODE_AUTO_COMPACT_WINDOW";

/// The window the trial's treatment arm runs with, in tokens.
pub const TRIAL_WINDOW_TOKENS: u64 = 400_000;

/// The values of `coding_agent_auto_compact_trial`, as the catalog lists them.
pub const TRIAL_MODES: &[&str] = &[
    TrialMode::Off.as_str(),
    TrialMode::Ab.as_str(),
    TrialMode::On.as_str(),
];

/// The `coding_agent_auto_compact_trial` preference, parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialMode {
    Off,
    /// Half of all thread families get the window, by [`arm_for_family`].
    Ab,
    /// Every family gets the window.
    On,
}

impl TrialMode {
    const ALL: [Self; 3] = [Self::Off, Self::Ab, Self::On];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ab => "ab",
            Self::On => "on",
        }
    }

    /// A value outside [`TRIAL_MODES`] reads as `off`, the catalog default.
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .unwrap_or(Self::Off)
    }
}

/// Which arm of the trial a session ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCompactArm {
    /// The engine set no window.
    Control,
    /// The engine set [`TRIAL_WINDOW_TOKENS`].
    Window,
    /// A workspace env var set the window, so the engine left it alone.
    Override,
}

/// What one spawn runs with. The env var and the `SessionStarted` record are
/// both read from this one value, so they cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactSetting {
    pub arm: AutoCompactArm,
    /// The window the session runs with. `None` under `control`, and under
    /// `override` when the workspace value is not a number.
    pub window: Option<u64>,
}

impl AutoCompactSetting {
    /// The value the engine itself writes into the spawn env. Only the
    /// `window` arm writes one: an override already reached the env through
    /// the workspace's own variables.
    pub fn engine_window(&self) -> Option<u64> {
        match self.arm {
            AutoCompactArm::Window => self.window,
            AutoCompactArm::Control | AutoCompactArm::Override => None,
        }
    }
}

/// The arm a thread family runs in under `ab`. The low bit of the first byte
/// of SHA-256 over the root's uuid bytes: stable across restarts and engines,
/// unlike `std`'s seeded `Hasher`.
pub fn arm_for_family(root: Uuid) -> AutoCompactArm {
    let digest = Sha256::digest(root.as_bytes());
    if digest[0] & 1 == 1 {
        AutoCompactArm::Window
    } else {
        AutoCompactArm::Control
    }
}

/// The setting for a spawn in the family rooted at `root`. `None` when the
/// trial is off, which leaves the spawn exactly as it was before the trial.
/// `workspace_value` is a `CLAUDE_CODE_AUTO_COMPACT_WINDOW` the session would
/// get anyway, and it always wins.
pub fn decide(
    mode: TrialMode,
    workspace_value: Option<&str>,
    root: Uuid,
) -> Option<AutoCompactSetting> {
    if mode == TrialMode::Off {
        return None;
    }
    if let Some(value) = workspace_value {
        return Some(AutoCompactSetting {
            arm: AutoCompactArm::Override,
            window: value.trim().parse().ok(),
        });
    }
    let arm = match mode {
        TrialMode::On => AutoCompactArm::Window,
        TrialMode::Ab | TrialMode::Off => arm_for_family(root),
    };
    Some(trial_setting(arm))
}

/// The setting of an engine-chosen arm: `window` runs with
/// [`TRIAL_WINDOW_TOKENS`], `control` with none.
fn trial_setting(arm: AutoCompactArm) -> AutoCompactSetting {
    AutoCompactSetting {
        arm,
        window: (arm == AutoCompactArm::Window).then_some(TRIAL_WINDOW_TOKENS),
    }
}

/// The engine-chosen arm `thread_id` first ran in under `ab`, from its
/// earliest `SessionStarted`. A detach can move a thread's family root, and
/// a thread must still keep one arm for its whole life.
async fn first_trial_arm(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<AutoCompactArm> {
    let arm: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'auto_compact_arm' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'auto_compact_arm' IN ('control', 'window') \
         ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[AutoCompactTrial] first arm of {}: {}", thread_id, e);
        None
    });
    match arm.as_deref() {
        Some("window") => Some(AutoCompactArm::Window),
        Some("control") => Some(AutoCompactArm::Control),
        _ => None,
    }
}

/// [`decide`] for a Claude Code spawn on `thread_id`, reading the preference,
/// the workspace env and the family root. A root that cannot be read leaves
/// the spawn out of the trial, which the analysis excludes.
pub async fn resolve(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    user_env_vars: &[(String, String)],
) -> Option<AutoCompactSetting> {
    let mode = TrialMode::parse(
        &crate::core::prefs::CODING_AGENT_AUTO_COMPACT_TRIAL
            .read(pool)
            .await,
    );
    if mode == TrialMode::Off {
        return None;
    }
    let workspace_value = user_env_vars
        .iter()
        .find(|(name, _)| name == AUTO_COMPACT_WINDOW_ENV)
        .map(|(_, value)| value.clone())
        .or_else(|| crate::core::inherited_env_var(AUTO_COMPACT_WINDOW_ENV));
    if mode == TrialMode::Ab && workspace_value.is_none() {
        if let Some(arm) = first_trial_arm(pool, thread_id).await {
            return Some(trial_setting(arm));
        }
    }
    let root = match super::thread_family::family_root(pool, thread_id).await {
        Ok(root) => root,
        Err(e) => {
            crate::log!(
                "[AutoCompactTrial] could not read the family root of {}, so the spawn sits out the trial: {}",
                thread_id,
                e
            );
            return None;
        }
    };
    decide(mode, workspace_value.as_deref(), root)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn session_started(pool: &sqlx::PgPool, thread_id: Uuid, arm: &str) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES (gen_random_uuid(), 'SessionStarted', $1, $2, 'thread', $2::text)",
        )
        .bind(serde_json::json!({ "auto_compact_arm": arm }))
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// I1: a thread keeps the arm it first ran in. An override never counts,
    /// since the engine did not choose it.
    #[tokio::test]
    async fn a_thread_keeps_the_arm_it_first_ran_in() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let thread = Uuid::new_v4();
        assert_eq!(first_trial_arm(&pool, thread).await, None);
        session_started(&pool, thread, "override").await;
        assert_eq!(first_trial_arm(&pool, thread).await, None);
        session_started(&pool, thread, "window").await;
        session_started(&pool, thread, "control").await;
        assert_eq!(
            first_trial_arm(&pool, thread).await,
            Some(AutoCompactArm::Window)
        );
        crate::test_support::teardown_test_db(&db).await;
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    /// Pinned against an independent SHA-256, so a change of hash or byte
    /// order cannot move a family to the other arm mid-trial.
    #[test]
    fn the_arm_is_a_pinned_function_of_the_root() {
        use AutoCompactArm::{Control, Window};
        let cases = [
            ("00000000-0000-0000-0000-000000000001", Control),
            ("00000000-0000-0000-0000-000000000002", Window),
            ("00000000-0000-0000-0000-000000000003", Window),
            ("6f1c2d3e-4b5a-4c6d-8e7f-90a1b2c3d4e5", Control),
        ];
        for (root, arm) in cases {
            assert_eq!(arm_for_family(uuid(root)), arm, "{root}");
        }
    }

    #[test]
    fn off_sets_nothing_and_records_nothing() {
        let root = Uuid::new_v4();
        assert_eq!(decide(TrialMode::Off, None, root), None);
        assert_eq!(decide(TrialMode::Off, Some("200000"), root), None);
    }

    #[test]
    fn ab_follows_the_root_and_on_always_sets_the_window() {
        let control = uuid("00000000-0000-0000-0000-000000000001");
        let window = uuid("00000000-0000-0000-0000-000000000002");
        let set = Some(AutoCompactSetting {
            arm: AutoCompactArm::Window,
            window: Some(TRIAL_WINDOW_TOKENS),
        });
        assert_eq!(
            decide(TrialMode::Ab, None, control),
            Some(AutoCompactSetting {
                arm: AutoCompactArm::Control,
                window: None
            })
        );
        assert_eq!(decide(TrialMode::Ab, None, window), set);
        assert_eq!(decide(TrialMode::On, None, control), set);
    }

    #[test]
    fn a_workspace_value_wins_and_is_recorded_as_an_override() {
        let root = uuid("00000000-0000-0000-0000-000000000002");
        let decided = decide(TrialMode::Ab, Some("250000"), root).unwrap();
        assert_eq!(decided.arm, AutoCompactArm::Override);
        assert_eq!(decided.window, Some(250_000));
        assert_eq!(decided.engine_window(), None, "the engine writes nothing");
        let unparsable = decide(TrialMode::On, Some("big"), root).unwrap();
        assert_eq!(unparsable.arm, AutoCompactArm::Override);
        assert_eq!(unparsable.window, None);
    }

    #[test]
    fn every_mode_round_trips_and_a_stranger_reads_as_off() {
        for mode in TrialMode::ALL {
            assert_eq!(TrialMode::parse(mode.as_str()), mode);
        }
        assert_eq!(TrialMode::parse("sometimes"), TrialMode::Off);
    }

    /// The preference text is a const string, so it cannot read the constant.
    /// This pins the number it names.
    #[test]
    fn the_preference_names_the_window_the_trial_sets() {
        let window = format!("{}k-token", TRIAL_WINDOW_TOKENS / 1000);
        assert!(crate::core::prefs::CODING_AGENT_AUTO_COMPACT_TRIAL
            .spec
            .description
            .contains(&window));
    }
}
