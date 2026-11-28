//! INVARIANT: Release notices: the one-time instructions a release hands the user.
//!
//! `release-notices.toml` at the repo root is the authored source, and it is
//! `include_str!`d here. So a running engine serves the copy it was BUILT with,
//! which is why the file joins the engine-bundled asset list in
//! `git_ops::restart_detection::files_require_restart`.
//!
//! Two rules make the sequence work, and both live here rather than in the UI.
//! A notice names the release it applies FROM, and is invisible until the
//! engine reports that version or newer. And a workspace answers them in file
//! order, tracked by a single cursor: the id of the last notice it resolved.
//! What is visible and sits after that is owed.
//!
//! A notice may also name a [`NoticeCondition`]. Where it fails, the notice is
//! invisible exactly as an unreleased one is, so nobody is told to do what
//! their workspace already does.
//!
//! A notice may also name, in `superseded_by`, a later notice that replaces
//! it. Once that one is meant for the workspace, the older one is hidden the
//! same way, so nobody reads an instruction the next notice contradicts.
//!
//! The modal and the What's New panel are two renderings of one list. So
//! [`view`] returns visible items grouped by release and the owed cursor range.

use crate::core::preferences::PreferenceStore;
use crate::core::prefs;
use crate::core::IntentStore;
use crate::engine::summary_tree::module::MemoryModule;
use crate::triggers::TriggerConfig;
use lucidos_update_channel::is_upgrade;
use semver::Version;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

/// INVARIANT: The authored notices as of this build. Read that file's own header before
/// adding one: its order is load-bearing.
const RELEASE_NOTICES_TOML: &str = include_str!("../../../../release-notices.toml");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNoticeItem {
    /// INVARIANT: Stable forever. The workspace cursor is this string.
    pub id: String,
    pub since: String,
    pub title: String,
    pub body: String,
    pub summary: String,
    #[serde(default, skip_serializing)]
    pub checks: Vec<String>,
    #[serde(default, skip_serializing)]
    pub action: Option<String>,
    #[serde(default, skip_serializing)]
    pub only_when: Option<NoticeCondition>,
    #[serde(default, skip_serializing)]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeCondition {
    OnClassicMemory,
    HasIntents,
    HasUngrantedCodingAgentTrigger,
    CommandGuardTurnsOn,
    HasLegacyTriggerGrants,
}

