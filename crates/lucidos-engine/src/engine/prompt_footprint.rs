//! The *workspace prompt footprint* report (ADR 0413): what each
//! workspace-grown section of a chat turn costs, against the user's ceilings,
//! with the evidence of which items are still used.
//!
//! The sections themselves are built in `chat::process::footprint_sections`,
//! by the builders a turn calls. This module owns the report's shape and the
//! use evidence, which reads the events table.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use sqlx::PgPool;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// When a section rides a chat turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionWhen {
    EveryTurn,
    /// Only when the turn's classifier asks for it.
    Gated,
    /// Only while the user has an app open.
    OpenApp,
    /// Only when a thread's enabled MCP servers offer more tools than fit.
    ToolsDropped,
    /// Only in the threads that enable an MCP server (ADR 0430).
    McpEnabled,
}

impl SectionWhen {
    /// Whether a section with this `when` counts toward the report's total.
    /// The MCP schemas, and the notice for the ones the ceiling cuts, ride only
    /// threads that enable servers, and nearly every thread enables none.
    pub fn counts_in_total(self) -> bool {
        !matches!(self, Self::McpEnabled | Self::ToolsDropped)
    }
}

/// What a footprint item is, which says where it is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    App,
    ReusableWidget,
    Knowhow,
    Intent,
    ResponseStyle,
    UserProfile,
    EmailAccount,
    OauthAccount,
    Credential,
    McpServer,
    /// One app's own know-how listing, sent while that app is open.
    AppKnowhow,
}

/// Whether an item earns its place, judged over the unused window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UseVerdict {
    Used,
    Unused,
    /// The evidence does not cover the whole window yet: the item is younger
    /// than it, or its use was not recorded for that long.
    NotYetJudged,
    /// A knowhow doc with no read the engine saw in the window. A shell read
    /// leaves no trace, so a doc is never proven unused, and nothing offers
    /// to delete one on this verdict.
    NotLoadedByName,
}

/// One item's last use and the verdict drawn from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemUsage {
    /// Whole days since the newest use, or `None` when none is recorded.
    pub last_used_days_ago: Option<i64>,
    pub verdict: UseVerdict,
}

/// One line of a section: an app, a knowhow doc, an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FootprintItem {
    pub kind: ItemKind,
    pub id: String,
    pub name: String,
    /// What this item's line costs in the prompt.
    pub chars: usize,
    /// How much of the written description the line leaves out.
    pub clipped_chars: usize,
    /// Present for apps, reusable widgets and knowhow docs only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ItemUsage>,
    /// A knowhow doc's file, relative to `data/`, where the user edits it.
    /// Absent for a doc outside the workspace, such as a shared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// An MCP server's schemas only: which threads paid for them and which
    /// used them, over the unused window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_threads: Option<McpThreadUse>,
}

/// How many threads enabled one MCP server, and how many called one of its
/// tools, over the unused window (ADR 0430).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct McpThreadUse {
    pub enabled: i64,
    pub called: i64,
}

/// One workspace-grown section of a chat turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FootprintSection {
    pub id: &'static str,
    pub title: &'static str,
    pub when: SectionWhen,
    pub chars: usize,
    pub over_ceiling: bool,
    pub items: Vec<FootprintItem>,
}

/// One area of the *system prompt footprint*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemPromptArea {
    pub label: &'static str,
    pub chars: usize,
}

/// The whole report, as the route, the CLI and the Settings page read it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspacePromptFootprint {
    pub sections: Vec<FootprintSection>,
    /// Every section that can ride every turn, summed: the worst turn, with
    /// every gate open and the largest app open. The MCP tool schemas and their
    /// tools-not-sent notice are not in it, since only a thread that enables a
    /// server carries them.
    pub total_chars: usize,
    pub over_total_ceiling: bool,
    pub section_ceiling: usize,
    pub total_ceiling: usize,
    pub unused_days: u32,
    pub system_prompt_chars: usize,
    pub system_prompt_areas: Vec<SystemPromptArea>,
}

