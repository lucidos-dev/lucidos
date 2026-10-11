//! Memory views and the module switch against a real Postgres.
//!
//! - I5: one snapshot, one byte string.
//! - I6: no view exceeds its budget.
//! - I7: a turn never waits on the compactor, and Classic holds until the
//!   workspace's trees are ready.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::seed;
use crate::engine::event_bus::EventBus;
use crate::engine::summary_tree::consumer::switched_to;
use crate::engine::summary_tree::log::{EntryKind, LogEntry};
use crate::engine::summary_tree::module::{tree_ready, MemoryModule, Surface, ViewBudgets};
use crate::engine::summary_tree::shape::complete_addresses;
use crate::engine::summary_tree::store::{self, StoredNode};
use crate::engine::summary_tree::view::{
    coding_agent_section, render_thread_view, rungs, thread_view, ViewBlocks, ViewSnapshots,
    WorkspaceView, BLOCK_LINES, LOOKBACK_BLOCKS, RECENT_LINES, THREAD_RUNGS,
};
use crate::engine::summary_tree::{NodeAddr, SummaryScope};
use crate::engine::thread_events::EventChannel;
use crate::test_support::{setup_test_db, teardown_test_db};

/// A built workspace tree over `len` leaves, each line `bytes` long.
async fn build_workspace(pool: &PgPool, len: u64, bytes: usize) {
    for addr in complete_addresses(len) {
        insert_workspace_node(pool, addr, bytes).await;
    }
}

async fn insert_workspace_node(pool: &PgPool, addr: NodeAddr, bytes: usize) {
    let label = format!("turn {addr} ");
    let text = format!("{label}{}", "x".repeat(bytes.saturating_sub(label.len())));
    store::insert_node(
        pool,
        SummaryScope::Workspace,
        addr,
        &StoredNode {
            text,
            model: None,
            source_event_id: addr.is_leaf().then(Uuid::new_v4),
            source_thread_id: None,
        },
    )
    .await
    .expect("insert node");
}

/// Settle one more workspace turn at the end, merges included.
async fn grow_workspace(pool: &PgPool, len: u64, bytes: usize) {
    for addr in complete_addresses(len + 1) {
        if addr.end() == len + 1 {
            insert_workspace_node(pool, addr, bytes).await;
        }
    }
}

fn view_bytes(view: &WorkspaceView) -> usize {
    view.snapshot.text.len() + view.recent.len()
}

/// I5: another thread's settled turn reaches this thread as a recent line,
/// and the snapshot every thread of the budget shares keeps its bytes.
#[tokio::test]
async fn a_settled_turn_rides_the_recent_block_and_leaves_the_snapshot_alone() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace(&pool, 20, 200).await;
    let snapshots = ViewSnapshots::default();

    let first = snapshots.workspace_view(&pool, 16_384).await.unwrap();
    assert!(first.snapshot.text.contains("[w/"), "{first:?}");
    assert!(first.recent.is_empty());
    assert_eq!(
        first.snapshot.marks,
        first
            .snapshot
            .ends
            .last()
            .copied()
            .into_iter()
            .collect::<Vec<_>>(),
        "the snapshot marks its last whole block"
    );

    grow_workspace(&pool, 20, 200).await;
    let second = snapshots.workspace_view(&pool, 16_384).await.unwrap();
    assert_eq!(
        second.snapshot, first.snapshot,
        "the snapshot is one byte string"
    );
    assert!(second.recent.contains("[w/20+1]"), "{}", second.recent);

    teardown_test_db(&db_name).await;
}

/// The snapshot rolls over once the recent block passes its cap, and a fresh
/// epoch starts with no recent lines.
#[tokio::test]
async fn the_snapshot_rolls_over_when_the_recent_block_passes_its_cap() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace(&pool, 8, 300).await;
    let snapshots = ViewSnapshots::default();
    let budget = 4_096;

    let first = snapshots.workspace_view(&pool, budget).await.unwrap();
    let mut rolled = None;
    for len in 8..40 {
        grow_workspace(&pool, len, 300).await;
        let view = snapshots.workspace_view(&pool, budget).await.unwrap();
        assert!(view_bytes(&view) <= budget, "{} bytes", view_bytes(&view));
        if view.snapshot != first.snapshot {
            rolled = Some(view);
            break;
        }
        assert!(view.recent.len() <= budget / 2);
    }
    let view = rolled.expect("the snapshot rolled over");
    assert!(view.recent.is_empty(), "{}", view.recent);

    teardown_test_db(&db_name).await;
}

