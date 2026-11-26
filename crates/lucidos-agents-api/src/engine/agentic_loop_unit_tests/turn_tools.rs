use crate::engine::agentic_loop::{mcp_dropped_correction, mcp_surface_correction, TurnTools};
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
        dropped: Vec::new(),
        servers: names
            .iter()
            .filter_map(|n| crate::mcp::McpManager::parse_mcp_tool_name(n))
            .map(|(server_id, _)| server_id)
            .collect(),
    }
}

fn engine_only() -> TurnTools {
    TurnTools::new(
        vec![tool("read_file"), tool("run_bash"), tool("mcp")],
        surface(&[], 7),
        crate::mcp::mcp_tool_char_ceiling(200_000),
    )
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

#[test]
fn the_engine_authored_head_survives_byte_for_byte_and_in_order() {
    let mut tools = engine_only();
    let before: Vec<String> = tools.names().to_vec();

    tools.refresh_mcp(surface(&["mcp__slack__search"], 8));

    assert_eq!(&tools.names()[..before.len()], &before[..]);
    assert_eq!(tools.defs()[0].description, "does read_file");

    let without = engine_only();
    let head = without.defs().len();
    assert_eq!(
        serde_json::to_string(&tools.defs()[..head]).unwrap(),
        serde_json::to_string(without.defs()).unwrap(),
        "an enabled server must not change one byte of the engine head"
    );
}

#[test]
fn a_moved_generation_with_the_same_slice_rewrites_nothing() {
    let mut tools = engine_only();
    assert!(tools.refresh_mcp(surface(&["mcp__slack__search"], 8)));
    let before_defs = tools.defs().to_vec();
    let before_servers = tools.mcp_server_ids().clone();
    let before_notice = tools.mcp_dropped_notice().map(str::to_owned);

    let changed = tools.refresh_mcp(surface(&["mcp__slack__search"], 9));

    assert!(!changed, "the same slice is no change");
    assert_eq!(tools.defs(), &before_defs[..]);
    assert_eq!(tools.mcp_generation(), 9, "the stamp still moves");
    assert_eq!(
        mcp_surface_correction(&before_servers, tools.mcp_server_ids()),
        None
    );
    assert_eq!(
        mcp_dropped_correction(before_notice.as_deref(), tools.mcp_dropped_notice()),
        None
    );
}

#[test]
fn a_stopped_server_leaves_the_array() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));

    tools.refresh_mcp(surface(&[], 9));

    assert_eq!(tools.names(), ["read_file", "run_bash", "mcp"]);
    assert_eq!(tools.mcp_generation(), 9);
}

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

#[test]
fn a_server_enabled_mid_turn_is_named_as_callable_in_the_correction() {
    let mut tools = engine_only();
    let before = tools.mcp_server_ids().clone();

    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));

    let correction = mcp_surface_correction(&before, tools.mcp_server_ids())
        .expect("a server came up, so the round owes a correction");
    assert!(correction.contains("slack"), "{correction}");
    assert!(correction.contains("callable now"), "{correction}");
    assert!(
        !correction.contains("no longer offered"),
        "nothing went away: {correction}"
    );
}

#[test]
fn a_server_stopped_mid_turn_is_named_as_gone() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__post", "mcp__jira__issue"], 8));
    let before = tools.mcp_server_ids().clone();

    tools.refresh_mcp(surface(&["mcp__jira__issue"], 9));

    let correction = mcp_surface_correction(&before, tools.mcp_server_ids())
        .expect("a server went away, so the round owes a correction");
    assert!(
        correction.contains("no longer offered in this thread: slack"),
        "{correction}"
    );
    assert!(
        !correction.contains("callable now"),
        "nothing came up: {correction}"
    );
}

#[test]
fn a_refresh_that_moved_no_server_says_nothing() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__slack__search", "mcp__slack__post"], 8));
    let before = tools.mcp_server_ids().clone();

    tools.refresh_mcp(surface(&["mcp__slack__search"], 9));

    assert_eq!(
        mcp_surface_correction(&before, tools.mcp_server_ids()),
        None
    );
}

#[test]
fn one_line_carries_both_halves() {
    let mut tools = engine_only();
    tools.refresh_mcp(surface(&["mcp__jira__issue"], 8));
    let before = tools.mcp_server_ids().clone();

    tools.refresh_mcp(surface(&["mcp__slack__post"], 9));

    let correction = mcp_surface_correction(&before, tools.mcp_server_ids()).unwrap();
    assert!(correction.contains("callable now: slack"), "{correction}");
    assert!(
        correction.contains("no longer offered in this thread: jira"),
        "{correction}"
    );
    assert_eq!(correction.lines().count(), 1, "{correction}");
}

#[test]
fn the_array_keeps_its_ceiling_and_says_what_the_surface_left_out() {
    let mut tools = engine_only();
    assert_eq!(
        tools.mcp_char_ceiling(),
        crate::mcp::mcp_tool_char_ceiling(200_000)
    );
    assert_eq!(tools.mcp_dropped_notice(), None);

    let mut cut = surface(&["mcp__slack__search"], 8);
    cut.dropped = vec![crate::mcp::McpDroppedTools {
        server_id: "slack".to_string(),
        tools: vec!["mcp__slack__post".to_string()],
    }];
    tools.refresh_mcp(cut);

    let notice = tools.mcp_dropped_notice().expect("a tool was left out");
    assert!(notice.contains("mcp__slack__post"), "{notice}");
    assert_eq!(notice.lines().count(), 1, "{notice}");

    tools.refresh_mcp(surface(&["mcp__slack__search"], 9));
    assert_eq!(tools.mcp_dropped_notice(), None);
}

#[test]
fn a_moved_dropped_set_owes_a_line_and_an_unchanged_one_does_not() {
    assert_eq!(mcp_dropped_correction(None, None), None);
    assert_eq!(mcp_dropped_correction(Some("cut a"), Some("cut a")), None);
    assert_eq!(
        mcp_dropped_correction(None, Some("cut a")).as_deref(),
        Some("cut a")
    );
    assert_eq!(
        mcp_dropped_correction(Some("cut a"), Some("cut b")).as_deref(),
        Some("cut b")
    );
    let cleared = mcp_dropped_correction(Some("cut a"), None).expect("a cleared set is news");
    assert!(cleared.contains("Every MCP tool is sent now"), "{cleared}");
}
