//! The compactor against a real Postgres:
//!
//! - I1: a rebuild lands on the same tree.
//! - I3: every leaf is reachable from the root, down to its event.
//! - I8: a restart resumes, and a failing node wedges nothing.
//! - I9: a delete leaves no words in any tree.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    age_events, deps, deps_with, line_response, seed, seed_artifact, EchoProvider, TestDeps,
    COMPACTOR_EFFORT, COMPACTOR_MODEL, TEST_LANE,
};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::summary_tree::compactor::{CompactionModel, Compactor, CompactorDeps, Drained};
use crate::engine::summary_tree::store::{self, StoredNode};
use crate::engine::summary_tree::workspace_log::WorkspaceLog;
use crate::engine::summary_tree::{
    fold, shape, BackfillProgress, NodeAddr, SummaryScope, HORIZON_SECS, NODE_BYTES, READY_DAYS,
};
use crate::engine::thread_events::EventChannel;
use crate::llm::provider::{LlmProvider, LlmResponse, Message};
use crate::test_support::{aux_captures, setup_test_db, teardown_test_db};

/// A marker only the chat thread says, for the delete test to hunt for.
const MARKER: &str = "zebraquilt";
const SHORT_USER_TEXT: &str = "remember the zebraquilt blue door";

struct Fixture {
    chat: Uuid,
    coding: Uuid,
    quiet_trigger: Uuid,
    home: Uuid,
}

/// A chat thread at full fidelity, a coding-agent thread, a trigger run nobody
/// talked to, and two artifact writes, one of them a binary.
///
/// The hidden home thread comes first. An artifact leaf's call records there,
/// so a build would otherwise create it midway and change the tree count.
async fn seed_workspace(pool: &PgPool, bus: &EventBus) -> Fixture {
    let home = crate::engine::home_thread::ensure_home_thread(bus, pool)
        .await
        .expect("create the home thread");
    let chat = Uuid::new_v4();
    seed(
        bus,
        chat,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": SHORT_USER_TEXT, "mode": "human"}),
            json!({"type": "ToolCalled", "name": "read_file", "args": {"path": "door.md"}}),
            json!({"type": "ToolResult", "name": "read_file",
                   "result": format!("{MARKER} {}", "the door is blue. ".repeat(50))}),
            json!({"type": "TextStreamed", "text": format!("The {MARKER} door is blue. {}", "Noted. ".repeat(90))}),
            json!({"type": "ResponseGenerated", "text": format!("The {MARKER} door is blue. {}", "Noted. ".repeat(90))}),
            json!({"type": "MessageReceived", "mode": "human",
                   "text": format!("OVERSHOOT {MARKER} {}", "a long paste. ".repeat(50))}),
            json!({"type": "ResponseGenerated", "text": "ok"}),
        ],
    )
    .await;

    let coding = Uuid::new_v4();
    seed(
        bus,
        coding,
        EventChannel::ClaudeCode,
        vec![
            json!({"type": "SessionStarted", "session_id": "s1"}),
            json!({"type": "MessageReceived", "text": "fix the parser", "mode": "human"}),
            json!({"type": "CodingAgentTextStreamed", "text": "Looking at the parser."}),
            json!({"type": "ResponseGenerated", "text": format!("Fixed the parser. {}", "Details. ".repeat(80))}),
            json!({"type": "CodingAgentIdled"}),
            json!({"type": "MessageReceived", "text": "now the docs", "mode": "human"}),
            json!({"type": "ResponseGenerated", "text": "Docs updated."}),
            json!({"type": "CodingAgentIdled"}),
        ],
    )
    .await;

    let quiet_trigger = Uuid::new_v4();
    seed(
        bus,
        quiet_trigger,
        EventChannel::Trigger,
        vec![
            json!({"type": "TriggerStarted", "trigger_id": "t1", "prompt": "check the mail"}),
            json!({"type": "ResponseGenerated", "text": "No new mail."}),
        ],
    )
    .await;

    seed_artifact(bus, "artifacts/notes/plan.md", "c1", None).await;
    seed_artifact(bus, "artifacts/photos/door.png", "c2", None).await;
    age_events(pool).await;
    Fixture {
        chat,
        coding,
        quiet_trigger,
        home,
    }
}

/// Seed and drain until the queue is empty.
async fn run_to_rest(compactor: &Arc<Compactor>) -> Vec<(SummaryScope, Drained)> {
    compactor.seed().await.expect("seed");
    let mut outcomes = Vec::new();
    loop {
        let round = compactor.drain_queue().await;
        if round.is_empty() {
            return outcomes;
        }
        outcomes.extend(round);
    }
}

fn compactor(pool: &PgPool, bus: &EventBus, deps: Arc<dyn CompactorDeps>) -> Arc<Compactor> {
    Compactor::new(pool.clone(), bus.clone(), deps)
}

type Snapshot = BTreeSet<(String, i64, i64, Option<Uuid>, String)>;

async fn snapshot(pool: &PgPool) -> Snapshot {
    let rows: Vec<(String, i64, i64, Option<Uuid>, String)> =
        sqlx::query_as("SELECT scope, start, span, source_event_id, text FROM summary_tree_nodes")
            .fetch_all(pool)
            .await
            .unwrap();
    rows.into_iter().collect()
}

async fn drop_every_node(pool: &PgPool) {
    sqlx::query("DELETE FROM summary_tree_nodes")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM summary_tree_scopes")
        .execute(pool)
        .await
        .unwrap();
}

async fn thread_len(pool: &PgPool, id: Uuid) -> u64 {
    let info = store::thread_in_scope(pool, id).await.unwrap().unwrap();
    store::load_thread_log(pool, id, info.kind)
        .await
        .unwrap()
        .log
        .entries
        .len() as u64
}

async fn workspace_log(pool: &PgPool) -> WorkspaceLog {
    let mut log = WorkspaceLog::default();
    log.extend(pool).await.unwrap();
    log
}

/// The scope holds exactly the nodes a complete tree over `len` entries has.
async fn assert_complete(pool: &PgPool, scope: SummaryScope, len: u64) {
    let nodes = store::load_nodes(pool, scope).await.unwrap();
    let have: BTreeSet<NodeAddr> = nodes.keys().copied().collect();
    let want: BTreeSet<NodeAddr> = shape::complete_addresses(len).into_iter().collect();
    assert_eq!(
        have,
        want,
        "{} is not a complete tree over {len}",
        scope.as_db()
    );
}