/// Short lines roll the snapshot over by count before they reach its byte cap.
/// So a rollover appends fewer blocks than a lookback, and the new snapshot's
/// rung still finds the old one in the cache.
#[tokio::test]
async fn the_snapshot_rolls_over_before_its_rung_leaves_the_lookback() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace(&pool, 8, 40).await;
    let snapshots = ViewSnapshots::default();
    let first = snapshots.workspace_view(&pool, 65_536).await.unwrap();
    let mut rolled = false;
    for len in 8..(8 + RECENT_LINES as u64 + 2) {
        grow_workspace(&pool, len, 40).await;
        let view = snapshots.workspace_view(&pool, 65_536).await.unwrap();
        assert!(view.recent.matches("[w/").count() <= RECENT_LINES);
        rolled |= view.snapshot != first.snapshot;
    }
    assert!(rolled, "the snapshot never rolled over");
    teardown_test_db(&db_name).await;
}

/// I6: snapshot and recent block together stay within every budget.
#[tokio::test]
async fn no_workspace_view_exceeds_its_budget() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace(&pool, 64, 480).await;
    for budget in [512, 2_048, 16_384, 65_536] {
        let view = ViewSnapshots::default()
            .workspace_view(&pool, budget)
            .await
            .unwrap();
        assert!(
            view_bytes(&view) <= budget,
            "budget {budget}: {}",
            view_bytes(&view)
        );
    }
    let off = ViewSnapshots::default()
        .workspace_view(&pool, 0)
        .await
        .unwrap();
    assert_eq!(off, WorkspaceView::default());
    teardown_test_db(&db_name).await;
}

/// A delete before the snapshot's end moves its last leaf, which retires it.
#[tokio::test]
async fn a_moved_leaf_retires_the_snapshot() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace(&pool, 6, 100).await;
    let snapshots = ViewSnapshots::default();
    let first = snapshots.workspace_view(&pool, 16_384).await.unwrap();

    sqlx::query(
        "UPDATE summary_tree_nodes SET source_event_id = gen_random_uuid(), text = 'moved up' \
         WHERE scope = 'workspace' AND span = 1 AND start = 5",
    )
    .execute(&pool)
    .await
    .unwrap();
    let second = snapshots.workspace_view(&pool, 16_384).await.unwrap();
    assert_ne!(second.snapshot, first.snapshot);
    assert!(second.snapshot.text.contains("moved up"));
    teardown_test_db(&db_name).await;
}

/// I7: with nothing built, the thread view reads every entry from its event,
/// leaves the turn's own message to the request line, and keeps its budget.
#[tokio::test]
async fn an_unbuilt_thread_view_reads_its_entries_from_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    seed(
        &bus,
        thread,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": "the door is blue", "mode": "human"}),
            json!({"type": "ResponseGenerated", "text": "Noted: blue."}),
            json!({"type": "MessageReceived", "text": "what colour is the door?", "mode": "human"}),
        ],
    )
    .await;

    let view = thread_view(&pool, thread, 65_536, "what colour is the door?")
        .await
        .unwrap()
        .text;
    assert!(
        view.contains("[0+1] prompt (human): the door is blue"),
        "{view}"
    );
    assert!(view.contains("[1+1] response: Noted: blue."), "{view}");
    assert!(
        !view.contains("what colour"),
        "the turn's message is the request line"
    );

    let tight = thread_view(&pool, thread, 200, "what colour is the door?")
        .await
        .unwrap()
        .text;
    assert!(tight.len() <= 200, "{} bytes", tight.len());
    teardown_test_db(&db_name).await;
}

/// I7: an untouched workspace, and a Tree one whose backfill has not
/// completed, both run Classic. Only a ready Tree workspace takes the views.
#[tokio::test]
async fn a_workspace_stays_on_classic_until_its_trees_are_ready() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    assert_eq!(MemoryModule::chosen(&pool).await, MemoryModule::Classic);
    assert!(!tree_ready(&pool).await);

    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        "tree",
        None,
    )
    .await
    .unwrap();
    assert_eq!(MemoryModule::chosen(&pool).await, MemoryModule::Tree);
    assert!(
        !tree_ready(&pool).await,
        "not ready before the ready flag sets"
    );

    store::mark_ready(&pool).await.unwrap();
    assert!(tree_ready(&pool).await);
    teardown_test_db(&db_name).await;
}

