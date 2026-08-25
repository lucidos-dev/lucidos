//! Release notices: the one-time instructions a release hands the user.
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
//! The modal and the What's New panel are two renderings of one list. So
//! [`view`] returns every visible notice, plus the id of the next one owed.

use crate::core::preferences::PreferenceStore;
use semver::Version;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::OnceLock;

/// Where a workspace keeps its cursor. Workspace-global and silent: answering
/// on one device settles it on every device, and the resolve announces on its
/// own. See `core::preference_catalog::SILENT_PREF_KEYS`.
pub const CURSOR_PREF_KEY: &str = "release_notice_cursor";

/// The authored notices as of this build. Read that file's own header before
/// adding one: its order is load-bearing.
const RELEASE_NOTICES_TOML: &str = include_str!("../../../../release-notices.toml");

/// One authored notice, exactly as `release-notices.toml` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNotice {
    /// Stable forever. The workspace cursor is this string.
    pub id: String,
    /// The release this applies FROM, plain semver. A floor, not a stamp: the
    /// notice is invisible until the engine reports this version or newer.
    pub since: String,
    pub title: String,
    /// RAW markdown, per `.claude/rules/rust.md`: the frontend converts.
    pub body: String,
    /// The button's label. Present exactly when `action_prompt` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_label: Option<String>,
    /// The sentence the button SENDS as a new message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_prompt: Option<String>,
}

/// The file's top level: a single array of tables, `[[notice]]`.
#[derive(Debug, Default, Deserialize)]
struct NoticeFile {
    #[serde(default)]
    notice: Vec<ReleaseNotice>,
}

/// One notice as a client reads it, with this workspace's answer folded in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeState {
    #[serde(flatten)]
    pub notice: ReleaseNotice,
    /// True once the workspace has answered it. The panel keeps showing it.
    pub resolved: bool,
}

/// What both surfaces are drawn from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeView {
    /// Every visible notice, oldest first, resolved and unresolved alike.
    pub notices: Vec<NoticeState>,
    /// The one the modal shows, or `None` when the workspace owes nothing.
    pub next_id: Option<String>,
}

/// Every notice in `src`, in file order, or the first authoring mistake.
///
/// Validation lives here rather than in a test, so a hand-edited file cannot
/// half work. [`all`] logs a rejection and serves nothing, which is the safe
/// direction: no modal beats a modal built from a half-read file.
pub fn parse(src: &str) -> Result<Vec<ReleaseNotice>, String> {
    let file: NoticeFile = toml::from_str(src).map_err(|e| format!("not valid TOML: {e}"))?;
    let mut previous: Option<Version> = None;
    for (i, notice) in file.notice.iter().enumerate() {
        let at = format!("notice {} ({:?})", i + 1, notice.id);
        if notice.id.trim().is_empty() {
            return Err(format!("{at} has an empty id"));
        }
        if file.notice[..i].iter().any(|n| n.id == notice.id) {
            return Err(format!("{at} reuses an id an earlier notice already has"));
        }
        if notice.title.trim().is_empty() || notice.body.trim().is_empty() {
            return Err(format!("{at} has an empty title or body"));
        }
        if notice.action_label.is_some() != notice.action_prompt.is_some() {
            return Err(format!(
                "{at} has only half an action: both fields or neither"
            ));
        }
        let since = Version::parse(&notice.since).map_err(|e| {
            format!(
                "{at} names since = {:?}, which is not semver: {e}",
                notice.since
            )
        })?;
        // Non-decreasing rather than increasing: one release may carry several.
        if previous.as_ref().is_some_and(|p| since < *p) {
            return Err(format!(
                "{at} is older than the notice above it; append, never reorder"
            ));
        }
        previous = Some(since);
    }
    Ok(file.notice)
}