async fn assert_every_scope_complete(pool: &PgPool, fixture: &Fixture) {
    for id in [fixture.chat, fixture.coding, fixture.home] {
        let len = thread_len(pool, id).await;
        assert_complete(pool, SummaryScope::Thread(id), len).await;
    }
    let len = workspace_log(pool).await.leaves.len() as u64;
    assert_complete(pool, SummaryScope::Workspace, len).await;
}

async fn dirty_ids(pool: &PgPool) -> Vec<Uuid> {
    store::dirty_threads(pool, READY_DAYS)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect()
}

async fn texts_containing(pool: &PgPool, needle: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT scope || ' ' || text FROM summary_tree_nodes WHERE text ILIKE $1")
        .bind(format!("%{needle}%"))
        .fetch_all(pool)
        .await
        .unwrap()
}

// ── I1: events stay the authority ─────────────────────────────────────

/// Every node is a projection: drop them all, rebuild, and the same nodes
/// come back at the same addresses over the same sources.
#[tokio::test]
async fn dropping_every_node_and_rebuilding_lands_on_the_same_tree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;

    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(store::is_ready(&pool).await.unwrap());
    let first = snapshot(&pool).await;

    drop_every_node(&pool).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_eq!(snapshot(&pool).await, first);

    teardown_test_db(&db_name).await;
}

/// The benchmark's wait between tasks: caught up only once the workspace
/// tree and every thread tree reflect their newest events, and no longer once
/// another arrives.
#[tokio::test]
async fn the_workspace_is_caught_up_only_once_its_newest_leaf_is_reflected() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    assert!(!store::caught_up(&pool).await.unwrap());

    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.mark_dirty(SummaryScope::Workspace, false);
    c.drain_queue().await;
    assert!(
        !store::caught_up(&pool).await.unwrap(),
        "the workspace tree is built, the thread trees are not"
    );
    run_to_rest(&c).await;
    assert!(store::caught_up(&pool).await.unwrap());

    seed_artifact(&bus, "artifacts/notes/later.md", "c3", None).await;
    assert!(!store::caught_up(&pool).await.unwrap());
    age_events(&pool).await;
    run_to_rest(&c).await;
    assert!(store::caught_up(&pool).await.unwrap());

    teardown_test_db(&db_name).await;
}

/// The workspace log holds one leaf per settled turn with content and one per
/// readable artifact write. A trigger run nobody talked to has no tree.
#[tokio::test]
async fn the_workspace_log_holds_turns_and_artifact_writes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;

    let log = workspace_log(&pool).await;
    let threads: Vec<Option<Uuid>> = log.leaves.iter().map(|l| l.thread_id()).collect();
    assert_eq!(
        threads,
        vec![
            Some(fixture.chat),
            Some(fixture.chat),
            Some(fixture.coding),
            Some(fixture.coding),
            None,
        ],
        "two chat turns, two coding turns (their empty idles skipped), one text artifact"
    );

    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    let quiet = store::load_nodes(&pool, SummaryScope::Thread(fixture.quiet_trigger))
        .await
        .unwrap();
    assert!(quiet.is_empty());

    teardown_test_db(&db_name).await;
}

/// A trigger thread joins once the owner writes in it. Only its turns after
/// that message reach the workspace log, so joining appends.
#[tokio::test]
async fn a_trigger_thread_joins_when_the_owner_writes_in_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let before = workspace_log(&pool).await.leaves.len();

    seed(
        &bus,
        fixture.quiet_trigger,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": "mark them read", "mode": "human"}),
            json!({"type": "ResponseGenerated", "text": "Marked."}),
        ],
    )
    .await;
    age_events(&pool).await;

    let log = workspace_log(&pool).await;
    assert_eq!(log.leaves.len(), before + 1);
    assert_eq!(
        log.leaves.last().unwrap().thread_id(),
        Some(fixture.quiet_trigger)
    );
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_eq!(thread_len(&pool, fixture.quiet_trigger).await, 4);
    assert_complete(&pool, SummaryScope::Thread(fixture.quiet_trigger), 4).await;

    teardown_test_db(&db_name).await;
}

/// A leaf event younger than the horizon is left for later, and the drain
/// says so, so the compactor comes back for it.
#[tokio::test]
async fn a_leaf_younger_than_the_horizon_is_read_on_the_next_drain() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_artifact(&bus, "artifacts/notes/plan.md", "c1", None).await;

    let mut log = WorkspaceLog::default();
    assert!(
        log.extend(&pool).await.unwrap(),
        "the young write is pending"
    );
    assert!(log.leaves.is_empty());

    age_events(&pool).await;
    assert!(!log.extend(&pool).await.unwrap());
    assert_eq!(log.leaves.len(), 1);
    assert!(!log.extend(&pool).await.unwrap());
    assert_eq!(log.leaves.len(), 1, "a leaf is read once");

    teardown_test_db(&db_name).await;
}

// ── I3: nothing is lost ───────────────────────────────────────────────

/// Collect the leaves under `addr`, zooming one level at a time.
fn zoom_to_leaves(addr: NodeAddr, nodes: &HashMap<NodeAddr, StoredNode>, out: &mut Vec<u64>) {
    assert!(
        nodes.contains_key(&addr),
        "zoom reached {addr}, which is not built"
    );
    match addr.children() {
        None => out.push(addr.start),
        Some((a, b)) => {
            zoom_to_leaves(a, nodes, out);
            zoom_to_leaves(b, nodes, out);
        }
    }
}

