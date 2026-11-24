# 0788: Related agent verbs ship as grouped tools built from the capability manifest with flat names as aliases

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0018](0018-capability-parity-manifest.md), [0220](0220-jev-is-a-judgment-provider-not-an-llm-provider.md), [0223](0223-a-typed-judgment-the-agent-asks-for.md)

## Context

`crates/lucidos-core/src/llm/tool_names.rs` holds the tool names the model
sees. Several domains have many verbs, such as triggers, trigger groups,
preferences and environment variables. Some single-purpose tools do not fit a
grouped domain. The comments record how each case is named and placed.

This record restates the design comments of
`crates/lucidos-core/src/llm/tool_names.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- Triggers, trigger groups, preferences and environment variables each get one
  grouped tool with an action argument.
  - `triggers` covers create, list, update, delete, pause and resume.
  - `trigger_groups` covers list, create, rename, reorder and delete. Trigger
    groups are the user-visible folders that organize triggers in the panel.
  - `preferences` covers get and set.
  - The environment variable tool covers list, set and delete.
- The grouped tools are built from the capability parity manifest. See
  `crate::capability_manifest`.
- The old flat per-verb names stay as back-compat aliases. They dispatch to the
  same handler as the grouped action.
- `set_environment_variable` stays wired as an alias to the `set` action.
- `set_preference` folds in the former `set_language`, `set_timezone` and
  `enable_push_notifications` tools. The catalog of settable keys lives in
  `core/preference_catalog.rs`.
- The tool that redirects an already spawned child thread is standalone. It is
  not an action of `threads`.
- The judgment tool, which asks a judgment provider typed questions about one
  state, is named for the capability and not for Jev.

## Rationale

- Grouping from the manifest keeps one source for each domain's verbs. Keeping
  the flat names as aliases means existing callers keep working and reach the
  same handler.
- The child redirect follows the hot-single-purpose guardrail. It is the third
  member of the `run_thread` and `run_coding_agent` family.
- The `threads` domain is an introspection tool whose summary opens "Introspect
  threads". A model that just called `run_thread` would not look there for a
  write verb.
- `llm::judgment` is backend-neutral. Naming the tool for the capability means
  a second backend needs no rename.

## Consequences

- Each grouped domain has one tool entry plus its flat aliases, and both paths
  reach one handler.
- The aliases must stay wired until the comments' back-compat purpose ends.
- The child redirect is its own tool entry and not a `threads` action.
- The judgment tool's name does not change when a backend is added.

## Alternatives considered

- Making the child redirect an action of `threads`. Rejected because of the
  hot-single-purpose guardrail and the introspection summary of `threads`.
- Naming the judgment tool for Jev. Rejected because `llm::judgment` is
  backend-neutral and a second backend would force a rename.