/// The authored notices, parsed once. Empty when the file did not validate.
pub fn all() -> &'static [ReleaseNotice] {
    static NOTICES: OnceLock<Vec<ReleaseNotice>> = OnceLock::new();
    NOTICES.get_or_init(|| match parse(RELEASE_NOTICES_TOML) {
        Ok(notices) => notices,
        Err(e) => {
            crate::log!("[ReleaseNotices] release-notices.toml is not usable: {e}");
            Vec::new()
        }
    })
}

/// Does `notice` apply to an engine reporting `running`?
fn applies_to(notice: &ReleaseNotice, running: &Version) -> bool {
    Version::parse(&notice.since).is_ok_and(|since| since <= *running)
}

/// Where the cursor sits in `notices`: the count of entries it has answered.
///
/// An id the file no longer carries reads as nothing answered. That needs an
/// entry deleted or renamed, which the file's header forbids, and the startup
/// seed repairs the stored value on the next boot. Showing a notice twice is
/// the recoverable direction. Swallowing one silently is not.
fn answered_count(notices: &[ReleaseNotice], cursor: Option<&str>) -> usize {
    cursor
        .and_then(|id| notices.iter().position(|n| n.id == id))
        .map_or(0, |i| i + 1)
}

/// The one notice this workspace owes an answer to, or `None`.
///
/// The single definition of "whose turn it is", shared by [`view`] (which names
/// it for the modal) and [`advanced_cursor`] (which accepts only it). Two
/// definitions would let the surfaces and the write disagree about the order.
fn owed<'a>(
    notices: &'a [ReleaseNotice],
    running: &Version,
    cursor: Option<&str>,
) -> Option<&'a ReleaseNotice> {
    notices
        .iter()
        .skip(answered_count(notices, cursor))
        .find(|n| applies_to(n, running))
}

/// Both surfaces, for one workspace on one release.
pub fn view(notices: &[ReleaseNotice], running: &Version, cursor: Option<&str>) -> NoticeView {
    let answered = answered_count(notices, cursor);
    let next_id = owed(notices, running, cursor).map(|n| n.id.clone());
    let states = notices
        .iter()
        .enumerate()
        .filter(|(_, n)| applies_to(n, running))
        .map(|(i, n)| NoticeState {
            notice: n.clone(),
            resolved: i < answered,
        })
        .collect();
    NoticeView {
        notices: states,
        next_id,
    }
}

/// The cursor a workspace with none starts at, or `None` when it starts owing
/// every notice.
///
/// The two cases differ by one comparison, deliberately. A workspace with
/// threads holds content this release may have changed, so it owes the current
/// release's notices. A workspace with none has nothing to audit and no
/// settings to migrate. It starts level and hears from the NEXT release
/// instead, which is what keeps a modal off the first-run welcome.
pub fn seed_cursor(
    notices: &[ReleaseNotice],
    running: &Version,
    has_threads: bool,
) -> Option<String> {
    notices
        .iter()
        .rev()
        .find(|n| match Version::parse(&n.since) {
            Ok(since) if has_threads => since < *running,
            Ok(since) => since <= *running,
            Err(_) => false,
        })
        .map(|n| n.id.clone())
}

/// The cursor after the workspace answers `id`, or `None` to refuse.
///
/// **Only the notice the workspace currently owes may be answered.** The cursor
/// jumps to whatever id it is given, and everything before that then reads as
/// answered. So one rule covers every way an answer could lose an instruction:
///
/// - An id this build does not carry names no place in the sequence.
/// - An id at or behind the cursor is a stale client re-answering.
/// - A queued id is a turn out of order, skipping the notices before it.
/// - A notice this release has not reached would be skipped by the release
///   that finally ships it.
///
/// The middle two are unreachable from our own surfaces, which draw from
/// [`view`] and disable what is not owed. The rule is here because the endpoint
/// is reachable without them.
pub fn advanced_cursor(
    notices: &[ReleaseNotice],
    running: &Version,
    cursor: Option<&str>,
    id: &str,
) -> Option<String> {
    owed(notices, running, cursor)
        .filter(|n| n.id == id)
        .map(|n| n.id.clone())
}