/// From the coarsest lines covering the log, zooming reaches every leaf, and
/// every leaf names the event it came from. A short user turn reads verbatim.
#[tokio::test]
async fn every_leaf_is_reachable_from_the_root_down_to_its_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;

    let scopes = [
        SummaryScope::Thread(fixture.chat),
        SummaryScope::Thread(fixture.coding),
        SummaryScope::Workspace,
    ];
    for scope in scopes {
        let nodes = store::load_nodes(&pool, scope).await.unwrap();
        let len = nodes.keys().filter(|a| a.is_leaf()).count() as u64;
        let texts: HashMap<NodeAddr, String> =
            nodes.iter().map(|(a, n)| (*a, n.text.clone())).collect();
        let mut reached = Vec::new();
        for root in fold::built_cover(len, |a| texts.get(&a).map(String::len)) {
            zoom_to_leaves(root, &nodes, &mut reached);
        }
        assert_eq!(reached, (0..len).collect::<Vec<_>>(), "{}", scope.as_db());

        for (addr, node) in &nodes {
            assert!(node.text.len() <= NODE_BYTES, "{addr} is over the size");
            if !addr.is_leaf() {
                continue;
            }
            let source = node.source_event_id.expect("a leaf names its event");
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE id = $1)")
                    .bind(source)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(exists, "{addr} names an event that is not there");
        }
    }

    // The thread tree's leaves are the log's entries, in order, and a short
    // user message is its own node word for word.
    let info = store::thread_in_scope(&pool, fixture.chat)
        .await
        .unwrap()
        .unwrap();
    let log = store::load_thread_log(&pool, fixture.chat, info.kind)
        .await
        .unwrap()
        .log;
    let nodes = store::load_nodes(&pool, SummaryScope::Thread(fixture.chat))
        .await
        .unwrap();
    for (i, entry) in log.entries.iter().enumerate() {
        let leaf = &nodes[&NodeAddr::leaf(i as u64)];
        assert_eq!(leaf.source_event_id, Some(entry.event_id));
        if leaf.model.is_none() {
            assert_eq!(leaf.text, entry.message());
        }
    }
    assert_eq!(
        nodes[&NodeAddr::leaf(0)].text,
        format!("user: {SHORT_USER_TEXT}")
    );

    // A workspace turn leaf zooms on into its thread's tree.
    let workspace = store::load_nodes(&pool, SummaryScope::Workspace)
        .await
        .unwrap();
    let turn = &workspace[&NodeAddr::leaf(0)];
    assert_eq!(turn.source_thread_id, Some(fixture.chat));
    assert_eq!(
        log.turn(turn.source_event_id.unwrap()).unwrap().entries,
        0..4
    );

    teardown_test_db(&db_name).await;
}

/// A line over the limit gets a second round in the same conversation, and
/// every round is recorded for token accounting.
#[tokio::test]
async fn an_overlong_line_is_asked_again_and_every_call_is_recorded() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let provider = Arc::new(EchoProvider::new());
    let deps = Arc::new(TestDeps {
        provider: Some(provider.clone()),
        artifacts: vec![],
    });
    run_to_rest(&compactor(&pool, &bus, deps)).await;

    let nodes = store::load_nodes(&pool, SummaryScope::Thread(fixture.chat))
        .await
        .unwrap();
    let overshoot = &nodes[&NodeAddr::leaf(4)];
    assert!(overshoot.text.starts_with("sum["), "{}", overshoot.text);
    assert_eq!(overshoot.model.as_deref(), Some(COMPACTOR_MODEL));

    let written = nodes.values().filter(|n| n.model.is_some()).count();
    let captures = aux_captures(&pool, fixture.chat, "summary_compaction").await;
    assert!(
        captures.len() > written,
        "{} captures for {written} written nodes, one of which took two rounds",
        captures.len()
    );
    for capture in &captures {
        assert_eq!(capture["model"], COMPACTOR_MODEL, "{capture}");
    }

    // Every round names the selected model and tier, so a router sends it to
    // whichever backend serves that model rather than to its own default.
    let selections = provider.selections.lock().unwrap().clone();
    assert!(!selections.is_empty());
    for (model, effort) in selections.iter() {
        assert_eq!(model.as_deref(), Some(COMPACTOR_MODEL));
        assert_eq!(effort.as_deref(), Some(COMPACTOR_EFFORT));
    }

    teardown_test_db(&db_name).await;
}

/// A workspace node with no thread inside it, an artifact leaf or a merge of
/// them, still records its model call: on the home thread. Eight leaves make
/// the top merge outgrow a node, so it calls the model too.
#[tokio::test]
async fn an_artifact_only_build_records_every_call_on_the_home_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let notes = format!("# Notes\n{}", "A line of the notes. ".repeat(40));
    let mut artifacts = Vec::new();
    for i in 0..8 {
        seed_artifact(&bus, &format!("artifacts/notes/n{i}.md"), "c1", None).await;
        artifacts.push((format!("notes/n{i}.md"), notes.clone()));
    }
    age_events(&pool).await;
    let deps = Arc::new(TestDeps {
        provider: Some(Arc::new(EchoProvider::new())),
        artifacts,
    });
    run_to_rest(&compactor(&pool, &bus, deps)).await;

    let nodes = store::load_nodes(&pool, SummaryScope::Workspace)
        .await
        .unwrap();
    let written = nodes.values().filter(|n| n.model.is_some()).count();
    assert_eq!(written, 9, "eight artifact leaves and the top merge");
    let home: Uuid = sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_one(&pool)
        .await
        .expect("a home thread to record on");
    let captures = aux_captures(&pool, home, "summary_compaction").await;
    assert_eq!(
        captures.len(),
        written,
        "one capture per model-written node"
    );

    teardown_test_db(&db_name).await;
}

/// An artifact leaf records on the thread that wrote it. A writer that is no
/// longer here falls back to the home thread, so no row lands on it.
#[tokio::test]
async fn an_artifact_leaf_records_on_its_writer_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let writer = Uuid::new_v4();
    seed(
        &bus,
        writer,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": "save the notes", "mode": "human"}),
            json!({"type": "ResponseGenerated", "text": "Saved."}),
        ],
    )
    .await;
    seed_artifact(&bus, "artifacts/notes/plan.md", "c1", Some(writer)).await;
    seed_artifact(&bus, "artifacts/notes/later.md", "c2", Some(Uuid::new_v4())).await;
    age_events(&pool).await;
    let notes = format!("# Notes\n{}", "A line of the notes. ".repeat(40));
    let deps = Arc::new(TestDeps {
        provider: Some(Arc::new(EchoProvider::new())),
        artifacts: vec![
            ("notes/plan.md".to_string(), notes.clone()),
            ("notes/later.md".to_string(), notes),
        ],
    });
    run_to_rest(&compactor(&pool, &bus, deps)).await;

    let leaves = workspace_log(&pool).await.leaves;
    let anchors: Vec<Option<Uuid>> = leaves.iter().map(|l| l.capture_thread()).collect();
    assert_eq!(anchors, vec![Some(writer), Some(writer), None]);
    // The writer's own short turn fits a node, so its one row is the plan's.
    let on_writer = aux_captures(&pool, writer, "summary_compaction").await;
    assert_eq!(on_writer.len(), 1);
    let on_home = aux_captures(&pool, home, "summary_compaction").await;
    assert_eq!(on_home.len(), 1, "the later notes' writer is gone");

    teardown_test_db(&db_name).await;
}