/// The compactor follows the preference: an untouched workspace boots with
/// none, and a change of `memory_module` starts or stops it.
#[tokio::test]
async fn the_compactor_runs_only_while_the_module_is_tree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    assert_eq!(MemoryModule::chosen(&pool).await, MemoryModule::Classic);
    for (value, module) in [
        ("tree", MemoryModule::Tree),
        ("classic", MemoryModule::Classic),
    ] {
        crate::core::PreferenceStore::set(
            &pool,
            &bus,
            crate::core::prefs::MEMORY_MODULE.key(),
            value,
            None,
        )
        .await
        .unwrap();
        let switched = loop {
            let emitted = rx.recv().await.expect("an event");
            if let Some(module) = switched_to(&emitted) {
                break module;
            }
        };
        assert_eq!(switched, module);
    }
    teardown_test_db(&db_name).await;
}

/// I15 on Classic: an empty view adds nothing to a coding agent's prompt.
#[test]
fn an_empty_view_adds_nothing_to_a_coding_agents_prompt() {
    assert_eq!(coding_agent_section(&WorkspaceView::default()), "");
    let view = WorkspaceView {
        snapshot: ViewBlocks {
            text: "[WORKSPACE MEMORY VIEW]\n[w/0+1] turn\n[END WORKSPACE MEMORY VIEW]\n".into(),
            ..ViewBlocks::default()
        },
        recent: String::new(),
    };
    let section = coding_agent_section(&view);
    assert!(section.starts_with("\n\n[WORKSPACE MEMORY VIEW]"));
    assert!(section.contains("lucidos recall zoom --id"));
}

/// A thread log of `len` entries with every node built, each line about
/// `line` bytes.
fn built_thread(
    len: u64,
    line: usize,
) -> (
    Vec<LogEntry>,
    std::collections::HashMap<NodeAddr, StoredNode>,
) {
    let entries: Vec<LogEntry> = (0..len)
        .map(|i| LogEntry {
            kind: EntryKind::Prompt(crate::engine::thread_events::ActorMode::Human),
            text: format!("entry {i}"),
            event_id: Uuid::new_v4(),
        })
        .collect();
    let nodes = complete_addresses(len)
        .into_iter()
        .map(|addr| {
            let label = format!("summary of {addr} ");
            let node = StoredNode {
                text: format!("{label}{}", "y".repeat(line.saturating_sub(label.len()))),
                model: None,
                source_event_id: addr
                    .is_leaf()
                    .then(|| entries[addr.start as usize].event_id),
                source_thread_id: None,
            };
            (addr, node)
        })
        .collect();
    (entries, nodes)
}

/// The thread view goes in blocks, each ending at a line end, so a cut splits
/// no line. Its marks are the last whole block and the one a lookback before.
#[test]
fn thread_view_marks_fall_on_block_ends() {
    let budget = 65_536;
    let (entries, nodes) = built_thread(1_000, 200);
    let view = render_thread_view(&entries, &nodes, budget);
    assert!(view.text.len() <= budget, "{} bytes", view.text.len());
    let mut from = 0;
    for (i, &end) in view.ends.iter().enumerate() {
        assert!(view.text[..end].ends_with('\n'), "block {i} ends no line");
        assert!(view.text[end..].starts_with('['), "block {i} ends mid-line");
        let lines = view.text[from..end]
            .lines()
            .filter(|l| l.starts_with('['))
            .count();
        // The first block also holds the frame's opening line.
        let expected = BLOCK_LINES + usize::from(i == 0);
        assert_eq!(lines, expected, "block {i}");
        from = end;
    }
    let last = view.ends.len() - 1;
    assert!(last > LOOKBACK_BLOCKS, "{} blocks", view.ends.len());
    assert_eq!(
        view.marks,
        vec![view.ends[last - LOOKBACK_BLOCKS], view.ends[last]]
    );
    let pieces = view.pieces();
    assert_eq!(pieces.iter().map(|p| p.0).collect::<String>(), view.text);
    assert_eq!(pieces.iter().filter(|p| p.1).count(), THREAD_RUNGS);
}