/// The three preferences the report is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootprintLimits {
    pub section_ceiling: usize,
    pub total_ceiling: usize,
    pub unused_days: u32,
}

impl FootprintLimits {
    pub async fn read(pool: &PgPool) -> Self {
        use crate::core::prefs;
        Self {
            section_ceiling: prefs::WORKSPACE_PROMPT_FOOTPRINT_SECTION_CEILING
                .read(pool)
                .await
                .round() as usize,
            total_ceiling: prefs::WORKSPACE_PROMPT_FOOTPRINT_TOTAL_CEILING
                .read(pool)
                .await
                .round() as usize,
            unused_days: prefs::WORKSPACE_PROMPT_FOOTPRINT_UNUSED_DAYS
                .read(pool)
                .await
                .round() as u32,
        }
    }
}

/// The newest recorded use of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastUse {
    pub days_ago: i64,
    pub in_window: bool,
}

/// The use evidence for one kind of item, by item id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UseEvidence {
    pub last_use: HashMap<String, LastUse>,
    /// Whether this kind of use was recorded for the whole window. See
    /// `use_evidence` for when it is not.
    pub covers_window: bool,
}

impl UseEvidence {
    /// Judge one item, given whether its file is older than the window.
    pub fn usage_of(&self, id: &str, older_than_window: bool) -> ItemUsage {
        let last_use = self.last_use.get(id).copied();
        ItemUsage {
            last_used_days_ago: last_use.map(|u| u.days_ago),
            verdict: judge_use(
                last_use.is_some_and(|u| u.in_window),
                older_than_window,
                self.covers_window,
            ),
        }
    }
}

/// An item is unused only when evidence covering the whole window shows no
/// use in it. Anything less is not yet judged, so a fresh install never lists
/// every app as dead.
pub fn judge_use(
    used_in_window: bool,
    older_than_window: bool,
    evidence_covers_window: bool,
) -> UseVerdict {
    if used_in_window {
        UseVerdict::Used
    } else if older_than_window && evidence_covers_window {
        UseVerdict::Unused
    } else {
        UseVerdict::NotYetJudged
    }
}

/// Whether the file at `path` was made more than `days` ago, by its creation
/// time, or its modification time where the filesystem keeps none. A missing
/// file is not older: it cannot be judged.
///
/// Both sides of the comparison read the host clock, so ADR 0053's two-clock
/// hazard does not arise here.
pub fn file_older_than(path: &Path, days: u32) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(made) = meta.created().or_else(|_| meta.modified()) else {
        return false;
    };
    std::time::SystemTime::now()
        .duration_since(made)
        .is_ok_and(|age| age.as_secs() >= u64::from(days) * 86_400)
}

/// Where one kind of use is recorded: the id expression and the row filter
/// over `events`. Every window resolves in SQL (ADR 0053).
struct UseSource {
    id: &'static str,
    filter: &'static str,
    /// The rows are thread events, which deleting a thread removes with it.
    thread_scoped: bool,
}

const APP_OPENS: UseSource = UseSource {
    id: "aggregate_id",
    filter: "event_type = 'AppOpened'",
    thread_scoped: false,
};

/// A widget is used where it is pinned, and where an old `WidgetShown` showed
/// it (ADR 0466). Its embeds in replies count too, but the embed grammar reads
/// those (`widget_embed_uses`). A reply has no `app_id`, so it adds no use
/// here, only its age to the record's coverage.
const WIDGET_PINS: UseSource = UseSource {
    id: "payload->>'app_id'",
    filter: "event_type IN ('WidgetShown', 'WidgetPinned', 'ResponseGenerated')",
    thread_scoped: true,
};