/// The cached log forgets a deleted writer, and only that one.
#[test]
fn a_deleted_writer_is_forgotten_from_its_leaves() {
    use crate::engine::summary_tree::workspace_log::WorkspaceLeaf;
    let (kept, deleted) = (Uuid::new_v4(), Uuid::new_v4());
    let artifact = |writer| WorkspaceLeaf::Artifact {
        event_id: Uuid::new_v4(),
        path: "notes/plan.md".to_string(),
        commit: "c1".to_string(),
        verb: "created",
        writer_thread_id: Some(writer),
    };
    let mut leaves = [artifact(kept), artifact(deleted)];
    for leaf in &mut leaves {
        leaf.forget_writer_in(&[deleted].into_iter().collect());
    }
    let anchors: Vec<Option<Uuid>> = leaves.iter().map(|l| l.capture_thread()).collect();
    assert_eq!(anchors, vec![Some(kept), None]);
    assert!(
        leaves.iter().all(|l| l.thread_id().is_none()),
        "a writer never makes a leaf its thread's, so a delete keeps it"
    );
}

/// With no model, only free nodes are built, and the drain says why.
#[tokio::test]
async fn without_a_model_only_free_nodes_are_built() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let none = Arc::new(TestDeps {
        provider: None,
        artifacts: Vec::new(),
    });
    let c = compactor(&pool, &bus, none);
    c.seed().await.unwrap();
    let outcomes = c.drain_queue().await;
    assert!(outcomes.contains(&(
        SummaryScope::Thread(fixture.chat),
        Drained::ModelUnavailable
    )));
    let nodes = store::load_nodes(&pool, SummaryScope::Thread(fixture.chat))
        .await
        .unwrap();
    assert!(nodes.values().all(|n| n.model.is_none()));
    assert!(nodes.contains_key(&NodeAddr::leaf(0)));
    assert!(!store::is_ready(&pool).await.unwrap());

    teardown_test_db(&db_name).await;
}

// ── I8: compaction is durable and resumable ───────────────────────────

/// Stop the compactor between nodes, start a new one, and it finishes the
/// same tree: no duplicates, no gaps, nothing built twice differently.
#[tokio::test]
async fn a_restart_mid_build_resumes_without_duplicates_or_gaps() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;

    let first = compactor(&pool, &bus, deps(EchoProvider::failing_after(2)));
    first.seed().await.unwrap();
    let outcomes = first.drain_queue().await;
    assert!(outcomes.iter().any(|(_, o)| *o == Drained::Failed));
    assert!(!store::is_ready(&pool).await.unwrap());
    let partial = snapshot(&pool).await;
    assert!(!partial.is_empty());
    drop(first);

    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(store::is_ready(&pool).await.unwrap());
    let resumed = snapshot(&pool).await;
    assert!(
        partial.is_subset(&resumed),
        "a node built before the stop changed"
    );

    drop_every_node(&pool).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_eq!(snapshot(&pool).await, resumed);

    teardown_test_db(&db_name).await;
}

/// A drained scope records its progress, so a restart re-queues only scopes
/// with newer events.
#[tokio::test]
async fn a_restart_requeues_only_scopes_with_newer_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    // Each written node left a capture on its thread, newer than the horizon.
    age_events(&pool).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert!(dirty_ids(&pool).await.is_empty());

    seed(
        &bus,
        fixture.coding,
        EventChannel::ClaudeCode,
        vec![json!({"type": "MessageReceived", "text": "and the tests", "mode": "human"})],
    )
    .await;
    assert_eq!(dirty_ids(&pool).await, vec![fixture.coding]);

    teardown_test_db(&db_name).await;
}

/// A node that keeps failing holds up its own scope and nothing else.
#[tokio::test]
async fn a_failing_node_never_wedges_the_queue() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let poisoned = Uuid::new_v4();
    seed(
        &bus,
        poisoned,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "mode": "human",
                   "text": format!("POISON {}", "pill ".repeat(200))}),
            json!({"type": "ResponseGenerated", "text": "ok"}),
        ],
    )
    .await;
    age_events(&pool).await;

    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.seed().await.unwrap();
    let outcomes: HashMap<SummaryScope, Drained> = c.drain_queue().await.into_iter().collect();
    assert_eq!(outcomes[&SummaryScope::Thread(poisoned)], Drained::Failed);
    for id in [fixture.chat, fixture.coding] {
        assert!(matches!(
            outcomes[&SummaryScope::Thread(id)],
            Drained::Complete
        ));
        let len = thread_len(&pool, id).await;
        assert_complete(&pool, SummaryScope::Thread(id), len).await;
    }
    // The workspace reads the poisoned turn's words too, so that leaf fails
    // there as well. Every other workspace leaf is built.
    assert_eq!(outcomes[&SummaryScope::Workspace], Drained::Failed);
    let workspace = store::load_nodes(&pool, SummaryScope::Workspace)
        .await
        .unwrap();
    assert_eq!(workspace.keys().filter(|a| a.is_leaf()).count(), 5);
    assert!(!store::is_ready(&pool).await.unwrap());

    teardown_test_db(&db_name).await;
}

// ── I9: delete removes content from every tree ────────────────────────