/// A view shorter than one block has nothing to mark.
#[test]
fn a_short_thread_view_has_no_block_to_mark() {
    let (entries, nodes) = built_thread(3, 40);
    let view = render_thread_view(&entries, &nodes, 65_536);
    assert!(view.text.contains("[2+1]"), "{}", view.text);
    assert!(view.marks.is_empty(), "{:?}", view.marks);
    assert_eq!(view.pieces(), vec![(view.text.as_str(), false)]);
}

/// The rungs: the last block, then every lookback before it, never block 0.
#[test]
fn the_rungs_climb_back_from_the_last_block() {
    assert_eq!(rungs(0, 2), Vec::<usize>::new());
    assert_eq!(rungs(3, 2), vec![3]);
    assert_eq!(rungs(LOOKBACK_BLOCKS, 2), vec![LOOKBACK_BLOCKS]);
    assert_eq!(rungs(LOOKBACK_BLOCKS + 1, 2), vec![1, LOOKBACK_BLOCKS + 1]);
    assert_eq!(rungs(50, 2), vec![50 - LOOKBACK_BLOCKS, 50]);
    assert_eq!(rungs(50, 1), vec![50]);
}

/// Unbuilt entries past the built prefix ride after its lines as raw text.
/// While they fit the reserve, more of them leave every line of the built prefix alone.
#[test]
fn the_unbuilt_tail_leaves_the_built_prefix_alone() {
    let budget = 16_384;
    let (entries, mut nodes) = built_thread(306, 200);
    nodes.retain(|addr, _| addr.end() <= 290);
    let view = render_thread_view(&entries[..300], &nodes, budget);
    assert!(view.text.len() <= budget, "{} bytes", view.text.len());
    assert!(
        view.text.contains("[299+1] prompt (human): entry 299"),
        "{}",
        view.text
    );

    let next = render_thread_view(&entries, &nodes, budget);
    assert!(
        next.text.contains("[305+1] prompt (human): entry 305"),
        "{}",
        next.text
    );
    let prefix_lines = view.text.find("[290+1]").expect("the tail starts at 290");
    assert_eq!(next.text[..prefix_lines], view.text[..prefix_lines]);
}

/// With the Tree module ready and the coding-agent preference left unset, a
/// coding agent's appended prompt carries no workspace view.
#[tokio::test]
async fn an_unset_coding_agent_preference_adds_no_workspace_view() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    build_workspace(&pool, 20, 200).await;
    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        "tree",
        None,
    )
    .await
    .unwrap();
    store::mark_ready(&pool).await.unwrap();
    assert!(tree_ready(&pool).await);

    let budgets = ViewBudgets::resolve(&pool, Surface::CodingAgent, None).await;
    assert_eq!(
        budgets.workspace,
        crate::core::prefs::WORKSPACE_VIEW_BYTES_CODING_AGENT.default_number() as usize
    );
    let view = ViewSnapshots::default()
        .workspace_view(&pool, budgets.workspace)
        .await
        .unwrap();
    assert_eq!(coding_agent_section(&view), "");

    teardown_test_db(&db_name).await;
}

/// With the preference set to a positive value, the workspace view carries
/// through to the coding agent's appended prompt.
#[tokio::test]
async fn a_positive_coding_agent_preference_adds_the_workspace_view() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    build_workspace(&pool, 20, 200).await;
    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        "tree",
        None,
    )
    .await
    .unwrap();
    store::mark_ready(&pool).await.unwrap();
    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::WORKSPACE_VIEW_BYTES_CODING_AGENT.key(),
        "16384",
        None,
    )
    .await
    .unwrap();

    let budgets = ViewBudgets::resolve(&pool, Surface::CodingAgent, None).await;
    assert_eq!(budgets.workspace, 16_384);
    let view = ViewSnapshots::default()
        .workspace_view(&pool, budgets.workspace)
        .await
        .unwrap();
    let section = coding_agent_section(&view);
    assert!(
        section.starts_with("\n\n[WORKSPACE MEMORY VIEW]"),
        "{section}"
    );

    teardown_test_db(&db_name).await;
}
