//! The *memory module* switch and the memory view budgets (ADR 0362).
//!
//! `memory_module` picks Classic or Tree per workspace. A turn takes the Tree
//! path only once the workspace is ready: its workspace tree and its recently
//! active threads are built (I7). Older threads fill in after. Every read here
//! is total. An unreadable module or ready flag resolves to Classic, because
//! Classic spends nothing. An unreadable view size resolves to its default.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::prefs::{self, Number, Pref};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryModule {
    Classic,
    Tree,
}

impl MemoryModule {
    /// A stored `memory_module` value, `None` for one the catalog refuses.
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "tree" => Some(Self::Tree),
            prefs::MEMORY_MODULE_WHILE_UNSET => Some(Self::Classic),
            _ => None,
        }
    }

    /// The module a stored value runs. An unset or unknown one runs
    /// [`prefs::MEMORY_MODULE_WHILE_UNSET`].
    pub(crate) fn from_pref(value: Option<&str>) -> Self {
        value.and_then(Self::parse).unwrap_or(Self::Classic)
    }

    /// The workspace's own choice, `None` while it has never chosen. The
    /// memory choice asks the user exactly then.
    pub(crate) async fn stored(pool: &PgPool) -> Result<Option<Self>, sqlx::Error> {
        Ok(prefs::MEMORY_MODULE
            .try_read(pool)
            .await?
            .as_deref()
            .and_then(Self::parse))
    }

    /// The module the workspace runs, whether or not its trees are ready. An
    /// unreadable preference is Classic: Tree spends money, and a workspace
    /// that stored Classic must not start a backfill on a blip.
    pub(crate) async fn chosen(pool: &PgPool) -> Self {
        match Self::stored(pool).await {
            Ok(stored) => stored.unwrap_or(Self::Classic),
            Err(e) => {
                log_legacy!(
                    "[SummaryTree] Failed to read the memory module: {}. Staying on Classic",
                    e
                );
                Self::Classic
            }
        }
    }
}

/// Whether the workspace runs Tree, ready or not: what keeps the compactor
/// running.
pub(crate) async fn tree_chosen(pool: &PgPool) -> bool {
    MemoryModule::chosen(pool).await == MemoryModule::Tree
}

/// Whether a turn takes the Tree path: the workspace chose Tree, and the ready
/// flag is set.
pub async fn tree_ready(pool: &PgPool) -> bool {
    if !tree_chosen(pool).await {
        return false;
    }
    match super::store::is_ready(pool).await {
        Ok(ready) => ready,
        Err(e) => {
            log_legacy!(
                "[SummaryTree] Failed to read the ready flag: {}. Staying on Classic",
                e
            );
            false
        }
    }
}

pub use crate::engine::thread_events::tree_backfill::BackfillProgress;

/// What `GET /api/v1/memory/tree-backfill` serves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TreeBackfill {
    /// The workspace is on Classic. `started` says a backfill has written
    /// nodes before, so going back to Tree resumes it with no new confirm.
    Off { started: bool },
    /// Tree is chosen and the ready flag is unset.
    Running { progress: BackfillProgress },
    /// Tree is chosen and turns take the Tree path. `filling` counts the
    /// older threads while they are still being built.
    Ready {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filling: Option<BackfillProgress>,
    },
}

/// The backfill state, from the preference, the ready flag and the
/// compactor's count. Unlike [`tree_ready`] it reports a failed read.
pub(crate) async fn tree_backfill(
    pool: &PgPool,
    runtime: &super::Runtime,
) -> Result<TreeBackfill, Box<dyn std::error::Error + Send + Sync>> {
    if MemoryModule::stored(pool).await? != Some(MemoryModule::Tree) {
        return Ok(TreeBackfill::Off {
            started: super::store::has_nodes(pool).await?,
        });
    }
    if super::store::is_ready(pool).await? {
        return Ok(TreeBackfill::Ready {
            filling: runtime.backfill_progress(),
        });
    }
    Ok(TreeBackfill::Running {
        progress: runtime.backfill_progress().unwrap_or_default(),
    })
}

/// Where a memory view is read. Each has its own workspace view size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Home,
    Chat,
    Trigger,
    CodingAgent,
}

impl Surface {
    /// The preference holding this surface's workspace-view size.
    fn workspace_view_bytes(self) -> &'static Pref<Number> {
        match self {
            Self::Home => &prefs::WORKSPACE_VIEW_BYTES_HOME,
            Self::Chat => &prefs::WORKSPACE_VIEW_BYTES_CHAT,
            Self::Trigger => &prefs::WORKSPACE_VIEW_BYTES_TRIGGER,
            Self::CodingAgent => &prefs::WORKSPACE_VIEW_BYTES_CODING_AGENT,
        }
    }
}

/// The byte budgets of one turn's two views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewBudgets {
    pub workspace: usize,
    pub thread: usize,
}

impl ViewBudgets {
    /// The budgets for `surface`, each capped for `model` when the workspace
    /// names one in `memory_view_model_caps`.
    pub async fn resolve(pool: &PgPool, surface: Surface, model: Option<&str>) -> Self {
        let workspace = surface.workspace_view_bytes().read(pool).await as usize;
        let thread = prefs::THREAD_VIEW_BYTES.read(pool).await as usize;
        let caps = prefs::MEMORY_VIEW_MODEL_CAPS.read(pool).await;
        Self::from_parts(workspace, thread, caps.as_deref(), model)
    }