/// Delete the chat thread as the route does, inside one transaction. No node
/// holds its words afterwards, and the rebuilt workspace tree is exactly what
/// a rebuild from scratch writes.
#[tokio::test]
async fn a_deleted_thread_leaves_no_words_in_any_tree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let model = Arc::new(EchoProvider::new());
    let c = compactor(&pool, &bus, deps_with(model.clone()));
    run_to_rest(&c).await;
    assert!(!texts_containing(&pool, MARKER).await.is_empty());
    let calls_before = model.calls.load(Ordering::SeqCst);

    let mut tx = pool.begin().await.unwrap();
    store::delete_family(&mut tx, &[fixture.chat])
        .await
        .unwrap();
    for sql in [
        "DELETE FROM events WHERE thread_id = $1",
        "DELETE FROM thread_summaries WHERE thread_id = $1",
    ] {
        sqlx::query(sql)
            .bind(fixture.chat)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(texts_containing(&pool, MARKER).await, Vec::<String>::new());

    // What `ThreadsDeleted` tells the compactor.
    c.forget_threads(&[fixture.chat]);
    while !c.drain_queue().await.is_empty() {}
    // The leaves after the deleted ones moved up with their text: the
    // artifact leaf a model wrote cost no second call.
    assert_eq!(model.calls.load(Ordering::SeqCst), calls_before);
    assert_eq!(texts_containing(&pool, MARKER).await, Vec::<String>::new());
    let len = workspace_log(&pool).await.leaves.len() as u64;
    assert_eq!(len, 3, "two coding turns and one artifact remain");
    assert_complete(&pool, SummaryScope::Workspace, len).await;

    let after_delete = snapshot(&pool).await;
    drop_every_node(&pool).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    assert_eq!(snapshot(&pool).await, after_delete);

    teardown_test_db(&db_name).await;
}

/// The preflight counts the model-written workspace merges a delete sends
/// back, so the dialog can name the cost.
#[tokio::test]
async fn the_rebuild_count_names_the_model_written_merges_a_delete_drops() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;

    let expected: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM summary_tree_nodes \
         WHERE scope = 'workspace' AND span > 1 AND model IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    // The chat thread's turns are the first leaves, so every merge goes.
    let count = store::family_rebuild_count(&mut tx, &[fixture.chat])
        .await
        .unwrap();
    assert_eq!(count, expected);
    let none = store::family_rebuild_count(&mut tx, &[fixture.quiet_trigger])
        .await
        .unwrap();
    assert_eq!(none, 0);
    tx.rollback().await.unwrap();

    teardown_test_db(&db_name).await;
}

// ── The memory module switch ───────────────────────────────────────────

/// A paused compactor writes nothing, and a resume builds every scope and sets
/// the ready flag, so a Classic workspace makes no compactor call.
#[tokio::test]
async fn a_paused_compactor_builds_nothing_until_it_resumes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let provider = Arc::new(EchoProvider::new());
    let compactor = compactor(&pool, &bus, deps_with(provider.clone()));

    compactor.pause();
    run_to_rest(&compactor).await;
    assert!(
        snapshot(&pool).await.is_empty(),
        "a paused compactor wrote nodes"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(!store::is_ready(&pool).await.unwrap());

    compactor.resume().await;
    run_to_rest(&compactor).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(store::is_ready(&pool).await.unwrap());
    teardown_test_db(&db_name).await;
}

// ── The backfill count Settings shows ─────────────────────────────────

/// The backfill frames the bus carried, oldest first.
fn backfill_frames(
    rx: &mut tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
) -> Vec<SystemEvent> {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut frames = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(emitted) => {
                if let BusEvent::System(
                    frame @ (SystemEvent::TreeBackfillProgressed { .. }
                    | SystemEvent::TreeBackfillCompleted { .. }
                    | SystemEvent::TreeBackfillReset {}),
                ) = emitted.typed
                {
                    frames.push(frame);
                }
            }
            Err(TryRecvError::Lagged(_)) => continue,
            Err(_) => return frames,
        }
    }
}

fn progress_of(frames: &[SystemEvent]) -> Vec<BackfillProgress> {
    frames
        .iter()
        .filter_map(|f| match f {
            SystemEvent::TreeBackfillProgressed { progress } => Some(*progress),
            _ => None,
        })
        .collect()
}

/// The count covers every tree: three in-scope threads, the hidden home
/// thread among them, and the workspace. It never goes back. Every thread is
/// recent, so the ready flag waits on all four: one completion, then the
/// frame that counts the last tree.
#[tokio::test]
async fn the_backfill_counts_every_tree_and_completes_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let mut rx = bus.subscribe();
    let c = compactor(&pool, &bus, deps(EchoProvider::new()));

    run_to_rest(&c).await;
    let frames = backfill_frames(&mut rx);
    let progress = progress_of(&frames);
    assert_eq!(progress.first().map(|p| (p.done, p.total)), Some((0, 4)));
    for pair in progress.windows(2) {
        assert!(
            pair[0].done <= pair[1].done,
            "the count went back: {progress:?}"
        );
    }
    let (last, before) = progress.split_last().expect("frames");
    assert!(before
        .iter()
        .all(|p| p.done < p.total && !p.ready && !p.waiting_for_model));
    assert_eq!((last.done, last.total, last.ready), (4, 4, true));
    let completed: Vec<&SystemEvent> = frames
        .iter()
        .filter(|f| matches!(f, SystemEvent::TreeBackfillCompleted { .. }))
        .collect();
    assert_eq!(completed.len(), 1, "{frames:?}");
    assert!(matches!(
        &frames[frames.len() - 2..],
        [
            SystemEvent::TreeBackfillCompleted { total: 4 },
            SystemEvent::TreeBackfillProgressed { .. }
        ]
    ));
    assert!(store::is_ready(&pool).await.unwrap());
    assert_eq!(c.backfill_progress(), None);
    teardown_test_db(&db_name).await;
}

/// The bar moves inside a scope: each built node of a tree under way adds
/// to `done_milli` while `done` holds. It never goes back, and it ends full.
#[tokio::test]
async fn the_count_moves_within_a_scope_and_never_goes_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let mut rx = bus.subscribe();
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;

    let progress = progress_of(&backfill_frames(&mut rx));
    for pair in progress.windows(2) {
        assert!(
            pair[0].done_milli <= pair[1].done_milli,
            "the bar went back: {progress:?}"
        );
    }
    assert!(progress.iter().all(|p| p.nodes_done <= p.nodes_total));
    assert!(
        progress
            .windows(2)
            .any(|w| w[0].done == w[1].done && w[0].done_milli < w[1].done_milli),
        "no frame moved within a scope: {progress:?}"
    );
    let last = progress.last().expect("frames");
    assert_eq!(last.done_milli, last.total as u64 * 1000);
    assert_eq!((last.nodes_done, last.nodes_total), (0, 0));
    teardown_test_db(&db_name).await;
}