/// Every knowhow read the engine sees, keyed by the routing-list id:
/// `load_knowhow` names it, `read_file` gives a data path to map, and
/// `KnowhowRead` records `lucidos knowhow read`.
const KNOWHOW_LOADS: UseSource = UseSource {
    id: "CASE \
           WHEN event_type = 'KnowhowRead' THEN aggregate_id \
           WHEN payload->>'name' = 'load_knowhow' THEN payload->'args'->>'id' \
           ELSE regexp_replace(regexp_replace(payload->'args'->>'path', \
                  '^(data/)?knowhow/(.*)\\.md$', '\\2'), \
                  '^(data/)?apps/([^/]+)/knowhow/(.*)\\.md$', '\\2/\\3') \
         END",
    filter: "event_type = 'KnowhowRead' \
             OR (event_type = 'ToolCalled' AND (payload->>'name' = 'load_knowhow' \
                 OR (payload->>'name' = 'read_file' AND NOT payload->'args' ? 'repo' \
                     AND payload->'args'->>'path' ~ '(^|/)knowhow/.*\\.md$')))",
    thread_scoped: true,
};

/// The newest use per id, and whether the record covers the whole window.
///
/// It covers the window once its oldest row is older than the window, since
/// uses before the event existed were never recorded. Thread-scoped evidence
/// also needs no `ThreadsDeleted` inside the window: a deleted thread took its
/// uses with it, so a missing use proves nothing.
async fn use_evidence(
    pool: &PgPool,
    source: &UseSource,
    days: u32,
) -> Result<UseEvidence, BoxError> {
    let UseSource {
        id,
        filter,
        thread_scoped,
    } = source;
    let last_use_sql = format!(
        "SELECT {id}, \
                floor(EXTRACT(EPOCH FROM now() - max(created)) / 86400)::bigint, \
                max(created) >= now() - make_interval(days => $1) \
         FROM events WHERE ({filter}) AND {id} IS NOT NULL GROUP BY {id}"
    );
    let rows: Vec<(String, i64, bool)> = sqlx::query_as(&last_use_sql)
        .bind(days as i32)
        .fetch_all(pool)
        .await?;
    let covers_sql = format!(
        "SELECT coalesce((SELECT min(created) FROM events WHERE ({filter})) \
                         <= now() - make_interval(days => $1), false) \
            AND NOT ($2 AND EXISTS (SELECT 1 FROM events WHERE event_type = 'ThreadsDeleted' \
                                    AND created >= now() - make_interval(days => $1)))"
    );
    let covers_window: bool = sqlx::query_scalar(&covers_sql)
        .bind(days as i32)
        .bind(thread_scoped)
        .fetch_one(pool)
        .await?;
    Ok(UseEvidence {
        last_use: rows
            .into_iter()
            .map(|(id, days_ago, in_window)| {
                (
                    id,
                    LastUse {
                        days_ago,
                        in_window,
                    },
                )
            })
            .collect(),
        covers_window,
    })
}

/// Apps by their newest `AppOpened`.
pub async fn app_open_evidence(pool: &PgPool, days: u32) -> Result<UseEvidence, BoxError> {
    use_evidence(pool, &APP_OPENS, days).await
}

/// Reusable widgets by their newest use, in any thread: an embed in a reply,
/// a pin, or an old `WidgetShown`.
pub async fn widget_use_evidence(pool: &PgPool, days: u32) -> Result<UseEvidence, BoxError> {
    let mut evidence = use_evidence(pool, &WIDGET_PINS, days).await?;
    for (app_id, used) in widget_embed_uses(pool, days).await? {
        let newer = evidence
            .last_use
            .get(&app_id)
            .is_none_or(|known| used.days_ago < known.days_ago);
        if newer {
            evidence.last_use.insert(app_id, used);
        }
    }
    Ok(evidence)
}