/// This workspace's cursor, or `None` when it has answered nothing.
///
/// A database failure reads as `None`, which owes the reader the full visible
/// list. That is the recoverable direction, and it is loud: the alternative
/// swallows an instruction and says nothing.
pub async fn stored_cursor(pool: &PgPool) -> Option<String> {
    match PreferenceStore::get(pool, CURSOR_PREF_KEY).await {
        Ok(cursor) => cursor,
        Err(e) => {
            crate::log!("[ReleaseNotices] could not read the cursor: {e}");
            None
        }
    }
}

/// Give a workspace its starting cursor, and repair one this build cannot read.
///
/// Runs once per boot, because both inputs are boot facts: the release this
/// binary reports, and whether the workspace has ever held a thread. A
/// workspace that already has a usable cursor is left alone.
pub async fn seed_cursor_at_startup(pool: &PgPool, notices: &[ReleaseNotice]) {
    match Version::parse(crate::LUCIDOS_RELEASE) {
        Ok(running) => place_workspace(pool, notices, &running).await,
        Err(_) => crate::log!(
            "[ReleaseNotices] {} is not semver, so no notice can be placed",
            crate::LUCIDOS_RELEASE
        ),
    }
}

/// The seed itself, with the running release passed in so it can be exercised
/// against a release the build is not on.
async fn place_workspace(pool: &PgPool, notices: &[ReleaseNotice], running: &Version) {
    if notices.is_empty() {
        return;
    }
    let stored = stored_cursor(pool).await;
    // Present and known: this workspace is already in the sequence.
    if stored.iter().any(|id| notices.iter().any(|n| &n.id == id)) {
        return;
    }
    let has_threads =
        match sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM thread_summaries)")
            .fetch_one(pool)
            .await
        {
            Ok(has_threads) => has_threads,
            Err(e) => {
                // Seeding on a guess would either stamp a real workspace silent or
                // open a modal over a first run. Skip, and let the next boot ask.
                crate::log!(
                    "[ReleaseNotices] could not tell whether the workspace has threads: {e}"
                );
                return;
            }
        };
    let Some(seed) = seed_cursor(notices, running, has_threads) else {
        return;
    };
    match PreferenceStore::set_silent(pool, CURSOR_PREF_KEY, &seed).await {
        Ok(()) => crate::log!("[ReleaseNotices] seeded the cursor at {seed}"),
        Err(e) => crate::log!("[ReleaseNotices] could not seed the cursor: {e}"),
    }
}

