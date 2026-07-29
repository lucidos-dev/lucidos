---
paths:
  - "system-knowhow/**"
  - "docs/taxonomy.md"
  - "docs/glossary.md"
  - "crates/lucidos-engine/src/engine/thread_events.rs"
  - "crates/lucidos-engine/src/engine/event_bus.rs"
  - "crates/lucidos-engine/src/scheduler/mod.rs"
  - "crates/lucidos-engine/src/llm/tools.rs"
  - "crates/lucidos-engine/src/engine/tools/**"
  - "crates/lucidos-engine/src/engine/agent_session/prompts.rs"
  - "crates/lucidos-engine/src/api/history.rs"
  - "crates/lucidos-engine/src/api/app_ui.rs"
  - "crates/lucidos-engine/src/api/sdk.rs"
  - "crates/lucidos-engine/src/api/sdk_iframe.css"
  - "crates/lucidos-app/src/styles/global/shared-components.css"
  - "crates/lucidos-engine/src/api/proxy_pipeline_config.rs"
  - "crates/lucidos-engine/src/api/proxy.rs"
  - "crates/lucidos-engine/src/api/proxy_migration.rs"
  - "crates/lucidos-engine/src/api/proxy_script_runner.rs"
  - "crates/lucidos-engine/src/api/plugins.rs"
  - "crates/lucidos-engine/src/core/plugins.rs"
  - "crates/lucidos-engine/src/engine/tools/plugins.rs"
  - "packages/lucidos-sdk/**"
  - "crates/lucidos-cli/**"
  - "crates/lucidos-engine/src/scheduler/push.rs"
  - "crates/lucidos-engine/src/core/device_presence.rs"
  - "crates/lucidos-engine/src/api/presence.rs"
  - "crates/lucidos-engine/src/api/presence_pong.rs"
  - "crates/lucidos-app/public/sw.js"
  - "crates/lucidos-app/src/utils/pageActive.ts"
  - "crates/lucidos-app/src/store/actions/device-presence.ts"
  - "crates/lucidos-app/src/store/actions/presence-pong.ts"
  - "crates/lucidos-app/src/store/actions/in-app-notification-toast.ts"
  - "crates/lucidos-app/src/store/actions/push.ts"
  - "crates/lucidos-engine/src/runtime/python.rs"
  - "crates/lucidos-engine/src/engine/agentic_loop.rs"
  - "crates/lucidos-engine/src/engine/thread_queue/**"
  - "crates/lucidos-engine/src/api/thread_queue.rs"
---

# System-Knowhow Maintenance

> **`system-knowhow/` ships publicly.** These files are NOT in `EXCLUDE_PATHS` and are not stubbed at release — they go verbatim to the public mirror. So every example in them must use generic placeholders, never real personal/family/company-internal data or machine paths. The definition + approved placeholders live in `.claude/rules/no-private-data.md`; `/harden` enforces it.

The `system-knowhow/` files are the workspace-facing contract: each `.md` file is loaded into the engine LLM's context as the canonical reference for the surface it documents. When the underlying surface drifts, the docs become a lie that the engine actively trusts — every workspace that retrieves the stale knowhow gets bad guidance.

This rule has two halves:

1. **Drift prevention (this file).** Every code change that touches a documented surface MUST update the matching `system-knowhow/*.md` in the same change. Reviewers and `/harden` flag any drift as a hardening failure.
2. **Recipe maintenance (below).** When you change one system-knowhow file, the *other* recipes that reference it (workspace-audit, workspace-learning) may need to follow.

## Drift prevention — required updates by surface

When you touch any of the surfaces in the left column, you MUST update the file in the right column **in the same commit / branch**. Failing to do so is a `/harden` failure: the diff makes a promise the docs don't keep, and any LLM that loads the stale knowhow afterwards will be wrong.