/// A failed node of an owed scope says the backfill is retrying, not that
/// it waits on a model. Other scopes completing meanwhile do not hide it.
#[tokio::test]
async fn the_count_says_when_a_failed_node_retries() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let mut rx = bus.subscribe();
    let c = compactor(&pool, &bus, deps(EchoProvider::failing_after(2)));

    c.seed().await.unwrap();
    let outcomes = c.drain_queue().await;
    assert!(outcomes.iter().any(|(_, o)| *o == Drained::Failed));
    let progress = c.backfill_progress().expect("a backfill runs");
    assert!(progress.retrying && !progress.waiting_for_model);
    assert!(progress_of(&backfill_frames(&mut rx))
        .iter()
        .any(|p| p.retrying));
    teardown_test_db(&db_name).await;
}

/// A workspace that built nodes and went back to Classic reads as started,
/// so returning to Tree needs no new confirm.
#[tokio::test]
async fn a_classic_workspace_with_nodes_reads_as_started() {
    use crate::engine::summary_tree::module::tree_backfill;
    use crate::engine::summary_tree::{Runtime, TreeBackfill};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;

    assert_eq!(
        tree_backfill(&pool, &Runtime::default()).await.unwrap(),
        TreeBackfill::Off { started: true }
    );
    teardown_test_db(&db_name).await;
}

/// A new compactor over a half-caught-up workspace counts what is already
/// done, so a restart never sends the bar back to zero.
#[tokio::test]
async fn a_reseed_counts_the_trees_already_caught_up_as_done() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    // Each written node left a capture on its thread, newer than the horizon.
    age_events(&pool).await;
    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;

    super::super::consumer::clear_ready_flag(&pool, &bus).await;
    seed(
        &bus,
        fixture.chat,
        EventChannel::Chat,
        vec![json!({"type": "MessageReceived", "text": "one more", "mode": "human"})],
    )
    .await;
    age_events(&pool).await;

    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.seed().await.unwrap();
    let progress = c.backfill_progress().expect("a backfill runs");
    assert_eq!(
        (progress.done, progress.total),
        (2, 4),
        "coding and home are done, chat and the workspace are owed"
    );
    teardown_test_db(&db_name).await;
}

/// Model resolution that fails until a test supplies one.
struct SwitchableDeps(std::sync::Mutex<Option<Arc<EchoProvider>>>);

#[async_trait::async_trait]
impl CompactorDeps for SwitchableDeps {
    async fn model(&self) -> Result<CompactionModel, String> {
        let provider = self
            .0
            .lock()
            .unwrap()
            .clone()
            .ok_or("no background model")?;
        Ok(CompactionModel {
            provider,
            model: COMPACTOR_MODEL.to_string(),
            effort: None,
            deadline: std::time::Duration::from_secs(5),
            lane: TEST_LANE.to_string(),
        })
    }

    fn read_artifact(&self, _path: &str, _commit: &str) -> Option<String> {
        None
    }
}

/// With no model the count says the backfill waits on one. Configuring a
/// model clears that and lets the backfill finish.
#[tokio::test]
async fn the_count_says_when_the_backfill_waits_on_a_model() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let mut rx = bus.subscribe();
    let switchable = Arc::new(SwitchableDeps(std::sync::Mutex::new(None)));
    let c = compactor(&pool, &bus, switchable.clone());

    c.seed().await.unwrap();
    c.drain_queue().await;
    assert!(c.backfill_progress().unwrap().waiting_for_model);
    assert!(progress_of(&backfill_frames(&mut rx))
        .iter()
        .any(|p| p.waiting_for_model));

    *switchable.0.lock().unwrap() = Some(Arc::new(EchoProvider::new()));
    run_to_rest(&c).await;
    let frames = backfill_frames(&mut rx);
    assert!(
        progress_of(&frames).iter().any(|p| !p.waiting_for_model),
        "{frames:?}"
    );
    assert!(store::is_ready(&pool).await.unwrap());
    teardown_test_db(&db_name).await;
}

/// Leaving Tree announces a reset only when it cleared a set ready flag.
#[tokio::test]
async fn clearing_the_ready_flag_announces_only_a_real_reset() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    super::super::consumer::clear_ready_flag(&pool, &bus).await;
    assert!(backfill_frames(&mut rx).is_empty());

    store::mark_ready(&pool).await.unwrap();
    super::super::consumer::clear_ready_flag(&pool, &bus).await;
    assert!(matches!(
        backfill_frames(&mut rx).as_slice(),
        [SystemEvent::TreeBackfillReset {}]
    ));
    assert!(!store::is_ready(&pool).await.unwrap());
    teardown_test_db(&db_name).await;
}

/// What `GET /api/v1/memory/tree-backfill` serves in each state: off on
/// Classic, running until the ready flag is set, then ready.
#[tokio::test]
async fn the_backfill_read_follows_the_module_and_the_ready_flag() {
    use crate::engine::summary_tree::module::tree_backfill;
    use crate::engine::summary_tree::{Runtime, TreeBackfill};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let runtime = Runtime::default();

    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Off { started: false }
    );
    store::mark_ready(&pool).await.unwrap();
    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Off { started: false },
        "Classic never reports ready"
    );
    super::super::consumer::clear_ready_flag(&pool, &bus).await;

    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        "tree",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Running {
            progress: BackfillProgress::default()
        }
    );
    store::mark_ready(&pool).await.unwrap();
    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Ready { filling: None }
    );
    teardown_test_db(&db_name).await;
}

/// The REST body and the SSE frame spell the progress the same way.
#[test]
fn the_progress_frame_carries_the_rest_shape() {
    use crate::engine::summary_tree::TreeBackfill;
    let progress = BackfillProgress {
        done: 2,
        total: 5,
        done_milli: 2_500,
        nodes_done: 3,
        nodes_total: 6,
        waiting_for_model: true,
        retrying: false,
        ready: false,
    };
    let rest = serde_json::to_value(TreeBackfill::Running { progress }).unwrap();
    let frame = serde_json::to_value(SystemEvent::TreeBackfillProgressed { progress }).unwrap();
    assert_eq!(rest["state"], "running");
    assert_eq!(rest["progress"], frame["data"]["progress"]);
}