/// The newest embed of each widget in a reply. The reply's whole text is read,
/// since a streamed chunk can split an embed, and the embed grammar decides
/// what draws: an example in a code block does not.
async fn widget_embed_uses(pool: &PgPool, days: u32) -> Result<HashMap<String, LastUse>, BoxError> {
    let rows: Vec<(String, i64, bool)> = sqlx::query_as(
        "SELECT payload->>'text', \
                floor(EXTRACT(EPOCH FROM now() - created) / 86400)::bigint, \
                created >= now() - make_interval(days => $1) \
         FROM events WHERE event_type = 'ResponseGenerated' AND payload->>'text' LIKE '%](app:%'",
    )
    .bind(days as i32)
    .fetch_all(pool)
    .await?;
    let mut uses: HashMap<String, LastUse> = HashMap::new();
    for (text, days_ago, in_window) in rows {
        for found in crate::engine::widget_embed::scan(&text) {
            let Ok(embed) = found.embed else { continue };
            let used = LastUse {
                days_ago,
                in_window,
            };
            let newer = uses
                .get(&embed.app_id)
                .is_none_or(|known| days_ago < known.days_ago);
            if newer {
                uses.insert(embed.app_id, used);
            }
        }
    }
    Ok(uses)
}

/// Per MCP server id: the threads that enabled it, and the threads that called
/// one of its tools, in the last `days`. Resolved in SQL (ADR 0053).
pub async fn mcp_thread_use(
    pool: &PgPool,
    days: u32,
) -> Result<HashMap<String, McpThreadUse>, BoxError> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT server_id, \
                count(DISTINCT thread_id) FILTER (WHERE enabled), \
                count(DISTINCT thread_id) FILTER (WHERE NOT enabled) \
         FROM ( \
           SELECT payload->>'server_id' AS server_id, thread_id, TRUE AS enabled \
           FROM events WHERE event_type = 'ThreadMcpServerEnabled' \
             AND created >= now() - make_interval(days => $1) \
           UNION ALL \
           SELECT split_part(payload->>'name', '__', 2), thread_id, FALSE \
           FROM events WHERE event_type = 'ToolCalled' \
             AND payload->>'name' LIKE 'mcp\\_\\_%' \
             AND created >= now() - make_interval(days => $1) \
         ) uses \
         WHERE server_id <> '' GROUP BY server_id",
    )
    .bind(days as i32)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(server_id, enabled, called)| (server_id, McpThreadUse { enabled, called }))
        .collect())
}

