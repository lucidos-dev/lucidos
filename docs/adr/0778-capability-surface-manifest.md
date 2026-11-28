# 0778: Each capability domain declares per operation which of the LLM tool, CLI and SDK carry it

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0018](0018-capability-parity-manifest.md), [0751](0751-manifest-tools-run-their-clis-route.md)

## Context

Each domain in the capability manifest declares which surfaces carry each
operation. The surfaces are the LLM tool, the `lucidos` CLI and the SDK. Parity
is declared per operation, not applied blanket, as ADR 0018 records. These are
the choices made for each domain in `crates/lucidos-capabilities/src/lib.rs`.

This record restates the design comments of
`crates/lucidos-capabilities/src/lib.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- `apps`: `llm` is false. The CLI carries list, get, update and delete. The SDK
  carries list and get. `create_app` stays a standalone LLM-only tool with no
  HTTP route. The domain has no create or source ops, because editing app
  source is the app-coding-agent's worktree job.
- `events`: one grouped LLM tool for emit, query and count. Each action
  delegates to `execute_emit_event`, `execute_query_events` or
  `execute_count_events` through the flat alias. `cli` and `sdk` are false.
- `changes`: one grouped LLM tool for list and apply. It delegates to
  `execute_list_changes` and `execute_apply_change` through the flat alias.
  `cli` and `sdk` are false.
- `threads`: one grouped LLM tool for the two reads `list_threads` and
  `count_threads`, delegating through the flat alias. `cli` and `sdk` are
  false. `run_thread` and `run_coding_agent` stay standalone and are not folded
  in.
- `models`: the grouped `manage_models` tool keeps its name and its schema is
  built from the manifest. A generated `lucidos models` CLI covers the
  `/models` CRUD. The tool is route-backed, so each action runs the CLI's
  route.
- `env_vars`: a grouped `env_vars` LLM tool and a generated CLI, each with
  list, set and delete. The CLI routes through the gateway-safe http client.
  `set_environment_variable` stays wired as an alias for the `set` action, per
  ADR 0788. The subcommand is `env-vars`, and the full CRUD lives at
  `/env-vars`.
- `memory`: surfaces are mixed, declared per op. The LLM gets correction, run
  in-process, plus two reads, `search` and `source`. The CLI gets every read.
  `stats` and `entries` stay CLI-only. `correct` has no HTTP route and is `cli`
  and `sdk` false. `correct_by_id` also has a route, for the CLI.
- `memory` under Tree: a Tree turn reads the summary trees and never this
  memory. The tool goes and `recall` takes its slot. The CLI stays, for
  Classic's own data.
- `pinned_apps` and `browser`: CLI routes for two tables an agent once wrote by
  SQL. Each runs the engine's own mutator, so a pin announces itself.

## Rationale

- `apps` closes a real gap: a subprocess or chat agent can run `lucidos apps
  list|get|update|delete` instead of reverse-engineering curl.
- `create_app` is a file write with a large `html_content` arg and no HTTP
  route, so it stays standalone under the hot-single-purpose-tool guardrail.
- `events` has a richer hand-written CLI with pagination cursors that the
  generator cannot reproduce. `changes` and `threads` also have hand-written
  CLIs.
- The spawn tools run through `handle_special_tool`, not `execute_tool`, and
  the hot-single-purpose guardrail keeps them standalone.
- `env_vars` closes a real subprocess-agent gap, and `llm = false` follows the
  `apps` precedent of CLI parity without touching the LLM surface.
- `memory` is injected into context, and injection is the primary path. It
  cannot recover from a bad guess, so the two LLM reads are the backstop for
  the misses that `SUB_QUERY_PROMPT` leaves.
- `correct` needs a model to judge matches, so it has no HTTP route.
- `stats` and `entries` are operator reads that never answer a user. They are
  the browsing the summaries warn against.
- Running the engine's own mutator for `pinned_apps` and `browser` is what
  makes a pin announce itself.

## Consequences

- LLM actions and CLI ops can diverge within one domain. In `models`, enable
  and disable are LLM-only PUTs with a fixed `on_chat_ladder`.
- `models` has `id` as a Body arg for `add` and a Query arg for update and delete. The manifest therefore holds two Args with the same name and type.
- `routes` in `models` is a Json arg that the LLM tool never derives. `add` and
  `update` therefore carry a raw `llm_schema`.
- The CLI subcommand `env-vars` is kebab-cased from the snake domain name so
  the generated `dispatch_env_vars` ident is valid.
- Hand-written CLIs for `events`, `changes` and `threads` stay hand-written,
  and their domains are LLM-only in the manifest.
- Under Tree, agents lose the memory tool and use `recall`, while Classic's
  data stays reachable by CLI.

## Alternatives considered

- Blanket parity across all three surfaces for every operation is rejected in
  favour of declared parity per op.
- Folding the spawn tools `run_thread` and `run_coding_agent` into the grouped
  `threads` tool is rejected. They stay standalone.
- Generating the `lucidos events` CLI is rejected, because the generator cannot
  reproduce its pagination cursors.
- Giving the LLM `stats` and `entries` for memory is rejected. They stay
  CLI-only on purpose.
- Replacing the standalone `set_environment_variable` LLM tool with a generated
  one is rejected. The LLM surface is left untouched.