impl NoticeCondition {
    fn holds(self, facts: &NoticeFacts) -> bool {
        match self {
            Self::OnClassicMemory => facts.memory_module == Some(MemoryModule::Classic),
            Self::HasIntents => facts.has_intents,
            Self::HasUngrantedCodingAgentTrigger => facts.has_ungranted_coding_agent_trigger,
            Self::CommandGuardTurnsOn => facts.command_guard_turns_on,
            Self::HasLegacyTriggerGrants => facts.has_legacy_trigger_grants,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeFacts {
    pub(crate) memory_module: Option<MemoryModule>,
    pub(crate) has_intents: bool,
    pub(crate) has_ungranted_coding_agent_trigger: bool,
    pub(crate) command_guard_turns_on: bool,
    pub(crate) has_legacy_trigger_grants: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuardExposure {
    pub unapproved_mcp_server: bool,
    pub judge_unresolved: bool,
}

impl NoticeFacts {
    /// INVARIANT: An unreadable memory module reads as Classic, which shows the Tree
    /// notice: the recoverable direction. An unreadable trigger history shows
    /// the coding-agent grant notice, and an unreadable guard row the command
    /// guard notice, for the same reason.
    pub async fn read(
        pool: &PgPool,
        workspace_path: &Path,
        trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
        exposure: GuardExposure,
    ) -> Self {
        let memory_module = MemoryModule::stored(pool).await.unwrap_or_else(|e| {
            log_legacy!("[ReleaseNotices] Failed to read the memory module: {}", e);
            Some(MemoryModule::Classic)
        });
        let ungranted = ungranted_trigger_ids(trigger_configs);
        let has_ungranted_coding_agent_trigger = any_started_a_coding_agent(pool, &ungranted)
            .await
            .unwrap_or_else(|e| {
                log_legacy!(
                    "[ReleaseNotices] Failed to read trigger coding agents: {}",
                    e
                );
                !ungranted.is_empty()
            });
        let command_guard_turns_on = command_guard_never_set(pool).await
            && (has_a_trigger(trigger_configs)
                || exposure.unapproved_mcp_server
                || exposure.judge_unresolved);
        Self {
            memory_module,
            has_intents: workspace_has_intents(workspace_path),
            has_ungranted_coding_agent_trigger,
            command_guard_turns_on,
            has_legacy_trigger_grants: has_legacy_trigger_grants(trigger_configs),
        }
    }
}

pub fn has_legacy_trigger_grants(configs: &RwLock<HashMap<String, TriggerConfig>>) -> bool {
    configs
        .read()
        .unwrap()
        .values()
        .any(|config| !config.side_effect_grant.is_empty())
}

async fn command_guard_never_set(pool: &PgPool) -> bool {
    prefs::COMMAND_GUARD
        .try_stored(pool)
        .await
        .map(|row| row.is_none())
        .unwrap_or_else(|e| {
            log_legacy!("[ReleaseNotices] Failed to read the command guard: {}", e);
            true
        })
}

fn has_a_trigger(trigger_configs: &RwLock<HashMap<String, TriggerConfig>>) -> bool {
    trigger_configs
        .read()
        .map(|configs| !configs.is_empty())
        .unwrap_or(true)
}

fn workspace_has_intents(workspace_path: &Path) -> bool {
    !IntentStore::load_all(&workspace_path.join(crate::core::DATA_DIR)).is_empty()
}

fn ungranted_trigger_ids(trigger_configs: &RwLock<HashMap<String, TriggerConfig>>) -> Vec<String> {
    trigger_configs
        .read()
        .map(|configs| {
            configs
                .values()
                .filter(|c| !c.coding_agent_grant)
                .map(|c| c.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// INVARIANT: Whether a fire of one of `trigger_ids` has a coding-agent thread below it
/// in the callback tree. The spawn gate also traces top spawns, but that walk
/// reads every origin event, too slow for each page load. A trigger whose only
/// coding agents are top spawns hears from the gate's refusal notification.
async fn any_started_a_coding_agent(
    pool: &PgPool,
    trigger_ids: &[String],
) -> Result<bool, sqlx::Error> {
    if trigger_ids.is_empty() {
        return Ok(false);
    }
    sqlx::query_scalar(
        "WITH RECURSIVE tree AS ( \
           SELECT thread_id FROM thread_summaries WHERE trigger_id = ANY($1) \
           UNION \
           SELECT c.thread_id FROM thread_summaries c \
             JOIN tree t ON c.parent_thread_id = t.thread_id \
         ) \
         SELECT EXISTS ( \
           SELECT 1 FROM tree JOIN thread_summaries s USING (thread_id) \
           WHERE s.is_coding_agent \
         )",
    )
    .bind(trigger_ids)
    .fetch_one(pool)
    .await
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoticeFile {
    #[serde(default)]
    item: Vec<ReleaseNoticeItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeItemState {
    #[serde(flatten)]
    pub notice: ReleaseNoticeItem,
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseNotice {
    pub since: String,
    pub items: Vec<NoticeItemState>,
    pub resolved: bool,
    pub action_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeState {
    #[serde(flatten)]
    pub item: NoticeItemState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_label: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_prompt: Option<String>,
    pub action_deferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeView {
    pub releases: Vec<ReleaseNotice>,
    pub owed_through: Option<String>,
    pub action_prompt: Option<String>,
    // Compatibility for older desktop frontends (docs/temporary-measures.md).
    pub notices: Vec<NoticeState>,
    pub next_id: Option<String>,
}

pub fn parse(src: &str) -> Result<Vec<ReleaseNoticeItem>, String> {
    let file: NoticeFile = toml::from_str(src).map_err(|e| format!("not valid TOML: {e}"))?;
    let mut previous: Option<Version> = None;
    for (i, notice) in file.item.iter().enumerate() {
        let at = format!("notice {} ({:?})", i + 1, notice.id);
        if notice.id.trim().is_empty() {
            return Err(format!("{at} has an empty id"));
        }
        if file.item[..i].iter().any(|n| n.id == notice.id) {
            return Err(format!("{at} reuses an id an earlier notice already has"));
        }
        if notice.title.trim().is_empty() || notice.body.trim().is_empty() {
            return Err(format!("{at} has an empty title or body"));
        }
        if !single_line(&notice.summary) || notice.summary.chars().count() > 100 {
            return Err(format!(
                "{at} needs a one-line summary of at most 100 characters"
            ));
        }
        if notice.checks.iter().any(|check| {
            !single_line(check)
                || !check.split_once(": ").is_some_and(|(section, name)| {
                    !section.is_empty()
                        && section.split('-').all(|word| {
                            !word.is_empty() && word.bytes().all(|c| c.is_ascii_lowercase())
                        })
                        && !name.trim().is_empty()
                })
        }) {
            return Err(format!(
                "{at} needs checks in the form \"section: check name\""
            ));
        }
        if notice
            .action
            .as_deref()
            .is_some_and(|action| !single_line(action))
        {
            return Err(format!("{at} needs a one-line action"));
        }
        let since = Version::parse(&notice.since).map_err(|e| {
            format!(
                "{at} names since = {:?}, which is not semver: {e}",
                notice.since
            )
        })?;
        if previous
            .as_ref()
            .is_some_and(|p| is_upgrade(&since.to_string(), &p.to_string()))
        {
            return Err(format!(
                "{at} is older than the notice above it; append, never reorder"
            ));
        }
        previous = Some(since);
        if let Some(by) = &notice.superseded_by {
            let Some(replacement) = file.item[i + 1..].iter().find(|n| &n.id == by) else {
                return Err(format!(
                    "{at} names superseded_by = {by:?}, which is not a later notice"
                ));
            };
            if notice
                .action
                .as_ref()
                .is_some_and(|action| replacement.action.as_ref() != Some(action))
                || !notice
                    .checks
                    .iter()
                    .all(|check| replacement.checks.contains(check))
            {
                return Err(format!(
                    "{at} has an action that {by:?}, which supersedes it, does not carry"
                ));
            }
        }
    }
    Ok(file.item)
}

fn single_line(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

pub fn all() -> &'static [ReleaseNoticeItem] {
    static NOTICES: OnceLock<Vec<ReleaseNoticeItem>> = OnceLock::new();
    NOTICES.get_or_init(|| match parse(RELEASE_NOTICES_TOML) {
        Ok(notices) => notices,
        Err(e) => {
            crate::log_legacy!("[ReleaseNotices] release-notices.toml is not usable: {e}");
            Vec::new()
        }
    })
}

pub fn running_release() -> Result<Version, String> {
    reported_release(
        crate::build_identity().release,
        crate::build_identity().release_dirty,
    )
}

/// INVARIANT: The release an engine on `release`, built `dirty`, reports for this purpose.
///
/// A release-dirty stable build counts as the NEXT PATCH. Such a build is past the
/// published snapshot by definition, so a notice written for the release it is
/// becoming already applies to it. That is the only way an author reads their
/// own notice on screen. A pre-release keeps its precedence, even when dirty,
/// so it never exposes a notice for its future stable release.
///
/// It stays off for everyone else. `build.rs` sets the flag false whenever git
/// is unavailable, so a shipped install never takes this arm. And a notice
/// deliberately floored further out stays held back, because one patch is all
/// this adds.
pub(crate) fn reported_release(release: &str, dirty: bool) -> Result<Version, String> {
    let mut version =
        Version::parse(release).map_err(|e| format!("{release} is not semver: {e}"))?;
    if dirty && version.pre.is_empty() {
        version.patch += 1;
    }
    Ok(version)
}

fn applies_to(notice: &ReleaseNoticeItem, running: &Version) -> bool {
    Version::parse(&notice.since)
        .is_ok_and(|since| !is_upgrade(&running.to_string(), &since.to_string()))
}

fn meant_for(notice: &ReleaseNoticeItem, running: &Version, facts: &NoticeFacts) -> bool {
    applies_to(notice, running) && notice.only_when.is_none_or(|c| c.holds(facts))
}

fn shown(
    notice: &ReleaseNoticeItem,
    notices: &[ReleaseNoticeItem],
    running: &Version,
    facts: &NoticeFacts,
) -> bool {
    meant_for(notice, running, facts) && !superseded(notice, notices, running, facts)
}

/// INVARIANT: Is any notice down `notice`'s `superseded_by` chain meant for this
/// workspace? A link whose condition fails still passes the chain on, so a
/// replacement two steps later hides it too. [`parse`] only accepts later
/// ids, so the walk ends.
fn superseded(
    notice: &ReleaseNoticeItem,
    notices: &[ReleaseNoticeItem],
    running: &Version,
    facts: &NoticeFacts,
) -> bool {
    let mut by = notice.superseded_by.as_deref();
    while let Some(id) = by {
        let Some(replacement) = notices.iter().find(|n| n.id == id) else {
            return false;
        };
        if meant_for(replacement, running, facts) {
            return true;
        }
        by = replacement.superseded_by.as_deref();
    }
    false
}

fn answered_count(notices: &[ReleaseNoticeItem], cursor: Option<&str>) -> usize {
    cursor
        .and_then(|id| notices.iter().position(|n| n.id == id))
        .map_or(0, |i| i + 1)
}

fn owed<'a>(
    notices: &'a [ReleaseNoticeItem],
    running: &'a Version,
    facts: &'a NoticeFacts,
    cursor: Option<&str>,
) -> impl Iterator<Item = &'a ReleaseNoticeItem> {
    notices
        .iter()
        .skip(answered_count(notices, cursor))
        .filter(move |n| shown(n, notices, running, facts))
}

fn action_prompt<'a>(items: impl IntoIterator<Item = &'a ReleaseNoticeItem>) -> Option<String> {
    let mut checks = Vec::new();
    let mut actions = Vec::new();
    for item in items {
        for check in &item.checks {
            if !checks.contains(&check.as_str()) {
                checks.push(check.as_str());
            }
        }
        if let Some(action) = item
            .action
            .as_deref()
            .filter(|action| !actions.contains(action))
        {
            actions.push(action);
        }
    }
    let mut paragraphs = Vec::new();
    if !checks.is_empty() {
        paragraphs.push(format!(
            "Run these workspace audit checks:\n- {}",
            checks.join("\n- ")
        ));
    }
    paragraphs.extend(actions.into_iter().map(str::to_owned));
    (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"))
}

pub fn view(
    notices: &[ReleaseNoticeItem],
    running: &Version,
    facts: &NoticeFacts,
    cursor: Option<&str>,
) -> NoticeView {
    let answered = answered_count(notices, cursor);
    let visible: Vec<_> = notices
        .iter()
        .enumerate()
        .filter(|(_, n)| shown(n, notices, running, facts))
        .map(|(i, n)| (n, i < answered))
        .collect();
    let outstanding: Vec<_> = owed(notices, running, facts, cursor).collect();

    let releases = visible
        .chunk_by(|(a, _), (b, _)| a.since == b.since)
        .map(|group| {
            let resolved = group.iter().all(|(_, resolved)| *resolved);
            ReleaseNotice {
                since: group[0].0.since.clone(),
                items: group
                    .iter()
                    .map(|&(notice, resolved)| NoticeItemState {
                        notice: notice.clone(),
                        resolved,
                    })
                    .collect(),
                resolved,
                action_prompt: action_prompt(
                    group
                        .iter()
                        .filter(|(_, item_resolved)| resolved || !item_resolved)
                        .map(|(notice, _)| *notice),
                ),
            }
        })
        .collect();

    let legacy = visible
        .iter()
        .enumerate()
        .map(|(i, &(notice, resolved))| {
            let prompt = action_prompt([notice]);
            NoticeState {
                item: NoticeItemState {
                    notice: notice.clone(),
                    resolved,
                },
                action_label: prompt.as_ref().map(|_| "Check my workspace"),
                action_deferred: !resolved
                    && prompt.is_some()
                    && visible[i + 1..].iter().any(|&(later, _)| {
                        later.checks == notice.checks && later.action == notice.action
                    }),
                action_prompt: prompt,
            }
        })
        .collect();

    NoticeView {
        releases,
        owed_through: outstanding.last().map(|n| n.id.clone()),
        action_prompt: action_prompt(outstanding.iter().copied()),
        notices: legacy,
        next_id: outstanding.first().map(|n| n.id.clone()),
    }
}

/// INVARIANT: The stamp a workspace with no content starts at, or `None` when the running
/// release has reached no notice yet.
///
/// A fresh workspace has nothing to audit and no settings to migrate, so it
/// starts level and hears from the NEXT release instead. Only it is stamped:
/// [`place_workspace`] owns the other half of that decision.
///
/// Conditions and supersession are ignored. A fresh workspace picks its memory module in the
/// memory choice, and must not meet the Tree notice the day it picks Classic.
fn fresh_workspace_stamp(notices: &[ReleaseNoticeItem], running: &Version) -> Option<String> {
    notices
        .iter()
        .rev()
        .find(|n| applies_to(n, running))
        .map(|n| n.id.clone())
}

pub(crate) fn advanced_cursor(
    notices: &[ReleaseNoticeItem],
    running: &Version,
    facts: &NoticeFacts,
    cursor: Option<&str>,
    id: &str,
    shown_ids: Option<&[String]>,
) -> Option<String> {
    let mut outstanding = owed(notices, running, facts, cursor).map(|item| item.id.as_str());
    let matches = match shown_ids {
        Some(ids) => {
            ids.last().is_some_and(|last| last == id)
                && outstanding.eq(ids.iter().map(String::as_str))
        }
        None => outstanding.next() == Some(id),
    };
    matches.then(|| id.to_owned())
}

pub async fn stored_cursor(pool: &PgPool) -> Option<String> {
    prefs::RELEASE_NOTICE_CURSOR.stored(pool).await
}

pub async fn place_workspace(pool: &PgPool, notices: &[ReleaseNoticeItem], running: &Version) {
    if notices.is_empty() {
        return;
    }
    let stored = stored_cursor(pool).await;
    if stored.iter().any(|id| notices.iter().any(|n| &n.id == id)) {
        return;
    }
    let has_threads = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE NOT is_home)",
    )
    .fetch_one(pool)
    .await
    {
        Ok(has_threads) => has_threads,
        Err(e) => {
            // INVARIANT: Placing on a guess would either stamp a real workspace silent
            // or open a modal over a first run. Skip, and let the next boot
            // ask again.
            crate::log_legacy!(
                "[ReleaseNotices] could not tell whether the workspace has threads: {e}"
            );
            return;
        }
    };
    if has_threads {
        return;
    }
    let Some(stamp) = fresh_workspace_stamp(notices, running) else {
        return;
    };
    match PreferenceStore::set_silent(pool, prefs::RELEASE_NOTICE_CURSOR.key(), &stamp).await {
        Ok(()) => crate::log_legacy!("[ReleaseNotices] stamped a fresh workspace at {stamp}"),
        Err(e) => crate::log_legacy!("[ReleaseNotices] could not stamp the workspace: {e}"),
    }
}

/// INVARIANT: Record that the workspace has answered `id`, and say whether it moved.
///
/// Checks the owed sequence with [`advanced_cursor`], so
/// the caller announces only a real change.
///
/// **Reads the cursor fallibly, unlike [`stored_cursor`].** That helper answers
/// a display with `None` on a failed read, which is right for a list and wrong
/// here: `None` reads as "nothing answered", so the first visible notice looks
/// owed and the write would rewind the cursor onto it. A transient read failure
/// would then re-open every notice after it. A write that cannot see the
/// current position must not guess one.
pub async fn resolve(
    pool: &PgPool,
    notices: &[ReleaseNoticeItem],
    running: &Version,
    facts: &NoticeFacts,
    id: &str,
    shown_ids: Option<&[String]>,
) -> Result<bool, sqlx::Error> {
    let cursor = prefs::RELEASE_NOTICE_CURSOR.try_stored(pool).await?;
    let Some(next) = advanced_cursor(notices, running, facts, cursor.as_deref(), id, shown_ids)
    else {
        return Ok(false);
    };
    PreferenceStore::compare_set_silent(
        pool,
        prefs::RELEASE_NOTICE_CURSOR.key(),
        cursor.as_deref(),
        &next,
    )
    .await
}

#[cfg(test)]
#[path = "release_notices_upgrade_pins_tests.rs"]
mod upgrade_pins;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    pub(super) fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn a_dirty_build_reports_the_release_it_is_becoming() {
        assert_eq!(reported_release("0.38.2", true).unwrap(), v("0.38.3"));
    }

    #[test]
    fn a_clean_build_reports_the_release_it_is_on() {
        assert_eq!(reported_release("0.38.2", false).unwrap(), v("0.38.2"));
    }

    #[test]
    fn a_dirty_build_does_not_reach_a_notice_floored_further_out() {
        let reported = reported_release("0.38.2", true).unwrap();
        let next_minor = v(&format!("{}.{}.0", reported.major, reported.minor + 1));
        assert!(next_minor > reported);
    }

    #[test]
    fn a_release_that_is_not_semver_is_an_error_either_way() {
        assert!(reported_release("0.38", false).is_err());
        assert!(reported_release("0.38", true).is_err());
    }

    #[test]
    fn prereleases_do_not_expose_their_future_stable_notices_even_when_dirty() {
        let notices = vec![notice("previous", "9.1.0"), notice("future", "9.2.0")];
        for dirty in [false, true] {
            let running = reported_release("9.2.0-nightly.20261116.1", dirty).unwrap();
            let shown = view(&notices, &running, &CLASSIC, None);
            assert_eq!(ids(&shown), ["previous"]);
            assert_eq!(
                fresh_workspace_stamp(&notices, &running),
                Some("previous".to_string())
            );
        }
    }

    #[test]
    fn notice_eligibility_ignores_build_metadata() {
        assert!(applies_to(&notice("a", "1.0.0+z"), &v("1.0.0+a")));
    }

    #[tokio::test]
    async fn a_fresh_workspace_is_stamped_at_the_release_the_surfaces_report() {
        let (pool, db_name) = setup_test_db().await;
        let running = running_release().expect("the shipped RELEASE is semver");
        let notices = vec![notice("a", "1.0.0"), notice("b", &running.to_string())];
        place_workspace(&pool, &notices, &running).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        assert!(view(&notices, &running, &CLASSIC, Some("b"))
            .next_id
            .is_none());
        teardown_test_db(&db_name).await;
    }

    fn notice(id: &str, since: &str) -> ReleaseNoticeItem {
        ReleaseNoticeItem {
            id: id.to_string(),
            since: since.to_string(),
            title: format!("Notice {id}"),
            body: "Do the thing.".to_string(),
            summary: "Nothing to do.".to_string(),
            checks: Vec::new(),
            action: None,
            only_when: None,
            superseded_by: None,
        }
    }

    pub(super) const CLASSIC: NoticeFacts = NoticeFacts {
        memory_module: Some(MemoryModule::Classic),
        has_intents: true,
        has_ungranted_coding_agent_trigger: true,
        command_guard_turns_on: true,
        has_legacy_trigger_grants: false,
    };
    pub(super) const TREE: NoticeFacts = NoticeFacts {
        memory_module: Some(MemoryModule::Tree),
        ..CLASSIC
    };
    const NEVER_CHOSE: NoticeFacts = NoticeFacts {
        memory_module: None,
        ..TREE
    };
    const NO_INTENTS: NoticeFacts = NoticeFacts {
        has_intents: false,
        ..TREE
    };
    const ALL_TRIGGERS_GRANTED: NoticeFacts = NoticeFacts {
        has_ungranted_coding_agent_trigger: false,
        ..TREE
    };
    const GUARD_UNAFFECTED: NoticeFacts = NoticeFacts {
        command_guard_turns_on: false,
        ..TREE
    };

    fn b_only_on_classic() -> Vec<ReleaseNoticeItem> {
        let mut notices = three();
        notices[1].only_when = Some(NoticeCondition::OnClassicMemory);
        notices
    }

    #[test]
    fn a_notice_whose_condition_fails_is_neither_listed_nor_owed() {
        let view = view(&b_only_on_classic(), &v("3.0.0"), &TREE, Some("a"));
        assert_eq!(ids(&view), ["a", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn a_workspace_that_never_chose_is_not_owed_the_classic_notice() {
        let view = view(&b_only_on_classic(), &v("3.0.0"), &NEVER_CHOSE, Some("a"));
        assert_eq!(ids(&view), ["a", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn a_notice_whose_condition_holds_shows_as_usual() {
        let view = view(&b_only_on_classic(), &v("3.0.0"), &CLASSIC, Some("a"));
        assert_eq!(ids(&view), ["a", "b", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("b"));
    }

    #[test]
    fn a_hidden_notice_never_blocks_the_ones_after_it() {
        let notices = b_only_on_classic();
        let after_a = advanced_cursor(&notices, &v("3.0.0"), &TREE, Some("a"), "c", None);
        assert_eq!(after_a.as_deref(), Some("c"));
        assert_eq!(
            view(&notices, &v("3.0.0"), &TREE, after_a.as_deref()).next_id,
            None
        );
    }

    #[test]
    fn a_hidden_notice_cannot_be_answered() {
        let notices = b_only_on_classic();
        assert_eq!(
            advanced_cursor(&notices, &v("3.0.0"), &TREE, Some("a"), "b", None),
            None
        );
    }

    #[test]
    fn the_fresh_workspace_stamp_covers_a_conditional_notice() {
        let mut notices = three();
        notices[2].only_when = Some(NoticeCondition::OnClassicMemory);
        assert_eq!(
            fresh_workspace_stamp(&notices, &v("3.0.0")).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn a_condition_parses_from_its_kebab_case_name() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                   only_when = \"on-classic-memory\"\n";
        let parsed = parse(src).unwrap();
        assert_eq!(parsed[0].only_when, Some(NoticeCondition::OnClassicMemory));
    }

    #[test]
    fn legacy_trigger_grant_notice_condition_parses() {
        let source = include_str!("../../../../release-notices.toml");
        let mut notices = parse(source).unwrap();
        let pair_end = notices
            .iter()
            .position(|n| n.id == "trigger-category-grants-ended")
            .expect("the notice is in the file");
        notices.truncate(pair_end + 1);
        let version = Version::parse(&notices[pair_end].since).unwrap();
        let prior = Some("trigger-program-grants");
        let unaffected = view(&notices, &version, &CLASSIC, prior);
        assert_eq!(unaffected.next_id, None);
        assert!(!unaffected.notices.iter().any(|n| n
            .item
            .notice
            .id
            .starts_with("trigger-category-grants")));
        let affected = NoticeFacts {
            has_legacy_trigger_grants: true,
            ..CLASSIC
        };
        let notices = view(&notices, &version, &affected, prior);
        assert_eq!(
            notices.next_id.as_deref(),
            Some("trigger-category-grants-ended")
        );
        let row = notices.notices.last().unwrap();
        let notice = &row.item.notice;
        assert_eq!(row.action_label, Some("Check my workspace"));
        assert_eq!(
            row.action_prompt.as_deref(),
            Some("Help me review my trigger grants.")
        );
        assert!(!notice.body.contains("thread:"));
    }

    #[test]
    fn an_unknown_condition_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                   only_when = \"on-clasic-memory\"\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn a_condition_stays_off_the_wire() {
        let mut notice = notice("a", "1.0.0");
        notice.only_when = Some(NoticeCondition::OnClassicMemory);
        let json = serde_json::to_value(&notice).unwrap();
        assert!(json.get("only_when").is_none(), "{json}");
    }

    #[test]
    fn the_shipped_tree_notice_shows_only_on_classic() {
        let notices = parse(RELEASE_NOTICES_TOML).unwrap();
        let tree = notices
            .iter()
            .find(|n| n.id == "move-to-tree-memory")
            .unwrap();
        assert_eq!(tree.only_when, Some(NoticeCondition::OnClassicMemory));
    }

    fn b_only_with_intents() -> Vec<ReleaseNoticeItem> {
        let mut notices = three();
        notices[1].only_when = Some(NoticeCondition::HasIntents);
        notices
    }

    #[test]
    fn the_has_intents_notice_shows_to_a_workspace_with_an_intent() {
        let view = view(&b_only_with_intents(), &v("3.0.0"), &TREE, Some("a"));
        assert_eq!(ids(&view), ["a", "b", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("b"));
    }

    #[test]
    fn the_has_intents_notice_hides_from_a_workspace_without_one() {
        let view = view(&b_only_with_intents(), &v("3.0.0"), &NO_INTENTS, Some("a"));
        assert_eq!(ids(&view), ["a", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn has_intents_parses_from_its_kebab_case_name() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                   only_when = \"has-intents\"\n";
        assert_eq!(
            parse(src).unwrap()[0].only_when,
            Some(NoticeCondition::HasIntents)
        );
    }

    #[test]
    fn a_workspace_has_intents_once_the_registry_finds_one() {
        let ws = tempfile::tempdir().unwrap();
        assert!(!workspace_has_intents(ws.path()));
        let triggers = ws
            .path()
            .join(crate::core::DATA_DIR)
            .join("triggers/digest");
        std::fs::create_dir_all(&triggers).unwrap();
        std::fs::write(
            triggers.join("digest.md"),
            "---\nname: Digest\n---\nSum up.\n",
        )
        .unwrap();
        assert!(workspace_has_intents(ws.path()));
    }

    #[test]
    fn the_shipped_intents_notice_shows_only_with_intents() {
        let notices = parse(RELEASE_NOTICES_TOML).unwrap();
        let intents = notices
            .iter()
            .find(|n| n.id == "intents-run-in-the-thread")
            .unwrap();
        assert_eq!(intents.only_when, Some(NoticeCondition::HasIntents));
    }

    fn b_only_with_an_ungranted_trigger() -> Vec<ReleaseNoticeItem> {
        let mut notices = three();
        notices[1].only_when = Some(NoticeCondition::HasUngrantedCodingAgentTrigger);
        notices
    }

    #[test]
    fn the_coding_agent_grant_notice_shows_where_a_trigger_lacks_it() {
        let view = view(
            &b_only_with_an_ungranted_trigger(),
            &v("3.0.0"),
            &TREE,
            Some("a"),
        );
        assert_eq!(ids(&view), ["a", "b", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("b"));
    }

    #[test]
    fn the_coding_agent_grant_notice_hides_once_every_trigger_holds_it() {
        let view = view(
            &b_only_with_an_ungranted_trigger(),
            &v("3.0.0"),
            &ALL_TRIGGERS_GRANTED,
            Some("a"),
        );
        assert_eq!(ids(&view), ["a", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn has_ungranted_coding_agent_trigger_parses_from_its_kebab_case_name() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                   only_when = \"has-ungranted-coding-agent-trigger\"\n";
        assert_eq!(
            parse(src).unwrap()[0].only_when,
            Some(NoticeCondition::HasUngrantedCodingAgentTrigger)
        );
    }

    #[test]
    fn the_shipped_coding_agent_grant_notice_shows_only_where_a_trigger_lacks_it() {
        let notices = parse(RELEASE_NOTICES_TOML).unwrap();
        let grant = notices
            .iter()
            .find(|n| n.id == "triggers-need-the-coding-agent-grant")
            .unwrap();
        assert_eq!(
            grant.only_when,
            Some(NoticeCondition::HasUngrantedCodingAgentTrigger)
        );
    }

    #[test]
    fn the_command_guard_notice_hides_where_the_flip_changes_nothing() {
        let mut notices = three();
        notices[1].only_when = Some(NoticeCondition::CommandGuardTurnsOn);
        let shown = view(&notices, &v("3.0.0"), &TREE, Some("a"));
        assert_eq!(shown.next_id.as_deref(), Some("b"));
        let hidden = view(&notices, &v("3.0.0"), &GUARD_UNAFFECTED, Some("a"));
        assert_eq!(ids(&hidden), ["a", "c"]);
    }

    #[test]
    fn the_shipped_command_guard_notice_is_conditional_and_targeted() {
        let notices = parse(RELEASE_NOTICES_TOML).unwrap();
        let guard = notices
            .iter()
            .find(|n| n.id == "command-guard-on-by-default")
            .unwrap();
        assert_eq!(guard.only_when, Some(NoticeCondition::CommandGuardTurnsOn));
        let prompt = action_prompt([guard]).unwrap();
        assert!(prompt.contains("mcp-servers: command guard"), "{prompt}");
        for link in [
            "(settings:permissions#command-safety)",
            "(settings:models#command-judge)",
            "(settings:permissions#coding-agent-auto-answer)",
        ] {
            assert!(guard.body.contains(link), "the notice links {link}");
        }
    }

    fn c_supersedes_a() -> Vec<ReleaseNoticeItem> {
        let mut notices = three();
        notices[0].superseded_by = Some("c".to_string());
        notices
    }

    #[test]
    fn a_superseded_notice_is_neither_listed_nor_owed() {
        let view = view(&c_supersedes_a(), &v("3.0.0"), &TREE, None);
        assert_eq!(ids(&view), ["b", "c"]);
        assert_eq!(view.next_id.as_deref(), Some("b"));
    }

    #[test]
    fn a_superseded_notice_cannot_be_answered() {
        let notices = c_supersedes_a();
        assert_eq!(
            advanced_cursor(&notices, &v("3.0.0"), &TREE, None, "a", None),
            None
        );
    }

    #[test]
    fn a_superseder_not_yet_reached_hides_nothing() {
        let view = view(&c_supersedes_a(), &v("2.0.0"), &TREE, None);
        assert_eq!(ids(&view), ["a", "b"]);
        assert_eq!(view.next_id.as_deref(), Some("a"));
    }

    #[test]
    fn a_superseder_whose_condition_fails_hides_nothing() {
        let mut notices = c_supersedes_a();
        notices[2].only_when = Some(NoticeCondition::OnClassicMemory);
        assert_eq!(ids(&view(&notices, &v("3.0.0"), &TREE, None)), ["a", "b"]);
        assert_eq!(
            ids(&view(&notices, &v("3.0.0"), &CLASSIC, None)),
            ["b", "c"]
        );
    }

    #[test]
    fn supersession_passes_through_a_link_whose_condition_fails() {
        let mut notices = c_supersedes_a();
        notices[0].superseded_by = Some("b".to_string());
        notices[1].superseded_by = Some("c".to_string());
        notices[1].only_when = Some(NoticeCondition::OnClassicMemory);
        assert_eq!(ids(&view(&notices, &v("3.0.0"), &TREE, None)), ["c"]);
        assert_eq!(ids(&view(&notices, &v("2.0.0"), &TREE, None)), ["a"]);
    }

    #[test]
    fn an_answered_superseded_notice_leaves_the_page_too() {
        let view = view(&c_supersedes_a(), &v("3.0.0"), &TREE, Some("b"));
        assert_eq!(ids(&view), ["b", "c"]);
        assert!(view.notices[0].item.resolved);
    }

    #[test]
    fn a_cursor_on_a_superseded_notice_keeps_its_place() {
        let view = view(&c_supersedes_a(), &v("3.0.0"), &TREE, Some("a"));
        assert_eq!(view.next_id.as_deref(), Some("b"));
    }

    fn superseded_src(superseded_by: &str) -> String {
        format!(
            "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
             superseded_by = \"{superseded_by}\"\n\
             [[item]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n"
        )
    }

    #[test]
    fn superseded_by_parses_when_it_names_a_later_notice() {
        let parsed = parse(&superseded_src("b")).unwrap();
        assert_eq!(parsed[0].superseded_by.as_deref(), Some("b"));
    }

    #[test]
    fn superseded_by_must_name_a_later_notice() {
        assert!(parse(&superseded_src("a")).is_err(), "itself");
        assert!(parse(&superseded_src("z")).is_err(), "an unknown id");
        let earlier = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                       [[item]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                       superseded_by = \"a\"\n";
        assert!(parse(earlier).is_err(), "an earlier notice");
    }

    fn superseded_action_src(replacement_action: &str) -> String {
        format!(
            "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
             superseded_by = \"b\"\naction = \"Audit it.\"\n\
             [[item]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
             {replacement_action}"
        )
    }

    #[test]
    fn a_superseder_must_carry_the_replaced_action() {
        assert!(parse(&superseded_action_src("")).is_err(), "no action");
        let other = "action = \"Fix it.\"\n";
        assert!(parse(&superseded_action_src(other)).is_err(), "another one");
        let same = "action = \"Audit it.\"\n";
        assert!(parse(&superseded_action_src(same)).is_ok());
    }

    #[test]
    fn superseded_by_stays_off_the_wire() {
        let notices = c_supersedes_a();
        let json = serde_json::to_value(&notices[0]).unwrap();
        assert!(json.get("superseded_by").is_none(), "{json}");
    }

    fn three() -> Vec<ReleaseNoticeItem> {
        vec![
            notice("a", "1.0.0"),
            notice("b", "2.0.0"),
            notice("c", "3.0.0"),
        ]
    }

    fn ids(view: &NoticeView) -> Vec<&str> {
        view.notices
            .iter()
            .map(|s| s.item.notice.id.as_str())
            .collect()
    }

    #[test]
    fn a_file_with_no_entries_is_valid_and_empty() {
        assert!(parse("# nothing yet\n").unwrap().is_empty());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn entries_keep_their_file_order() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\n\
                   [[item]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\nsummary = \"Nothing to do.\"\n";
        let parsed = parse(src).unwrap();
        assert_eq!(
            parsed.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn a_reused_id_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\n\
                   [[item]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\nsummary = \"Nothing to do.\"\n";
        assert!(parse(src).unwrap_err().contains("reuses an id"));
    }

    #[test]
    fn an_entry_older_than_the_one_above_it_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\n\
                   [[item]]\nid = \"b\"\nsince = \"1.0.0\"\ntitle = \"B\"\nbody = \"y\"\nsummary = \"Nothing to do.\"\n";
        assert!(parse(src).unwrap_err().contains("append, never reorder"));
    }

    #[test]
    fn two_notices_in_one_release_are_allowed() {
        let src = "[[item]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\n\
                   [[item]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\nsummary = \"Nothing to do.\"\n";
        assert_eq!(parse(src).unwrap().len(), 2);
    }

    #[test]
    fn an_empty_action_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n\
                   action = \" \"\n";
        assert!(parse(src).unwrap_err().contains("one-line action"));
    }

    #[test]
    fn a_release_that_is_not_semver_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"v0.30\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Nothing to do.\"\n";
        assert!(parse(src).unwrap_err().contains("not semver"));
    }

    #[test]
    fn an_empty_body_is_refused() {
        let src = "[[item]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"  \"\nsummary = \"Nothing to do.\"\n";
        assert!(parse(src).unwrap_err().contains("empty title or body"));
    }

    #[test]
    fn the_shipped_file_validates() {
        parse(RELEASE_NOTICES_TOML).expect("release-notices.toml must validate");
    }

    #[test]
    fn published_item_ids_keep_their_cursor_positions() {
        let pinned = [
            "run-a-workspace-audit",
            "deprecated-spawn-aliases-removed",
            "apps-run-isolated",
            "thread-summary-drops-coding-agent-applying",
            "cli-misrouted-looks-and-config",
            "spawn-thread-cc-model-removed",
            "every-workspace-has-home",
            "mcp-servers-reach-only-their-threads",
            "home-board",
            "move-to-tree-memory",
            "workspace-database-handle",
            "two-dashboards",
            "intents-run-in-the-thread",
            "widgets-pin-to-thread-removed",
            "triggers-need-the-coding-agent-grant",
            "command-guard-on-by-default",
            "agents-run-in-the-sandbox",
            "trigger-program-grants",
            "trigger-category-grants-ended",
            "agent-database-writes-closed",
        ];
        let items = parse(RELEASE_NOTICES_TOML).unwrap();
        assert_eq!(
            items
                .iter()
                .take(pinned.len())
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            pinned,
        );
    }

    #[test]
    fn a_summary_is_required_and_must_fit_on_one_line() {
        let base = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\n";
        assert!(parse(base).is_err());
        for summary in [" ".to_owned(), "one\ntwo".to_owned(), "é".repeat(101)] {
            let value = serde_json::to_string(&summary).unwrap();
            assert!(parse(&format!("{base}summary = {value}"))
                .unwrap_err()
                .contains("one-line summary"));
        }
        let summary = "é".repeat(100);
        assert!(parse(&format!("{base}summary = \"{summary}\"")).is_ok());
    }

    #[test]
    fn malformed_check_names_are_refused() {
        let base = "[[item]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\nsummary = \"Review it.\"\n";
        for check in ["unqualified", "cross-cutting: ", "../scripts: SQL write"] {
            assert!(parse(&format!("{base}checks = [\"{check}\"]"))
                .unwrap_err()
                .contains("section: check name"));
        }
    }

    #[test]
    fn grouping_omits_hidden_releases_and_combines_only_shown_requests() {
        let mut a = with_action("a", "1.0.0", "Audit my workspace for drift.");
        a.checks.push("apps: dashboard rename".to_owned());
        let mut b = notice("b", "2.0.0");
        b.only_when = Some(NoticeCondition::OnClassicMemory);
        b.checks.push("scripts: SQL write".to_owned());
        let c = ReleaseNoticeItem {
            id: "c".to_owned(),
            ..a.clone()
        };
        let d = notice("d", "3.0.0");
        let view = view(&[a, c, b, d], &v("3.0.0"), &TREE, None);
        assert_eq!(
            view.releases
                .iter()
                .map(|r| r.since.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0", "3.0.0"]
        );
        assert_eq!(view.releases[0].items.len(), 2);
        assert_eq!(view.releases[1].action_prompt, None);
        assert_eq!(view.action_prompt.as_deref(), Some(
            "Run these workspace audit checks:\n- apps: dashboard rename\n\nAudit my workspace for drift."
        ));
    }

    #[test]
    fn a_group_answer_requires_the_exact_owed_snapshot() {
        let notices = b_only_on_classic();
        let shown = ["a".to_owned(), "c".to_owned()];
        let running = v("3.0.0");
        let answer = |notices: &[ReleaseNoticeItem], facts: &NoticeFacts, ids: &[String]| {
            advanced_cursor(notices, &running, facts, None, "c", Some(ids))
        };
        assert_eq!(answer(&notices, &TREE, &shown).as_deref(), Some("c"));
        assert_eq!(answer(&notices, &CLASSIC, &shown), None);
        assert_eq!(
            answer(&notices, &TREE, &["a".into(), "b".into(), "c".into()]),
            None
        );
        let mut appended = notices;
        appended.push(notice("d", "3.0.0"));
        assert_eq!(answer(&appended, &TREE, &shown), None);
    }

    #[test]
    fn a_notice_newer_than_the_running_release_is_invisible() {
        let view = view(&three(), &v("2.0.0"), &CLASSIC, None);
        assert_eq!(ids(&view), ["a", "b"]);
        assert_eq!(view.next_id.as_deref(), Some("a"));
    }

    #[test]
    fn the_oldest_unresolved_notice_is_the_one_owed() {
        let view = view(&three(), &v("3.0.0"), &CLASSIC, Some("a"));
        assert_eq!(view.next_id.as_deref(), Some("b"));
        let resolved: Vec<bool> = view.notices.iter().map(|s| s.item.resolved).collect();
        assert_eq!(resolved, [true, false, false]);
    }

    #[test]
    fn answering_the_last_notice_empties_the_modal_but_not_the_panel() {
        let view = view(&three(), &v("3.0.0"), &CLASSIC, Some("c"));
        assert_eq!(view.next_id, None);
        assert_eq!(ids(&view), ["a", "b", "c"]);
        assert!(view.notices.iter().all(|s| s.item.resolved));
    }

    fn with_action(id: &str, since: &str, prompt: &str) -> ReleaseNoticeItem {
        ReleaseNoticeItem {
            action: Some(prompt.to_string()),
            ..notice(id, since)
        }
    }

    fn deferred(view: &NoticeView) -> Vec<bool> {
        view.notices.iter().map(|s| s.action_deferred).collect()
    }

    #[test]
    fn owed_notices_sharing_an_action_draw_it_once_on_the_last() {
        let notices = vec![
            with_action("a", "1.0.0", "Audit my workspace for drift."),
            notice("b", "2.0.0"),
            with_action("c", "3.0.0", "Audit my workspace for drift."),
        ];
        let view = view(&notices, &v("3.0.0"), &CLASSIC, None);
        assert_eq!(deferred(&view), [true, false, false]);
    }

    #[test]
    fn a_different_action_is_not_deferred() {
        let notices = vec![
            with_action("a", "1.0.0", "Audit my workspace for drift."),
            with_action("b", "2.0.0", "Set up backups."),
        ];
        assert_eq!(
            deferred(&view(&notices, &v("2.0.0"), &CLASSIC, None)),
            [false, false]
        );
    }

    #[test]
    fn an_action_is_not_deferred_to_a_notice_not_yet_visible() {
        let notices = vec![
            with_action("a", "1.0.0", "Audit my workspace for drift."),
            with_action("b", "2.0.0", "Audit my workspace for drift."),
        ];
        assert_eq!(
            deferred(&view(&notices, &v("1.0.0"), &CLASSIC, None)),
            [false]
        );
    }

    #[test]
    fn a_resolved_notice_never_defers_and_is_never_deferred_to() {
        let notices = vec![
            with_action("a", "1.0.0", "Audit my workspace for drift."),
            with_action("b", "2.0.0", "Audit my workspace for drift."),
        ];
        assert_eq!(
            deferred(&view(&notices, &v("2.0.0"), &CLASSIC, Some("a"))),
            [false, false]
        );
    }

    #[test]
    fn a_cursor_this_build_does_not_carry_reads_as_nothing_answered() {
        let view = view(&three(), &v("3.0.0"), &CLASSIC, Some("gone"));
        assert_eq!(view.next_id.as_deref(), Some("a"));
    }

    #[test]
    fn a_fresh_workspace_is_stamped_past_the_current_release() {
        let stamp = fresh_workspace_stamp(&three(), &v("3.0.0"));
        assert_eq!(stamp.as_deref(), Some("c"));
        assert_eq!(
            view(&three(), &v("3.0.0"), &CLASSIC, stamp.as_deref()).next_id,
            None
        );
    }

    #[test]
    fn a_fresh_workspace_still_hears_from_the_next_release() {
        let stamp = fresh_workspace_stamp(&three(), &v("2.0.0"));
        assert_eq!(stamp.as_deref(), Some("b"));
        let view = view(&three(), &v("3.0.0"), &CLASSIC, stamp.as_deref());
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn a_release_older_than_every_notice_stamps_nothing() {
        assert_eq!(fresh_workspace_stamp(&three(), &v("0.9.0")), None);
    }

    #[test]
    fn the_first_answer_moves_a_workspace_that_has_never_answered() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, None, "a", None).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn answering_an_older_notice_never_moves_the_cursor_back() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("b"), "a", None),
            None
        );
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("b"), "c", None).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn answering_the_notice_already_on_the_cursor_is_refused() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("b"), "b", None),
            None
        );
    }

    #[test]
    fn an_unknown_id_never_moves_the_cursor() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("a"), "invented", None),
            None
        );
    }

    #[test]
    fn a_notice_this_release_has_not_reached_cannot_be_answered() {
        assert_eq!(
            advanced_cursor(&three(), &v("2.0.0"), &CLASSIC, Some("a"), "c", None),
            None
        );
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("b"), "c", None).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn a_notice_whose_turn_has_not_come_cannot_be_answered() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("a"), "c", None),
            None
        );
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), &CLASSIC, Some("a"), "b", None).as_deref(),
            Some("b")
        );
    }

    async fn add_thread_in_tree(
        pool: &PgPool,
        parent: Option<uuid::Uuid>,
        trigger_id: Option<&str>,
        is_coding_agent: bool,
    ) -> uuid::Uuid {
        let thread_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, \
              parent_thread_id, trigger_id, is_coding_agent) \
             VALUES ($1, 'a thread', 'chat', 0, NOW(), false, false, $2, $3, $4)",
        )
        .bind(thread_id)
        .bind(parent)
        .bind(trigger_id)
        .bind(is_coding_agent)
        .execute(pool)
        .await
        .expect("insert thread_summary");
        thread_id
    }

    fn trigger_registry(grants: &[(&str, bool)]) -> RwLock<HashMap<String, TriggerConfig>> {
        let configs = grants.iter().map(|(id, grant)| {
            let config = TriggerConfig::from_created_payload(&serde_json::json!({
                "trigger_id": id,
                "name": format!("Trigger {id}"),
                "schedule": ["0 0 3 * * *"],
                "run": { "type": "intent", "intent": "fix what broke" },
                "coding_agent_grant": grant,
            }))
            .expect("build TriggerConfig");
            (id.to_string(), config)
        });
        RwLock::new(configs.collect())
    }

    async fn facts(pool: &PgPool, grants: &[(&str, bool)], exposure: GuardExposure) -> NoticeFacts {
        let ws = tempfile::tempdir().unwrap();
        NoticeFacts::read(pool, ws.path(), &trigger_registry(grants), exposure).await
    }

    #[tokio::test]
    async fn retired_trigger_grants_satisfy_the_notice_condition_before_review() {
        let (pool, db_name) = setup_test_db().await;
        let ws = tempfile::tempdir().unwrap();
        let configs = trigger_registry(&[("nightly", false)]);
        let exposure = GuardExposure::default();
        let condition = NoticeCondition::HasLegacyTriggerGrants;
        assert!(!condition.holds(&NoticeFacts::read(&pool, ws.path(), &configs, exposure).await));

        configs
            .write()
            .unwrap()
            .get_mut("nightly")
            .unwrap()
            .side_effect_grant = vec![crate::engine::command_guard::SideEffectCategory::Email];
        let before_review = NoticeFacts::read(&pool, ws.path(), &configs, exposure).await;
        assert!(condition.holds(&before_review));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn ungranted_fact(pool: &PgPool, grants: &[(&str, bool)]) -> bool {
        facts(pool, grants, GuardExposure::default())
            .await
            .has_ungranted_coding_agent_trigger
    }

    #[tokio::test]
    async fn the_command_guard_turns_on_only_without_a_row_and_with_an_exposure() {
        let (pool, db_name) = setup_test_db().await;
        let none = GuardExposure::default();
        let mcp = GuardExposure {
            unapproved_mcp_server: true,
            ..none
        };
        let judge = GuardExposure {
            judge_unresolved: true,
            ..none
        };
        let turns_on = async |grants: &[(&str, bool)], exposure| {
            facts(&pool, grants, exposure).await.command_guard_turns_on
        };
        assert!(!turns_on(&[], none).await);
        assert!(turns_on(&[("nightly", false)], none).await);
        assert!(turns_on(&[], mcp).await);
        assert!(turns_on(&[], judge).await);
        for value in ["true", "false"] {
            PreferenceStore::set_row_for_test(&pool, prefs::COMMAND_GUARD.key(), value)
                .await
                .unwrap();
            assert!(!turns_on(&[("nightly", false)], mcp).await, "{value}");
        }
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_ungranted_trigger_counts_once_its_tree_holds_a_coding_agent() {
        let (pool, db_name) = setup_test_db().await;
        let fire = add_thread_in_tree(&pool, None, Some("nightly"), false).await;
        assert!(!ungranted_fact(&pool, &[("nightly", false)]).await);
        let chat = add_thread_in_tree(&pool, Some(fire), None, false).await;
        add_thread_in_tree(&pool, Some(chat), None, true).await;
        assert!(ungranted_fact(&pool, &[("nightly", false)]).await);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_granted_or_deleted_trigger_never_counts() {
        let (pool, db_name) = setup_test_db().await;
        let fire = add_thread_in_tree(&pool, None, Some("nightly"), false).await;
        add_thread_in_tree(&pool, Some(fire), None, true).await;
        assert!(!ungranted_fact(&pool, &[("nightly", true)]).await);
        assert!(!ungranted_fact(&pool, &[]).await);
        assert!(!ungranted_fact(&pool, &[("other", false)]).await);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_workspace_with_no_threads_is_stamped_and_owes_nothing() {
        let (pool, db_name) = setup_test_db().await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("c"));
        let view = view(
            &three(),
            &v("3.0.0"),
            &CLASSIC,
            stored_cursor(&pool).await.as_deref(),
        );
        assert_eq!(view.next_id, None);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_used_workspace_is_left_owing_the_oldest_visible_notice() {
        let (pool, db_name) = setup_test_db().await;
        add_thread_in_tree(&pool, None, None, false).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await, None);
        let view = view(
            &three(),
            &v("3.0.0"),
            &CLASSIC,
            stored_cursor(&pool).await.as_deref(),
        );
        assert_eq!(view.next_id.as_deref(), Some("a"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_workspace_already_in_the_sequence_is_left_alone() {
        let (pool, db_name) = setup_test_db().await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        add_thread_in_tree(&pool, None, None, false).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("c"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_used_workspace_with_an_unplaceable_cursor_owes_everything_again() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set_silent(&pool, prefs::RELEASE_NOTICE_CURSOR.key(), "gone")
            .await
            .expect("the cursor key must be writable without announcing");
        add_thread_in_tree(&pool, None, None, false).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        let view = view(
            &three(),
            &v("3.0.0"),
            &CLASSIC,
            stored_cursor(&pool).await.as_deref(),
        );
        assert_eq!(view.next_id.as_deref(), Some("a"));
        teardown_test_db(&db_name).await;
    }

    const REPAIR_SQL: &str = include_str!(
        "../../../lucidos-core/migrations/20260825153250_clear_seeded_release_notice_cursor.sql"
    );

    async fn run_the_repair(pool: &PgPool) {
        sqlx::raw_sql(REPAIR_SQL)
            .execute(pool)
            .await
            .expect("the repair migration must run");
    }

    async fn add_an_answer_event(pool: &PgPool) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'ReleaseNoticeResolved', $2, 'release_notice', 'b')",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(serde_json::json!({ "notice_id": "b" }))
        .execute(pool)
        .await
        .expect("insert ReleaseNoticeResolved");
    }

    #[tokio::test]
    async fn the_repair_clears_a_stamp_a_used_workspace_never_answered() {
        let (pool, db_name) = setup_test_db().await;
        add_thread_in_tree(&pool, None, None, false).await;
        PreferenceStore::set_silent(&pool, prefs::RELEASE_NOTICE_CURSOR.key(), "b")
            .await
            .expect("the cursor key must be writable without announcing");

        run_the_repair(&pool).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;

        assert_eq!(stored_cursor(&pool).await, None);
        let view = view(&three(), &v("3.0.0"), &CLASSIC, None);
        assert_eq!(view.next_id.as_deref(), Some("a"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_repair_keeps_a_cursor_the_user_answered() {
        let (pool, db_name) = setup_test_db().await;
        add_thread_in_tree(&pool, None, None, false).await;
        PreferenceStore::set_silent(&pool, prefs::RELEASE_NOTICE_CURSOR.key(), "b")
            .await
            .expect("the cursor key must be writable without announcing");
        add_an_answer_event(&pool).await;

        run_the_repair(&pool).await;

        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_repair_keeps_a_fresh_workspaces_stamp() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set_silent(&pool, prefs::RELEASE_NOTICE_CURSOR.key(), "c")
            .await
            .expect("the cursor key must be writable without announcing");

        run_the_repair(&pool).await;

        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("c"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn answering_moves_the_cursor_and_going_back_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        assert!(resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "a", None)
            .await
            .unwrap());
        assert!(resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "b", None)
            .await
            .unwrap());
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        assert!(!resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "a", None)
            .await
            .unwrap());
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_cursor_read_that_fails_refuses_the_write() {
        let (pool, db_name) = setup_test_db().await;
        assert!(resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "a", None)
            .await
            .unwrap());
        assert!(resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "b", None)
            .await
            .unwrap());
        pool.close().await;

        let refused = resolve(&pool, &three(), &v("3.0.0"), &CLASSIC, "a", None).await;
        assert!(refused.is_err(), "an unreadable cursor must not be written");
        teardown_test_db(&db_name).await;
    }

    mod guard_tests {
        use super::*;

        fn knowhow_source(id: &str) -> String {
            let path = format!(
                "{}/../../system-knowhow/{id}.md",
                env!("CARGO_MANIFEST_DIR")
            );
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("system-knowhow/{id}.md must exist: {e}"))
        }

        fn knowhow_description(id: &str) -> String {
            knowhow_source(id)
                .lines()
                .find_map(|l| l.strip_prefix("description:"))
                .unwrap_or_else(|| panic!("system-knowhow/{id}.md needs a frontmatter description"))
                .to_lowercase()
        }

        #[test]
        fn every_action_prompt_routes_to_the_knowhow_meant_to_answer_it() {
            let routes: [(&str, &str, &[&str]); 2] = [
                (
                    "run-a-workspace-audit",
                    "workspace-audit",
                    &["audit", "workspace", "drift"],
                ),
                ("two-dashboards", "workspace-audit", &["apps", "check"]),
            ];
            let notices = parse(RELEASE_NOTICES_TOML).expect("release-notices.toml must validate");
            for (id, knowhow, words) in routes {
                let notice = notices
                    .iter()
                    .find(|n| n.id == id)
                    .unwrap_or_else(|| panic!("{id} must still be in release-notices.toml"));
                let prompt = action_prompt([notice])
                    .unwrap_or_else(|| panic!("{id} must carry the action this pins"))
                    .to_lowercase();
                let description = knowhow_description(knowhow);
                for word in words {
                    assert!(
                        prompt.contains(word),
                        "{id}'s prompt drops the routing word {word:?}"
                    );
                    assert!(
                        description.contains(word),
                        "system-knowhow/{knowhow}.md's description drops {word:?}, so {id} may not reach it"
                    );
                }
            }
        }

        #[test]
        fn a_targeted_action_names_a_check_its_audit_section_owns() {
            let notices = parse(RELEASE_NOTICES_TOML).expect("release-notices.toml must validate");
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            for notice in notices {
                for check in &notice.checks {
                    let (section, name) = check.split_once(": ").unwrap();
                    let family = crate::core::knowhow::doc_family_text(
                        &repo,
                        &format!("system-knowhow/workspace-audit/{section}.md"),
                    );
                    assert!(
                        family.to_lowercase().contains(&name.to_lowercase()),
                        "system-knowhow/workspace-audit/{section}.md must name {name:?} for {}",
                        notice.id,
                    );
                }
            }
        }
    }
}