| You changed… | You MUST also update… |
|---|---|
| `crates/lucidos-engine/src/engine/thread_events.rs` (`ThreadEvent` enum — variant added/removed/renamed, payload field changed, persistence flipped, alias added/removed) | `system-knowhow/thread-events.md` (master enumeration + payload shapes), AND if the change touches a `CodingAgent*` / `UserQuestion*` / `CodingAgentPermission*` variant, also `system-knowhow/coding-agent-events.md` |
| `crates/lucidos-engine/src/engine/event_bus.rs` (`SystemEvent` enum — variant added/removed/renamed, aggregate name changed, persistence/projection routing changed) | `.claude/rules/db.md` § Key event types, AND any `system-knowhow/*.md` that references that event by name (grep first — workspace-learning + thread-events + coding-agent-events all index events by name) |
| `ThreadEvent::is_per_token_streaming` in `crates/lucidos-engine/src/engine/thread_events.rs`, or the scheduler trigger gate in `crates/lucidos-engine/src/scheduler/mod.rs` that consumes it (adding/removing a blocklisted variant, changing the trigger matcher routing) | `system-knowhow/thread-events.md` "Triggerable" column + "Today the scheduler uses a blocklist" section, AND `system-knowhow/coding-agent-events.md` "Triggerability: blocklist semantics" section, AND `system-knowhow/building-a-trigger.md` if the change opens a new "you can now `on_event:` X" path |
| `packages/lucidos-sdk/**` (the `window.lucidos.*` JS surface — new/changed method, signature change, namespace addition) | `system-knowhow/js-sdk.md` § matching `lucidos.<namespace>` heading (also see `.claude/rules/sdk.md` for the same rule from the SDK side) |
| `crates/lucidos-app/src/styles/global/shared-components.css` (the app-facing shared component layer — the engine `include_str!`s it into `/api/v1/sdk-iframe.css` via `crates/lucidos-engine/src/api/sdk.rs`, so any class here ships to every opted-in app) — OR the iframe-only `crates/lucidos-engine/src/api/sdk_iframe.css` (tokens served to apps, `.action-btn-secondary`) | `system-knowhow/js-sdk.md` § "Component classes" + § "Theme variables" (the app-author contract — add/rename the class or token row, and keep documented token VALUES matching the CSS). Also keep the three-file split honest (reusable → `shared-components.css`; host-chrome → `host-components.css`; iframe-only → `sdk_iframe.css`) per `.claude/rules/frontend.md`. |
| `crates/lucidos-cli/**` (the `lucidos` CLI — new subcommand, flag change, output shape change) | `system-knowhow/lucidos-cli.md` |
| `crates/lucidos-engine/src/capability_manifest/**` (the *capability parity manifest* — a domain/operation/arg added or changed, or a `llm`/`cli`/`sdk` target flag flipped) | Regenerate the generated surfaces (`cargo test -p lucidos-engine --lib -- --ignored generate_cli_commands_file generate_sdk_capabilities_file`) so the staleness tests pass; wire any NEW `cli`-domain enum into `crates/lucidos-cli/src/main.rs` (`Command` variant + `run()` arm); add/adjust the grouped LLM handler under `engine/tools/` so its recognised actions match the manifest; register any NEW `sdk`-domain facade in `packages/lucidos-sdk/src/generated/capabilities.test.ts`; AND update `system-knowhow/lucidos-cli.md` / `system-knowhow/js-sdk.md` for the affected surface. See `docs/adr/0018-capability-parity-manifest.md`. |
| `crates/lucidos-engine/src/{api,core}/plugins.rs` + `crates/lucidos-engine/src/engine/tools/plugins.rs` (plugin manifest schema, install / uninstall / list flow, plugin LLM tools) | `system-knowhow/building-a-plugin.md`, AND `docs/taxonomy.md` § plugins if the layout / install semantics changed |
| `crates/lucidos-engine/src/llm/tools.rs` + `crates/lucidos-engine/src/engine/tools/**` (LLM tool added/removed/renamed, args schema changed) | `crates/lucidos-engine/src/engine/agent_session/prompts.rs` (system prompts advertise tools), AND `system-knowhow/best-practices.md` / `system-knowhow/intent-registry.md` if the tool's intent maps there |
| `crates/lucidos-engine/src/core/preference_catalog.rs` (the *preference catalog* — a settable-key added/removed/renamed, its scope/allowed-values/default/side-effect changed, or an `INTERNAL_KEYS` entry changed) | `system-knowhow/preferences.md` (the agent-facing key table — a `cargo test` sync test in `preference_catalog.rs` already fails if a catalog or internal key is undocumented), AND the *preference* / *preference catalog* glossary entries if the user-facing semantics shifted |
| `crates/lucidos-engine/src/engine/tools/bash_background.rs` (`BackgroundBashRegistry` — `read_output_in_memory_wait`, `BASH_OUTPUT_MAX_WAIT_SECS`, the `Notify`-on-chunk semantics), `crates/lucidos-engine/src/engine/tools/bash.rs` (`execute_bash_output_tool` — `wait_secs` arg handling, clamping), `crates/lucidos-engine/src/runtime/python.rs` (`truncate_python_error` — frame-trim shape, line/byte budgets), or `crates/lucidos-engine/src/engine/agentic_loop.rs` (`derive_call_key` / `python_call_key` for `RUN_PYTHON*`, the `excluded` list in the generic 3-strike guard) | `system-knowhow/running-python.md` (drain pattern, `wait_secs` semantics, error-truncation behavior, anti-pattern list — the LLM acts on what this file says about `bash_output(wait_secs)`, the auto-truncation, and the repeated-call guard) |
| `crates/lucidos-engine/src/api/history.rs` + `crates/lucidos-engine/src/api/app_ui.rs` (HTTP shapes for events / app UI bridge) | `system-knowhow/js-sdk.md` (the SDK calls these), AND `system-knowhow/building-an-app.md` if the app-side contract shifts |
| `crates/lucidos-engine/src/api/proxy_pipeline_config.rs` + `proxy*.rs` siblings (the on-disk `data/config/apis.json` schema — auth pipeline, signer kinds, header/body shapes) | `system-knowhow/building-an-auth-handshake.md`, AND `system-knowhow/best-practices.md` § `config/` |
| `crates/lucidos-engine/src/engine/agent_session/prompts.rs` (engine system prompts — the taxonomy/trigger sections, the intent registry advertise-list, the knowhow listing) | `system-knowhow/intent-registry.md` if intents added/removed, AND `system-knowhow/workspace-audit.md` (audit's reference table names sections of this file by heading) |
| `crates/lucidos-engine/src/engine/thread_queue/**` (the *Thread Queue* — admission decision rules, `CapacityPolicy` fields/defaults, `ThreadQueueRequest` variants, what queues vs preempts, boot requeue semantics, notification thresholds) or `crates/lucidos-engine/src/api/thread_queue.rs` (the panel's HTTP shapes) | `system-knowhow/thread-queue.md`, AND `.claude/rules/db.md` § Key tables + § Key event types if the projection schema or the `ThreadQueue*` / `CapacityPolicyChanged` events changed, AND the *Thread Queue* / *capacity policy* glossary entries if the user-facing semantics shifted |
| `crates/lucidos-engine/src/scheduler/push.rs`, `crates/lucidos-engine/src/core/device_presence.rs`, `crates/lucidos-engine/src/api/presence.rs`, `crates/lucidos-engine/src/api/presence_pong.rs`, `crates/lucidos-app/public/sw.js`, `crates/lucidos-app/src/utils/pageActive.ts`, `crates/lucidos-app/src/store/actions/device-presence.ts`, `crates/lucidos-app/src/store/actions/presence-pong.ts`, `crates/lucidos-app/src/store/actions/in-app-notification-toast.ts`, `crates/lucidos-app/src/store/actions/push.ts` (notification fan-out, presence tracking, PresenceCheck protocol, in-app surface §4 matrix, service worker push handling, push subscription lifecycle) | `system-knowhow/notifications.md` § matching `§N` section (§2 for the fan-out matrix, §3 for the PresenceCheck protocol, §4 for the in-app surface), AND `system-knowhow/glossary.md` if the change shifts what "active device" / "in-app surface" / "OS surface" / "source event" mean |
| Any code, UI string, or prose change that renames / retires / semantically shifts a term in `system-knowhow/glossary.md` (user-facing) or `docs/glossary.md` (dev-only) — e.g. renaming the `Trigger` Rust type, the `app` URL prefix, the `artifact` CLI subcommand, a `lucidos.*` SDK namespace; or replacing the canonical word in a user-facing message | The matching glossary entry — in the same commit. New term introduced → add a new entry to the appropriate layer (user-facing → `system-knowhow/glossary.md`; dev-only → `docs/glossary.md`). Drift between code/UI and the glossary is a `/harden` failure on the same footing as a stale `system-knowhow` file. |

The rule is simple: **the doc and the code ship together, in the same commit, on the same branch**. If the doc update would be large and you want to defer it, that's a sign the change itself needs to wait. Do not land a code change with a TODO to "update knowhow later" — the engine LLM doesn't read TODOs, it reads the published `system-knowhow/*.md`.

## High-risk surfaces — new ones MUST ship with their doc

A new surface in any of these categories cannot land without a matching `system-knowhow/*.md` (either a new file or a section appended to an existing file):

- A new `ThreadEvent` variant — must land with a row in `system-knowhow/thread-events.md` (and a deep-dive section if the payload is non-trivial).
- A new `SystemEvent` variant — must land with a row in `.claude/rules/db.md` § Key event types AND, if it's user-meaningful, in `system-knowhow/thread-events.md` or `system-knowhow/workspace-learning.md` as appropriate.
- A new `lucidos.*` SDK method or namespace — must land with a `## lucidos.<namespace>` section in `system-knowhow/js-sdk.md` (signature, example, when-to-use).
- A new `lucidos` CLI subcommand or flag — must land with a section in `system-knowhow/lucidos-cli.md`.
- A new auth-pipeline shape in the `apis.json` schema (signer kind, layer, header/body mode — defined in `crates/lucidos-engine/src/api/proxy_pipeline_config.rs` and the `proxy*.rs` siblings) — must land with the worked example in `system-knowhow/building-an-auth-handshake.md`.
- A new project term that's reused in user-facing prose, code identifiers, or design docs — must land with a glossary entry. User-facing concept (anything the user or the workspace LLM would encounter) → `system-knowhow/glossary.md`. Internal-only (engine plumbing, DB columns, build/test tooling, CC mechanics) → `docs/glossary.md`. No term gets defined in both files. See `.claude/rules/glossary.md` for the canonical-term rule itself.
- A change to the scheduler `ThreadEvent` blocklist (a new variant added to `ThreadEvent::is_per_token_streaming`, or an existing entry removed) — must land with the "Triggerable" column flipped in `system-knowhow/thread-events.md` AND a follow-up note in `system-knowhow/coding-agent-events.md` if the entry is CC-related.
- A change to the plugin manifest schema (required keys, validation rules, install / uninstall flow) — must land with the matching schema section in `system-knowhow/building-a-plugin.md`.

## `/harden` enforcement

`/harden` reviews the diff against this rule. A diff that touches any of the surfaces above without a corresponding `system-knowhow/*.md` update is a hardening failure — the same severity as a failing test. Re-open the work, write the doc update, and re-run `/harden`.

The check is intentional, not just a heuristic: the engine LLM that reads these knowhow files is a downstream consumer of the same code, on every workspace install. Drift compounds across thousands of LLM calls. A 5-minute doc edit at change time saves hours of wrong guidance later.

## After-the-fact detection

`system-knowhow/workspace-audit.md` is the recipe a workspace can run to detect drift between its installed system-knowhow files and the actual engine surfaces. It's the safety net — useful for spotting drift introduced by older changes that landed without the rule above. The rule above is the prevention; the audit is the cleanup.

## Maintaining workspace-audit

`system-knowhow/workspace-audit.md` is the recipe Lucidos uses to audit a workspace for drift against current conventions. It deliberately **references** the other system-knowhow files instead of restating their rules — so when you change one of those files, the audit may need to follow.

If your change touches `system-knowhow/best-practices.md`, `system-knowhow/js-sdk.md`, `system-knowhow/lucidos-cli.md`, `system-knowhow/intent-registry.md`, `docs/taxonomy.md`, or the engine system prompt's taxonomy / trigger section, open `system-knowhow/workspace-audit.md` and check that:

- The reference table still names the right files and what they own.
- Any check that names a section heading or filename in those sources still resolves.
- New rules warrant a new check (or expansion of an existing check).
- Removed / renamed rules don't leave dangling checks.

## Maintaining workspace-learning

`system-knowhow/workspace-learning.md` is the sibling recipe that looks at runtime *events* and proposes improvements to the workspace's apps, triggers, knowhow, and scripts. Where audit checks compliance against today's rules, learning surfaces the cases where today's rules might be wrong. Both produce a report under `data/artifacts/` and emit a completion event; neither edits anything.

The learning recipe queries event types by name. If you rename, retire, or add an event type that signals friction (failures, aborts, circuit-breaker trips, trigger errors, repeated user corrections), open `system-knowhow/workspace-learning.md` and check that:

- The event names listed under "What to walk" still exist and still mean the same thing.
- New friction signals warrant a new pattern category.
- The completion event name (`WorkspaceLearningCompleted`) still matches what the recipe emits.