/// The estimate reads the same trees the compactor builds: the two in-scope
/// threads, the workspace's turns and artifact writes. Building them shrinks
/// what is left to spend.
#[tokio::test]
async fn the_estimate_counts_the_trees_the_compactor_builds() {
    use crate::engine::summary_tree::estimate;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;

    let counts = estimate::counts(&pool).await.unwrap();
    assert_eq!(counts.threads.len(), 2, "the quiet trigger run has no tree");
    assert_eq!(
        counts.workspace.all.count,
        workspace_log(&pool).await.leaves.len() as u64
    );
    assert_eq!(counts.built, 0);
    let no_history = estimate::Measured::default();
    let before = estimate::estimate(
        &counts,
        &no_history,
        &crate::llm::model_registry::empty(),
        &crate::llm::model_registry::ProviderKind::ALL,
    );
    assert!(before.calls.low > 0 && before.calls.low <= before.calls.high);
    assert!(before
        .costs
        .iter()
        .all(|c| c.by_effort.iter().all(|e| e.backfill_usd.high > 0.0)));

    run_to_rest(&compactor(&pool, &bus, deps(EchoProvider::new()))).await;
    let after = estimate::estimate(
        &estimate::counts(&pool).await.unwrap(),
        &no_history,
        &crate::llm::model_registry::empty(),
        &crate::llm::model_registry::ProviderKind::ALL,
    );
    assert!(
        after.calls.high < before.calls.high,
        "{after:?} vs {before:?}"
    );
    teardown_test_db(&db_name).await;
}

// ── Speed: queue order, early ready, lanes ────────────────────────────

/// Engine stand-ins around any model, with the plan artifact [`deps`] holds.
struct ProviderDeps(Arc<dyn LlmProvider>);

#[async_trait::async_trait]
impl CompactorDeps for ProviderDeps {
    async fn model(&self) -> Result<CompactionModel, String> {
        Ok(CompactionModel {
            provider: self.0.clone(),
            model: COMPACTOR_MODEL.to_string(),
            effort: None,
            deadline: Duration::from_secs(30),
            lane: TEST_LANE.to_string(),
        })
    }

    fn read_artifact(&self, path: &str, _commit: &str) -> Option<String> {
        (path == "notes/plan.md").then(|| "A line of the plan. ".repeat(40))
    }
}

/// A model whose line hashes everything it was sent, context included. So two
/// builds agree only if every call read the same. Each call sleeps a delay
/// drawn from the hash and `salt`, so another salt returns in another order.
struct HashingProvider {
    salt: u64,
}

#[async_trait::async_trait]
impl LlmProvider for HashingProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for message in &messages {
            message.content.as_text().hash(&mut hasher);
        }
        system_prompt.hash(&mut hasher);
        let digest = hasher.finish();
        tokio::time::sleep(Duration::from_millis((digest ^ self.salt) % 20)).await;
        Ok(line_response(format!("line {digest:016x}")))
    }

    fn default_model(&self) -> &str {
        "hashing-model"
    }
}

/// Build every tree from nothing, draining the scopes in `order`.
async fn build_in_order(
    pool: &PgPool,
    bus: &EventBus,
    order: &[SummaryScope],
    salt: u64,
) -> Snapshot {
    drop_every_node(pool).await;
    let model = Arc::new(HashingProvider { salt });
    let c = compactor(pool, bus, Arc::new(ProviderDeps(model)));
    for scope in order {
        c.mark_dirty(*scope, false);
    }
    while !c.drain_queue().await.is_empty() {}
    snapshot(pool).await
}

/// What newest-first seeding rests on: a tree is the same whether its scope
/// was queued first or last, however long each call took.
#[tokio::test]
async fn a_tree_is_the_same_whichever_order_its_scope_was_queued() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let (chat, coding) = (
        SummaryScope::Thread(fixture.chat),
        SummaryScope::Thread(fixture.coding),
    );

    let oldest_first =
        build_in_order(&pool, &bus, &[chat, coding, SummaryScope::Workspace], 1).await;
    let newest_first =
        build_in_order(&pool, &bus, &[SummaryScope::Workspace, coding, chat], 7).await;
    assert!(
        oldest_first
            .iter()
            .filter(|n| n.4.starts_with("line "))
            .count()
            > 5,
        "the fixture makes model calls in every tree"
    );
    assert_eq!(oldest_first, newest_first);
    assert_every_scope_complete(&pool, &fixture).await;

    teardown_test_db(&db_name).await;
}

/// Push a thread's last activity out of the ready window.
async fn make_old(pool: &PgPool, thread: Uuid) {
    sqlx::query(
        "UPDATE thread_summaries SET last_activity = now() - interval '30 days' \
         WHERE thread_id = $1",
    )
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
}

/// Ready waits on the workspace tree and the recent threads, never on an old
/// thread. Seeding takes the workspace first, then threads newest first. The
/// old thread fills in after, the read says so, and until then recall opens
/// its entries raw.
#[tokio::test]
async fn the_workspace_is_ready_before_an_old_thread_is_built() {
    use crate::engine::summary_tree::module::tree_backfill;
    use crate::engine::summary_tree::recall;
    use crate::engine::summary_tree::view::NodeId;
    use crate::engine::summary_tree::{Runtime, TreeBackfill};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    make_old(&pool, fixture.coding).await;
    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        "tree",
        None,
    )
    .await
    .unwrap();
    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    let runtime = Runtime::default();
    runtime.set(c.clone());

    c.seed().await.unwrap();
    let (chat, home, coding) = (
        SummaryScope::Thread(fixture.chat),
        SummaryScope::Thread(fixture.home),
        SummaryScope::Thread(fixture.coding),
    );
    assert_eq!(
        c.queued(),
        vec![SummaryScope::Workspace, chat, home, coding]
    );

    assert_eq!(
        c.drain_next().await.map(|d| d.0),
        Some(SummaryScope::Workspace)
    );
    assert_eq!(c.drain_next().await.map(|d| d.0), Some(chat));
    assert!(
        !store::is_ready(&pool).await.unwrap(),
        "the recent home thread is not built yet"
    );
    assert_eq!(c.drain_next().await.map(|d| d.0), Some(home));
    assert!(store::is_ready(&pool).await.unwrap());
    // No scope is under way between drains, so no node counts.
    let filling = BackfillProgress {
        done: 3,
        total: 4,
        done_milli: 3_000,
        ready: true,
        ..BackfillProgress::default()
    };
    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Ready {
            filling: Some(filling)
        }
    );

    let whole = NodeId {
        scope: coding,
        addr: NodeAddr { start: 0, span: 4 },
    };
    let lines = recall::zoom(&pool, whole, 2, &|_, _| None).await.unwrap();
    assert!(
        lines.iter().any(|l| l.text == "user: fix the parser"),
        "{lines:?}"
    );

    assert_eq!(c.drain_next().await.map(|d| d.0), Some(coding));
    assert_eq!(
        tree_backfill(&pool, &runtime).await.unwrap(),
        TreeBackfill::Ready { filling: None }
    );
    assert_every_scope_complete(&pool, &fixture).await;
    teardown_test_db(&db_name).await;
}

