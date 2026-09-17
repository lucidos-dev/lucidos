//! What the turn's tool array does when an MCP server comes up or goes away
//! mid-turn. The loop's half is the generation check; this is the swap itself.

use crate::engine::agentic_loop::{mcp_surface_correction, TurnTools};
use crate::llm::provider::ToolDefinition;
use crate::mcp::McpToolSurface;

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("does {name}"),
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
    }
}

fn surface(names: &[&str], generation: u64) -> McpToolSurface {
    McpToolSurface {
        tools: names.iter().map(|n| tool(n)).collect(),
        generation,
    }
}

/// The turn as it stood before the model started anything: engine-authored
/// families only.
fn engine_only() -> TurnTools {
    TurnTools::new(vec![tool("read_file"), tool("run_bash"), tool("mcp")], 7)
}

#[test]
fn a_server_started_mid_turn_lands_in_the_array() {
    let mut tools = engine_only();
    assert_eq!(tools.mcp_generation(), 7);

    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));

    assert_eq!(
        tools.names(),
        [
            "read_file",
            "run_bash",
            "mcp",
            "mcp__slack__search",
            "mcp__slack__post"
        ]
    );
    assert_eq!(
        tools.mcp_generation(),
        8,
        "the turn now holds what the surface said it was reading"
    );
}

/// The `mcp` management tool is not an MCP tool. Dropping it on a refresh
/// would take away the only tool that could stop the server again.
#[test]
fn the_engine_authored_head_survives_byte_for_byte_and_in_order() {
    let mut tools = engine_only();
    let before: Vec<String> = tools.names().to_vec();

    tools.refresh_mcp(surface(&["mcp__slack__search"], 8));

    assert_eq!(&tools.names()[..before.len()], &before[..]);
    assert_eq!(tools.defs()[0].description, "does read_file");
}

#[test]
fn a_stopped_server_leaves_the_array() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));

    // What a Stop leaves behind: no running server offers anything.
    tools.refresh_mcp(surface(&[], 9));

    assert_eq!(tools.names(), ["read_file", "run_bash", "mcp"]);
    assert_eq!(tools.mcp_generation(), 9);
}

/// One server stopping must not take the other one's tools with it, because
/// the refresh replaces the whole slice rather than removing one server's.
#[test]
fn one_server_stopping_leaves_the_other_one_offered() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__post", "mcp__jira__issue"], 8));

    tools.refresh_mcp(surface(&["mcp__jira__issue"], 9));

    assert_eq!(
        tools.names(),
        ["read_file", "run_bash", "mcp", "mcp__jira__issue"]
    );
}

/// The schemas ride every request, so the budget the trimmer works against is
/// derived from this figure. A stale one lets the array and the messages
/// together overflow the context window.
#[test]
fn the_schema_size_tracks_what_the_array_actually_holds() {
    let mut tools = engine_only();
    let engine_chars = tools.defs_chars();
    assert!(engine_chars > 0);

    tools.refresh_mcp(surface(&["mcp__slack__search"], 8));
    let with_slack = tools.defs_chars();
    assert!(
        with_slack > engine_chars,
        "a bigger array costs more: {engine_chars} → {with_slack}"
    );

    tools.refresh_mcp(surface(&[], 9));
    assert_eq!(
        tools.defs_chars(),
        engine_chars,
        "and the cost goes back down when the server stops"
    );
}

// ---------------------------------------------------------------------------
// The tail correction
// ---------------------------------------------------------------------------
//
// `[STOPPED MCP SERVERS]` is assembled once, into the first message, which the
// turn never rewrites. So the round that moves the array owes the model one
// line saying which servers moved.

/// Only MCP tools name a server, and the `mcp` management tool is not one.
#[test]
fn the_offered_servers_are_read_off_the_mcp_tools_alone() {
    let mut tools = engine_only();
    assert!(tools.mcp_server_ids().is_empty());

    tools.refresh_mcp(surface(&["mcp__slack__post", "mcp__jira__issue"], 8));

    assert_eq!(
        tools.mcp_server_ids().into_iter().collect::<Vec<_>>(),
        ["jira", "slack"]
    );
}

/// The reported failure: round 1 starts `slack`, round 2 carries its tools,
/// and the first message still tells the model to call `start_mcp_server`.
#[test]
fn a_server_started_mid_turn_is_named_as_callable_in_the_correction() {
    let mut tools = engine_only();
    let before = tools.mcp_server_ids();

    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));

    let correction = mcp_surface_correction(&before, &tools.mcp_server_ids())
        .expect("a server came up, so the round owes a correction");
    assert!(correction.contains("slack"), "{correction}");
    assert!(correction.contains("callable now"), "{correction}");
    assert!(
        !correction.contains("no longer running"),
        "nothing went away: {correction}"
    );
}

#[test]
fn a_server_stopped_mid_turn_is_named_as_gone() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__post", "mcp__jira__issue"], 8));
    let before = tools.mcp_server_ids();

    tools.refresh_mcp(surface(&["mcp__jira__issue"], 9));

    let correction = mcp_surface_correction(&before, &tools.mcp_server_ids())
        .expect("a server went away, so the round owes a correction");
    assert!(
        correction.contains("no longer running: slack"),
        "{correction}"
    );
    assert!(
        !correction.contains("callable now"),
        "nothing came up: {correction}"
    );
}

/// Switching one tool off bumps the generation and rebuilds the array, but the
/// server list is unchanged. A line saying so would bill every request for
/// news the first message never contradicted.
#[test]
fn a_refresh_that_moved_no_server_says_nothing() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));
    let before = tools.mcp_server_ids();

    tools.refresh_mcp(surface(&["mcp__slack__search"], 9));

    assert_eq!(
        mcp_surface_correction(&before, &tools.mcp_server_ids()),
        None
    );
}

/// A round can see both halves at once: the user stops one server from
/// Settings while the model starts another.
#[test]
fn one_line_carries_both_halves() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__jira__issue"], 8));
    let before = tools.mcp_server_ids();

    tools.refresh_mcp(surface(&["mcp__slack__post"], 9));

    let correction = mcp_surface_correction(&before, &tools.mcp_server_ids()).unwrap();
    assert!(correction.contains("callable now: slack"), "{correction}");
    assert!(
        correction.contains("no longer running: jira"),
        "{correction}"
    );
    assert_eq!(correction.lines().count(), 1, "{correction}");
}