/// Record that the workspace has answered `id`, and say whether it moved.
///
/// Refuses anything but the notice currently owed, per [`advanced_cursor`], so
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
    notices: &[ReleaseNotice],
    running: &Version,
    id: &str,
) -> Result<bool, sqlx::Error> {
    let cursor = PreferenceStore::get(pool, CURSOR_PREF_KEY).await?;
    let Some(next) = advanced_cursor(notices, running, cursor.as_deref(), id) else {
        return Ok(false);
    };
    PreferenceStore::set_silent(pool, CURSOR_PREF_KEY, &next).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// A notice with no action, which is the shape most of them have.
    fn notice(id: &str, since: &str) -> ReleaseNotice {
        ReleaseNotice {
            id: id.to_string(),
            since: since.to_string(),
            title: format!("Notice {id}"),
            body: "Do the thing.".to_string(),
            action_label: None,
            action_prompt: None,
        }
    }

    fn three() -> Vec<ReleaseNotice> {
        vec![
            notice("a", "1.0.0"),
            notice("b", "2.0.0"),
            notice("c", "3.0.0"),
        ]
    }

    fn ids(view: &NoticeView) -> Vec<&str> {
        view.notices.iter().map(|s| s.notice.id.as_str()).collect()
    }

    #[test]
    fn a_file_with_no_entries_is_valid_and_empty() {
        assert!(parse("# nothing yet\n").unwrap().is_empty());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn entries_keep_their_file_order() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\n\n\
                   [[notice]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\n";
        let parsed = parse(src).unwrap();
        assert_eq!(
            parsed.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    /// The cursor IS the id, so a reused one would resolve two notices at once.
    #[test]
    fn a_reused_id_is_refused() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"1.0.0\"\ntitle = \"A\"\nbody = \"x\"\n\n\
                   [[notice]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\n";
        assert!(parse(src).unwrap_err().contains("reuses an id"));
    }

    /// File order is the reading order, so it has to agree with release order.
    #[test]
    fn an_entry_older_than_the_one_above_it_is_refused() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\n\n\
                   [[notice]]\nid = \"b\"\nsince = \"1.0.0\"\ntitle = \"B\"\nbody = \"y\"\n";
        assert!(parse(src).unwrap_err().contains("append, never reorder"));
    }

    #[test]
    fn two_notices_in_one_release_are_allowed() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\n\n\
                   [[notice]]\nid = \"b\"\nsince = \"2.0.0\"\ntitle = \"B\"\nbody = \"y\"\n";
        assert_eq!(parse(src).unwrap().len(), 2);
    }

    /// A label with no prompt draws a button that does nothing.
    #[test]
    fn half_an_action_is_refused() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"x\"\n\
                   action_label = \"Do it\"\n";
        assert!(parse(src).unwrap_err().contains("half an action"));
    }

    #[test]
    fn a_release_that_is_not_semver_is_refused() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"v0.30\"\ntitle = \"A\"\nbody = \"x\"\n";
        assert!(parse(src).unwrap_err().contains("not semver"));
    }

    #[test]
    fn an_empty_body_is_refused() {
        let src = "[[notice]]\nid = \"a\"\nsince = \"2.0.0\"\ntitle = \"A\"\nbody = \"  \"\n";
        assert!(parse(src).unwrap_err().contains("empty title or body"));
    }

    /// The shipped file is parsed at runtime with no chance to report a problem,
    /// so its validity is pinned here instead.
    #[test]
    fn the_shipped_file_validates() {
        parse(RELEASE_NOTICES_TOML).expect("release-notices.toml must validate");
    }

    /// A knowhow file's frontmatter `description`, lowercased. That line is what
    /// the engine splices into the routing list, so it is the half a prompt has
    /// to meet.
    fn knowhow_description(id: &str) -> String {
        let path = format!(
            "{}/../../system-knowhow/{id}.md",
            env!("CARGO_MANIFEST_DIR")
        );
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("system-knowhow/{id}.md must exist: {e}"));
        src.lines()
            .find_map(|l| l.strip_prefix("description:"))
            .unwrap_or_else(|| panic!("system-knowhow/{id}.md needs a frontmatter description"))
            .to_lowercase()
    }

    /// An action SENDS its sentence as an ordinary message, so the knowhow meant
    /// to answer it has to route on the words that sentence uses.
    ///
    /// Nothing else holds the two together. Reword either side alone and the
    /// button still works, still starts a thread, and quietly gets a general
    /// answer instead of the recipe. Same shape as
    /// `setup_interview_route_matches_the_frontend_seeded_prompt`, which pins
    /// the welcome button against its own knowhow.
    #[test]
    fn every_action_prompt_routes_to_the_knowhow_meant_to_answer_it() {
        // Notice id, the knowhow that must answer it, and the words both sides
        // have to keep. A word here is one the description ROUTES on.
        let routes = [(
            "run-a-workspace-audit",
            "workspace-audit",
            ["audit", "workspace", "drift"],
        )];
        let notices = parse(RELEASE_NOTICES_TOML).expect("release-notices.toml must validate");
        for (id, knowhow, words) in routes {
            let notice = notices
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id} must still be in release-notices.toml"));
            let prompt = notice
                .action_prompt
                .as_ref()
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

    /// A notice authored for the release being prepared must not reach the dev
    /// workspaces running the release before it.
    #[test]
    fn a_notice_newer_than_the_running_release_is_invisible() {
        let view = view(&three(), &v("2.0.0"), None);
        assert_eq!(ids(&view), ["a", "b"]);
        assert_eq!(view.next_id.as_deref(), Some("a"));
    }

    #[test]
    fn the_oldest_unresolved_notice_is_the_one_owed() {
        let view = view(&three(), &v("3.0.0"), Some("a"));
        assert_eq!(view.next_id.as_deref(), Some("b"));
        let resolved: Vec<bool> = view.notices.iter().map(|s| s.resolved).collect();
        assert_eq!(resolved, [true, false, false]);
    }

    /// The panel keeps every visible notice, answered or not, so an instruction
    /// the reader walked past is still findable.
    #[test]
    fn answering_the_last_notice_empties_the_modal_but_not_the_panel() {
        let view = view(&three(), &v("3.0.0"), Some("c"));
        assert_eq!(view.next_id, None);
        assert_eq!(ids(&view), ["a", "b", "c"]);
        assert!(view.notices.iter().all(|s| s.resolved));
    }

    #[test]
    fn a_cursor_this_build_does_not_carry_reads_as_nothing_answered() {
        let view = view(&three(), &v("3.0.0"), Some("gone"));
        assert_eq!(view.next_id.as_deref(), Some("a"));
    }

    /// A workspace with content owes the notices of the release it just took.
    #[test]
    fn a_workspace_with_threads_is_seeded_behind_the_current_release() {
        let seed = seed_cursor(&three(), &v("3.0.0"), true);
        assert_eq!(seed.as_deref(), Some("b"));
        let view = view(&three(), &v("3.0.0"), seed.as_deref());
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    /// A workspace with nothing in it has nothing to audit, so it starts level.
    #[test]
    fn a_fresh_workspace_is_seeded_past_the_current_release() {
        let seed = seed_cursor(&three(), &v("3.0.0"), false);
        assert_eq!(seed.as_deref(), Some("c"));
        assert_eq!(view(&three(), &v("3.0.0"), seed.as_deref()).next_id, None);
    }

    /// Its silence covers only the notices it started level with. The next
    /// release still reaches it, which is what makes the stamp safe.
    #[test]
    fn a_fresh_workspace_still_hears_from_the_next_release() {
        let seed = seed_cursor(&three(), &v("2.0.0"), false);
        assert_eq!(seed.as_deref(), Some("b"));
        let view = view(&three(), &v("3.0.0"), seed.as_deref());
        assert_eq!(view.next_id.as_deref(), Some("c"));
    }

    #[test]
    fn a_release_older_than_every_notice_seeds_nothing() {
        assert_eq!(seed_cursor(&three(), &v("0.9.0"), true), None);
        assert_eq!(seed_cursor(&three(), &v("0.9.0"), false), None);
    }

    #[test]
    fn the_first_answer_moves_a_workspace_that_has_never_answered() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), None, "a").as_deref(),
            Some("a")
        );
    }

    /// A second client, still showing the modal it opened before the first one
    /// answered, must not re-open what has already been walked past.
    #[test]
    fn answering_an_older_notice_never_moves_the_cursor_back() {
        assert_eq!(advanced_cursor(&three(), &v("3.0.0"), Some("b"), "a"), None);
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), Some("b"), "c").as_deref(),
            Some("c")
        );
    }

    #[test]
    fn answering_the_notice_already_on_the_cursor_is_refused() {
        assert_eq!(advanced_cursor(&three(), &v("3.0.0"), Some("b"), "b"), None);
    }

    #[test]
    fn an_unknown_id_never_moves_the_cursor() {
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), Some("a"), "invented"),
            None
        );
    }

    /// Nothing the surfaces draw can name an unreleased notice, so this is a
    /// hand-made request. Letting it through would jump the cursor over that
    /// notice, and the release that ships it would then never show it.
    #[test]
    fn a_notice_this_release_has_not_reached_cannot_be_answered() {
        assert_eq!(advanced_cursor(&three(), &v("2.0.0"), Some("a"), "c"), None);
        // The same id, once the release reaches it and it is the one owed.
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), Some("b"), "c").as_deref(),
            Some("c")
        );
    }

    /// The ordering guarantee, held at the write rather than only in the UI.
    /// The panel disables a queued notice's button, but the endpoint is
    /// reachable without the panel, and the cursor jumps to whatever id it is
    /// given. Answering "c" while "b" is owed would mark "b" answered too.
    #[test]
    fn a_notice_whose_turn_has_not_come_cannot_be_answered() {
        assert_eq!(advanced_cursor(&three(), &v("3.0.0"), Some("a"), "c"), None);
        assert_eq!(
            advanced_cursor(&three(), &v("3.0.0"), Some("a"), "b").as_deref(),
            Some("b")
        );
    }

    /// One thread is all "this workspace has been used" means here.
    async fn add_a_thread(pool: &PgPool) {
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved) \
             VALUES ($1, 'a thread', 'chat', 0, NOW(), false, false)",
        )
        .bind(uuid::Uuid::new_v4())
        .execute(pool)
        .await
        .expect("insert thread_summary");
    }

    /// The invariant the whole seed exists for: nothing stacks over a first run.
    #[tokio::test]
    async fn a_workspace_with_no_threads_is_stamped_and_owes_nothing() {
        let (pool, db_name) = setup_test_db().await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("c"));
        let view = view(&three(), &v("3.0.0"), stored_cursor(&pool).await.as_deref());
        assert_eq!(view.next_id, None);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_workspace_that_has_been_used_owes_the_current_release() {
        let (pool, db_name) = setup_test_db().await;
        add_a_thread(&pool).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        teardown_test_db(&db_name).await;
    }

    /// The seed runs on every boot, so it has to be a no-op after the first.
    /// Re-deriving would walk a workspace back through notices it answered.
    #[tokio::test]
    async fn a_workspace_already_in_the_sequence_is_left_alone() {
        let (pool, db_name) = setup_test_db().await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        add_a_thread(&pool).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("c"));
        teardown_test_db(&db_name).await;
    }

    /// A stored id this build cannot place reads as nothing answered. So the
    /// next boot repairs the value, rather than leaving it owing every notice.
    #[tokio::test]
    async fn a_cursor_naming_an_unknown_notice_is_repaired() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set_silent(&pool, CURSOR_PREF_KEY, "gone")
            .await
            .expect("the cursor key must be writable without announcing");
        add_a_thread(&pool).await;
        place_workspace(&pool, &three(), &v("3.0.0")).await;
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn answering_moves_the_cursor_and_going_back_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        assert!(resolve(&pool, &three(), &v("3.0.0"), "a").await.unwrap());
        assert!(resolve(&pool, &three(), &v("3.0.0"), "b").await.unwrap());
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        assert!(!resolve(&pool, &three(), &v("3.0.0"), "a").await.unwrap());
        assert_eq!(stored_cursor(&pool).await.as_deref(), Some("b"));
        teardown_test_db(&db_name).await;
    }

    /// A write that cannot read the current position must not guess one.
    ///
    /// `stored_cursor` answers a DISPLAY with `None` on a failed read, which is
    /// the safe direction for a list. Reused here it would read as "nothing
    /// answered". The first notice would then look owed, and the write would
    /// rewind the cursor onto it, re-opening everything after.
    #[tokio::test]
    async fn a_cursor_read_that_fails_refuses_the_write() {
        let (pool, db_name) = setup_test_db().await;
        assert!(resolve(&pool, &three(), &v("3.0.0"), "a").await.unwrap());
        assert!(resolve(&pool, &three(), &v("3.0.0"), "b").await.unwrap());
        pool.close().await;

        let refused = resolve(&pool, &three(), &v("3.0.0"), "a").await;
        assert!(refused.is_err(), "an unreadable cursor must not be written");
        teardown_test_db(&db_name).await;
    }
}