    /// Cap both views so together they take at most `limit` bytes, half
    /// each. A small model window then shrinks the views, not the turn.
    pub fn within(self, limit: usize) -> Self {
        Self {
            workspace: self.workspace.min(limit / 2),
            thread: self.thread.min(limit / 2),
        }
    }

    pub(crate) fn from_parts(
        workspace: usize,
        thread: usize,
        caps: Option<&str>,
        model: Option<&str>,
    ) -> Self {
        let cap = match (caps, model) {
            (Some(caps), Some(model)) => model_cap(caps, model),
            _ => None,
        };
        let capped = |bytes: usize| cap.map_or(bytes, |cap| bytes.min(cap));
        Self {
            workspace: capped(workspace),
            thread: capped(thread),
        }
    }
}

/// The cap a `memory_view_model_caps` value sets for `model`. A model id
/// matches with or without its context suffix (`[1m]`).
fn model_cap(caps: &str, model: &str) -> Option<usize> {
    let base = model.split('[').next().unwrap_or(model);
    parse_model_caps(caps)
        .ok()?
        .into_iter()
        .find(|(id, _)| id == model || id == base)
        .map(|(_, bytes)| bytes)
}

use crate::core::prefs::parse_model_caps;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tree_selects_tree() {
        assert_eq!(MemoryModule::from_pref(None), MemoryModule::Classic);
        assert_eq!(
            MemoryModule::from_pref(Some("bogus")),
            MemoryModule::Classic
        );
        assert_eq!(
            MemoryModule::from_pref(Some("classic")),
            MemoryModule::Classic
        );
        assert_eq!(MemoryModule::from_pref(Some("tree")), MemoryModule::Tree);
    }

    /// Nothing spends on Tree before someone chooses it: a workspace that
    /// stores no module runs Classic, and its choice reads as unset.
    #[tokio::test]
    async fn an_unset_module_runs_classic() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        assert_eq!(MemoryModule::stored(&pool).await.unwrap(), None);
        assert_eq!(MemoryModule::chosen(&pool).await, MemoryModule::Classic);
        assert!(!tree_chosen(&pool).await);
        assert!(!tree_ready(&pool).await);

        crate::test_support::seed_preference(&pool, prefs::MEMORY_MODULE.key(), "tree")
            .await
            .unwrap();
        assert_eq!(
            MemoryModule::stored(&pool).await.unwrap(),
            Some(MemoryModule::Tree)
        );
        assert!(tree_chosen(&pool).await);
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// I6: a surface with no preference row set resolves to the preference
    /// catalog's default for its key, the only place that default is written.
    #[tokio::test]
    async fn each_surface_resolves_to_its_catalog_default() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        for surface in [
            Surface::Home,
            Surface::Chat,
            Surface::Trigger,
            Surface::CodingAgent,
        ] {
            let expected = surface.workspace_view_bytes().default_number() as usize;
            let budgets = ViewBudgets::resolve(&pool, surface, None).await;
            assert_eq!(budgets.workspace, expected, "{surface:?}");
        }
        let expected_thread = prefs::THREAD_VIEW_BYTES.default_number() as usize;
        let budgets = ViewBudgets::resolve(&pool, Surface::Home, None).await;
        assert_eq!(budgets.thread, expected_thread);
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// I6: a per-model cap lowers both views, and only for that model.
    #[test]
    fn a_model_cap_lowers_both_views_for_that_model_only() {
        let caps = Some("claude-haiku-4-5=8192, gpt-5-mini = 4096");
        let haiku = ViewBudgets::from_parts(65_536, 65_536, caps, Some("claude-haiku-4-5"));
        assert_eq!(
            haiku,
            ViewBudgets {
                workspace: 8192,
                thread: 8192
            }
        );
        let other = ViewBudgets::from_parts(16_384, 65_536, caps, Some("claude-opus-5"));
        assert_eq!(
            other,
            ViewBudgets {
                workspace: 16_384,
                thread: 65_536
            }
        );
        let small = ViewBudgets::from_parts(2_000, 65_536, caps, Some("gpt-5-mini"));
        assert_eq!(
            small,
            ViewBudgets {
                workspace: 2_000,
                thread: 4096
            }
        );
    }

    #[test]
    fn a_context_suffix_still_matches_its_model() {
        assert_eq!(
            model_cap("claude-opus-5-5=1000", "claude-opus-5-5[1m]"),
            Some(1000)
        );
    }

    #[test]
    fn a_small_window_halves_its_limit_between_the_views() {
        let budgets = ViewBudgets {
            workspace: 65_536,
            thread: 65_536,
        }
        .within(40_000);
        assert_eq!(
            budgets,
            ViewBudgets {
                workspace: 20_000,
                thread: 20_000
            }
        );
        let roomy = ViewBudgets {
            workspace: 16_384,
            thread: 65_536,
        }
        .within(usize::MAX);
        assert_eq!(
            roomy,
            ViewBudgets {
                workspace: 16_384,
                thread: 65_536
            }
        );
    }

    #[test]
    fn malformed_caps_are_refused() {
        assert!(parse_model_caps("").unwrap().is_empty());
        assert!(parse_model_caps("model").is_err());
        assert!(parse_model_caps("=12").is_err());
        assert!(parse_model_caps("m=lots").is_err());
    }
}