/// Knowhow docs by their newest read the engine saw, keyed by the id the
/// routing list names.
pub async fn knowhow_load_evidence(pool: &PgPool, days: u32) -> Result<UseEvidence, BoxError> {
    use_evidence(pool, &KNOWHOW_LOADS, days).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_used_inside_the_window_is_used() {
        assert_eq!(judge_use(true, true, true), UseVerdict::Used);
        assert_eq!(judge_use(true, false, false), UseVerdict::Used);
    }

    /// The fresh-install case: an app made yesterday has had no chance.
    #[test]
    fn an_item_younger_than_the_window_is_not_yet_judged() {
        assert_eq!(judge_use(false, false, true), UseVerdict::NotYetJudged);
    }

    /// An old app with no recorded open, before `AppOpened` has run a whole
    /// window, may well be opened daily.
    #[test]
    fn an_old_item_before_recording_covers_the_window_is_not_yet_judged() {
        assert_eq!(judge_use(false, true, false), UseVerdict::NotYetJudged);
    }

    /// Opened only before the window, with recording covering all of it.
    #[test]
    fn an_old_item_unused_for_the_whole_window_is_unused() {
        assert_eq!(judge_use(false, true, true), UseVerdict::Unused);
    }

    #[test]
    fn usage_reports_the_last_use_beside_the_verdict() {
        let evidence = UseEvidence {
            last_use: HashMap::from([(
                "habit-tracker".to_string(),
                LastUse {
                    days_ago: 90,
                    in_window: false,
                },
            )]),
            covers_window: true,
        };
        assert_eq!(
            evidence.usage_of("habit-tracker", true),
            ItemUsage {
                last_used_days_ago: Some(90),
                verdict: UseVerdict::Unused,
            }
        );
        assert_eq!(
            evidence.usage_of("never-opened", true).last_used_days_ago,
            None
        );
    }

    /// A row `days_ago` old. Seeded raw, because the age is the point and
    /// EventBus stamps `now()`.
    async fn seed(
        pool: &PgPool,
        event_type: &str,
        aggregate_id: &str,
        payload: serde_json::Value,
        days_ago: i32,
    ) {
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created) \
             VALUES ($1, 'app', $2, $3, $4, now() - make_interval(days => $5))",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(aggregate_id)
        .bind(event_type)
        .bind(payload)
        .bind(days_ago)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn app_opens_judge_against_a_window_resolved_in_sql() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let none = app_open_evidence(&pool, 60).await.unwrap();
        assert!(!none.covers_window, "no open ever recorded covers nothing");

        seed(
            &pool,
            "AppOpened",
            "habit-tracker",
            serde_json::json!({}),
            90,
        )
        .await;
        seed(&pool, "AppOpened", "documents", serde_json::json!({}), 3).await;
        let evidence = app_open_evidence(&pool, 60).await.unwrap();
        assert!(evidence.covers_window);
        assert_eq!(
            evidence.last_use["habit-tracker"],
            LastUse {
                days_ago: 90,
                in_window: false
            }
        );
        assert!(evidence.last_use["documents"].in_window);
        assert_eq!(
            evidence.usage_of("habit-tracker", true).verdict,
            UseVerdict::Unused
        );

        let wider = app_open_evidence(&pool, 120).await.unwrap();
        assert!(!wider.covers_window, "recording is younger than 120 days");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn widget_uses_and_knowhow_loads_are_keyed_by_the_id_they_name() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        seed(
            &pool,
            "WidgetShown",
            "x",
            serde_json::json!({ "app_id": "fare-grid" }),
            2,
        )
        .await;
        seed(
            &pool,
            "ToolCalled",
            "x",
            serde_json::json!({ "name": "load_knowhow", "args": { "id": "ops/nightly" } }),
            70,
        )
        .await;
        seed(
            &pool,
            "ToolCalled",
            "x",
            serde_json::json!({ "name": "read_file", "args": { "id": "ops/other" } }),
            1,
        )
        .await;

        seed(
            &pool,
            "ResponseGenerated",
            "x",
            serde_json::json!({ "text": "Pick one: ![Fares](app:fare-picker?params={\"day\":1}), ![](app:clock) or ![v](app:Fare_grid.v2). Written `![x](app:example)`." }),
            3,
        )
        .await;
        seed(
            &pool,
            "WidgetPinned",
            "x",
            serde_json::json!({ "app_id": "inbox" }),
            30,
        )
        .await;

        let widgets = widget_use_evidence(&pool, 60).await.unwrap();
        assert!(widgets.last_use["fare-grid"].in_window, "an old showing");
        assert!(
            widgets.last_use["fare-picker"].in_window,
            "an embed in a reply"
        );
        assert!(
            widgets.last_use["clock"].in_window,
            "a second embed in the same reply"
        );
        assert!(
            widgets.last_use["Fare_grid.v2"].in_window,
            "any id the embed grammar takes"
        );
        assert!(
            !widgets.last_use.contains_key("example"),
            "an embed in code draws nothing, so it is no use"
        );
        assert!(widgets.last_use["inbox"].in_window, "a pin");

        seed(
            &pool,
            "ToolCalled",
            "x",
            serde_json::json!({ "name": "read_file", "args": { "path": "knowhow/ops/runbook.md" } }),
            5,
        )
        .await;
        seed(
            &pool,
            "ToolCalled",
            "x",
            serde_json::json!({ "name": "read_file", "args": { "path": "data/apps/habit-tracker/knowhow/flow.md" } }),
            6,
        )
        .await;
        seed(
            &pool,
            "KnowhowRead",
            "ops/cli-only",
            serde_json::json!({}),
            7,
        )
        .await;
        seed(
            &pool,
            "ToolCalled",
            "x",
            serde_json::json!({ "name": "read_file", "args": { "repo": "example-repo", "path": "knowhow/ops/repo-doc.md" } }),
            3,
        )
        .await;

        let knowhow = knowhow_load_evidence(&pool, 60).await.unwrap();
        assert_eq!(knowhow.last_use["ops/nightly"].days_ago, 70);
        assert!(!knowhow.last_use.contains_key("ops/other"));
        assert!(
            knowhow.last_use["ops/runbook"].in_window,
            "read_file counts"
        );
        assert!(
            knowhow.last_use["habit-tracker/flow"].in_window,
            "an app's doc maps to its id"
        );
        assert!(
            knowhow.last_use["ops/cli-only"].in_window,
            "lucidos knowhow read counts"
        );
        assert!(
            !knowhow.last_use.contains_key("ops/repo-doc"),
            "a repository's file is not the workspace's doc"
        );
        assert!(knowhow.covers_window);
        assert!(
            !widgets.covers_window,
            "a month of widget uses cannot cover sixty days"
        );

        // A deleted thread took its loads with it, so a missing load inside
        // the window proves nothing.
        seed(
            &pool,
            "AppOpened",
            "habit-tracker",
            serde_json::json!({}),
            90,
        )
        .await;
        seed(&pool, "ThreadsDeleted", "global", serde_json::json!({}), 1).await;
        assert!(
            !knowhow_load_evidence(&pool, 60)
                .await
                .unwrap()
                .covers_window
        );
        assert!(
            app_open_evidence(&pool, 60).await.unwrap().covers_window,
            "app opens are not thread events, so a delete leaves them whole"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A thread event `days_ago` old, on `thread_id`.
    async fn seed_on_thread(
        pool: &PgPool,
        thread_id: uuid::Uuid,
        event_type: &str,
        payload: serde_json::Value,
        days_ago: i32,
    ) {
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, thread_id, event_type, payload, created) \
             VALUES ($1, 'thread', $2, $3, $4, $5, now() - make_interval(days => $6))",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(thread_id)
        .bind(event_type)
        .bind(payload)
        .bind(days_ago)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Per server: the threads that enabled it and the threads that called
    /// one of its tools, inside the window only, each thread once.
    #[tokio::test]
    async fn mcp_thread_use_counts_enabling_and_calling_threads_in_the_window() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (a, b, old) = (
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
        );
        let enabled = |id: &str| serde_json::json!({ "server_id": id, "by": "agent" });
        let called = |name: &str| serde_json::json!({ "name": name, "args": {} });
        seed_on_thread(&pool, a, "ThreadMcpServerEnabled", enabled("slack"), 1).await;
        seed_on_thread(&pool, b, "ThreadMcpServerEnabled", enabled("slack"), 2).await;
        seed_on_thread(&pool, a, "ToolCalled", called("mcp__slack__search"), 1).await;
        seed_on_thread(&pool, a, "ToolCalled", called("mcp__slack__post"), 1).await;
        seed_on_thread(&pool, old, "ThreadMcpServerEnabled", enabled("jira"), 90).await;
        seed_on_thread(&pool, a, "ToolCalled", called("read_file"), 1).await;

        let usage = mcp_thread_use(&pool, 30).await.unwrap();
        assert_eq!(
            usage.get("slack"),
            Some(&McpThreadUse {
                enabled: 2,
                called: 1
            })
        );
        assert_eq!(usage.get("jira"), None, "outside the window");
        assert_eq!(usage.len(), 1, "{usage:?}");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_missing_file_is_never_older_than_the_window() {
        assert!(!file_older_than(Path::new("/nonexistent/manifest.json"), 1));
    }

    #[test]
    fn a_file_just_written_is_younger_than_a_day() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("manifest.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(!file_older_than(&path, 1));
    }
}