/// A restart after early ready keeps the flag, owes the old thread again, and
/// counts it as filling until it is built.
#[tokio::test]
async fn a_restart_after_early_ready_keeps_the_flag_and_keeps_filling() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    make_old(&pool, fixture.coding).await;
    let first = compactor(&pool, &bus, deps(EchoProvider::new()));
    first.seed().await.unwrap();
    for _ in 0..3 {
        first.drain_next().await;
    }
    assert!(store::is_ready(&pool).await.unwrap());
    drop(first);

    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.seed().await.unwrap();
    let progress = c.backfill_progress().expect("the old thread is still owed");
    assert_eq!(
        (progress.done, progress.total, progress.ready),
        (3, 4, true)
    );
    assert!(store::is_ready(&pool).await.unwrap());

    run_to_rest(&c).await;
    assert_eq!(c.backfill_progress(), None);
    assert!(store::is_ready(&pool).await.unwrap());
    assert_every_scope_complete(&pool, &fixture).await;
    teardown_test_db(&db_name).await;
}

/// A model that takes a while and counts the calls it holds at once.
#[derive(Default)]
struct SlowProvider {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmProvider for SlowProvider {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(line_response("a line".to_string()))
    }

    fn default_model(&self) -> &str {
        "slow-model"
    }
}

/// On the running workers, a backfill runs many calls at once.
#[tokio::test]
async fn a_backfill_runs_many_calls_at_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for i in 0..48 {
        seed(
            &bus,
            Uuid::new_v4(),
            EventChannel::Chat,
            vec![
                json!({"type": "MessageReceived", "mode": "human",
                       "text": format!("request {i}: {}", "a long paste. ".repeat(50))}),
                json!({"type": "ResponseGenerated", "text": "ok"}),
            ],
        )
        .await;
    }
    age_events(&pool).await;
    let model = Arc::new(SlowProvider::default());
    let c = compactor(&pool, &bus, Arc::new(ProviderDeps(model.clone())));

    c.start();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !store::is_ready(&pool).await.unwrap() {
        assert!(std::time::Instant::now() < deadline, "the backfill stalled");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    c.pause();
    let peak = model.peak.load(Ordering::SeqCst);
    assert!(peak >= 32, "only {peak} calls ran at once");
    teardown_test_db(&db_name).await;
}

/// A model that is out of capacity on its first call, then answers.
#[derive(Default)]
struct RateLimitedOnce {
    refused: AtomicBool,
}

#[async_trait::async_trait]
impl LlmProvider for RateLimitedOnce {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        if !self.refused.swap(true, Ordering::SeqCst) {
            return Err("API error 429: rate limit exceeded".into());
        }
        Ok(line_response("a line".to_string()))
    }

    fn default_model(&self) -> &str {
        "rate-limited-model"
    }
}

/// A rate limit halves the lane its call ran in, and the scope comes back
/// later rather than failing for good.
#[tokio::test]
async fn a_rate_limited_call_halves_its_lane() {
    use crate::engine::summary_tree::limit::START_LIMIT;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let c = compactor(
        &pool,
        &bus,
        Arc::new(ProviderDeps(Arc::new(RateLimitedOnce::default()))),
    );
    c.seed().await.unwrap();
    let outcomes = c.drain_queue().await;
    assert!(outcomes.iter().any(|(_, o)| *o == Drained::Failed));
    let limit = c.lane(TEST_LANE).limit();
    assert!(
        (START_LIMIT / 2..START_LIMIT).contains(&limit),
        "the lane holds {limit}"
    );
    teardown_test_db(&db_name).await;
}

/// After ready, a live drain cannot read its own event until the horizon
/// passes, so it comes back. It comes back urgent, at the front, never behind
/// the older threads still filling in.
#[tokio::test]
async fn a_live_scope_comes_back_urgent_after_the_horizon() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    run_to_rest(&c).await;
    assert!(store::is_ready(&pool).await.unwrap());

    seed(
        &bus,
        fixture.chat,
        EventChannel::Chat,
        vec![json!({"type": "MessageReceived", "text": "one more", "mode": "human"})],
    )
    .await;
    c.mark_dirty(SummaryScope::Thread(fixture.coding), false);
    c.mark_dirty(SummaryScope::Thread(fixture.chat), true);
    assert_eq!(
        c.drain_next().await.map(|d| d.0),
        Some(SummaryScope::Thread(fixture.chat))
    );
    tokio::time::sleep(Duration::from_secs(HORIZON_SECS as u64) + Duration::from_millis(500)).await;
    assert_eq!(c.front(), Some((SummaryScope::Thread(fixture.chat), true)));
    teardown_test_db(&db_name).await;
}

/// The workspace drain the backfill owes holds its whole history, so a live
/// event marking it does not make it urgent. Once it is built, it is.
#[tokio::test]
async fn the_backfills_workspace_drain_is_never_urgent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.seed().await.unwrap();
    c.mark_dirty(SummaryScope::Workspace, true);
    assert_eq!(c.front(), Some((SummaryScope::Workspace, false)));

    run_to_rest(&c).await;
    c.mark_dirty(SummaryScope::Workspace, true);
    assert_eq!(c.front(), Some((SummaryScope::Workspace, true)));
    teardown_test_db(&db_name).await;
}

/// A live-only worker takes an urgent thread even when the owed workspace
/// drain, which never runs urgent, holds the front.
#[tokio::test]
async fn a_live_only_worker_reaches_past_the_backfills_workspace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let c = compactor(&pool, &bus, deps(EchoProvider::new()));
    c.seed().await.unwrap();
    c.mark_dirty(SummaryScope::Thread(fixture.coding), true);
    c.mark_dirty(SummaryScope::Workspace, true);
    assert_eq!(c.front(), Some((SummaryScope::Workspace, false)));
    assert_eq!(
        c.pop_live(),
        Some((SummaryScope::Thread(fixture.coding), true))
    );
    teardown_test_db(&db_name).await;
}
