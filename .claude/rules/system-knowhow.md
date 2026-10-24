---
paths:
  - "system-knowhow/**"
  - "docs/taxonomy.md"
  - "docs/glossary.md"
  # Widen a file's path when its module becomes a directory.
  - "crates/lucidos-engine/src/engine/thread_events/**"
  - "crates/lucidos-engine/src/engine/event_bus/**"
  - "crates/lucidos-engine/src/engine/agentic_loop/**"
  - "crates/lucidos-engine/src/scheduler/mod.rs"
  - "crates/lucidos-engine/src/scheduler/notifications.rs"
  - "crates/lucidos-engine/src/llm/tools/**"
  - "crates/lucidos-engine/src/engine/tools/**"
  - "crates/lucidos-engine/src/engine/agent_session/prompts.rs"
  - "crates/lucidos-engine/src/api/history.rs"
  - "crates/lucidos-engine/src/api/app_ui.rs"
  - "crates/lucidos-engine/src/api/sdk.rs"
  - "crates/lucidos-engine/src/api/sdk_iframe.css"
  - "crates/lucidos-engine/src/api/sdk_iframe_control_theme_parts.css"
  - "crates/lucidos-engine/src/api/sdk_iframe_shapes.css"
  - "crates/lucidos-app/src/styles/global/shared-components.css"
  - "crates/lucidos-app/src/styles/global/surface.css"
  - "crates/lucidos-app/src/styles/global/text-input.css"
  - "crates/lucidos-engine/src/api/proxy_pipeline_config.rs"
  - "crates/lucidos-engine/src/api/proxy.rs"
  - "crates/lucidos-engine/src/api/proxy_migration.rs"
  - "crates/lucidos-engine/src/api/proxy_script_runner.rs"
  - "crates/lucidos-engine/src/api/plugins.rs"
  - "crates/lucidos-engine/src/core/plugins.rs"
  - "crates/lucidos-engine/src/engine/tools/plugins/**"
  - "packages/lucidos-sdk/**"
  - "crates/lucidos-app/src/components/apps/appFrameSandbox.ts"
  - "crates/lucidos-app/src/components/apps/appFrameReveal.ts"
  - "crates/lucidos-engine/src/core/apps.rs"
  - "crates/lucidos-app/src/store/actions/app-bridge.ts"
  - "crates/lucidos-cli/**"
  - "crates/lucidos-capabilities/**"
  - "crates/lucidos-engine/src/capability_manifest/**"
  - "crates/lucidos-engine/src/scheduler/push.rs"
  - "crates/lucidos-engine/src/core/device_presence.rs"
  - "crates/lucidos-engine/src/api/presence.rs"
  - "crates/lucidos-engine/src/api/presence_pong.rs"
  - "crates/lucidos-app/public/sw.js"
  - "crates/lucidos-app/src/utils/pageActive.ts"
  - "crates/lucidos-app/src/store/actions/device-presence.ts"
  - "crates/lucidos-app/src/store/actions/presence-pong.ts"
  - "crates/lucidos-app/src/store/actions/in-app-notification-toast.ts"
  - "crates/lucidos-app/src/store/actions/notification-discuss.ts"
  - "crates/lucidos-app/src/store/actions/push.ts"
  - "crates/lucidos-engine/src/net_config.rs"
  - "crates/lucidos-gateway/src/net_config.rs"
  - "crates/lucidos-app/src/mobile.rs"
  - "crates/lucidos-tailscale/**"
  - "crates/lucidos-app/src/components/settings/MobileAccessPage.tsx"
  # The other two § Settings -> Access surfaces.
  - "crates/lucidos-app/src/components/settings/AddDeviceSection.tsx"
  - "crates/lucidos-app/src/components/picker/PairingGate.tsx"
  # The device LIST, documented in remote-access.md.
  - "crates/lucidos-app/src/components/settings/deviceList.ts"
  - "crates/lucidos-app/src/components/settings/pairedDevices.ts"
  - "crates/lucidos-engine/src/runtime/python.rs"
  - "crates/lucidos-engine/src/engine/thread_queue/**"
  - "crates/lucidos-engine/src/api/thread_queue.rs"
  # Load this rule when editing its gates.
  - "scripts/check-knowhow-refs.sh"
  - "scripts/check-audit-scans.sh"
  - "scripts/check-retired-terms.sh"
  - "scripts/lib/check_retired_terms_test.sh"
---

# System-Knowhow Maintenance

> **`system-knowhow/` ships publicly.** Its files go verbatim to the public mirror, so every example uses generic placeholders. Definition and approved placeholders: `.claude/rules/no-private-data.md`.

Each `system-knowhow/*.md` is the engine LLM's canonical reference for its surface. A stale file misleads every workspace. Two halves:

1. **Drift prevention (this file).** A code change to a documented surface MUST update the matching `system-knowhow/*.md` in the same change.
2. **Recipe maintenance (below).** Changing one file may require the recipes that cite it (workspace-audit, workspace-learning) to follow.

**Frontmatter is billed on every request.** The engine splices each `name` and `description` into the chat agent's routing list. `system_knowhow_descriptions_stay_routing_sized` caps a description at 400 chars, and `always_loaded_context_stays_under_budget` caps the total. Write coverage plus routing phrases, and leave detail to the body.

## Drift prevention — required updates by surface

Touch a surface on the left and update the right **in the same commit**. Skipping it is a `/harden` failure.

| You changed… | You MUST also update… |
|---|---|
| `engine/thread_events/` (`ThreadEvent` variant added, removed or renamed, payload or persistence changed) | `system-knowhow/thread-events.md`, and `coding-agent-events.md` for a `CodingAgent*` / `UserQuestion*` variant. A **rename** adds the old name to `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, beside its `serde(alias)`. The `events` tool serves it as `retired`, and the retired-name check in `workspace-audit/triggers.md` reads only it. A rename with no `serde(alias)` belongs in neither. |
| `engine/event_bus/` (`SystemEvent` variant, aggregate name, persistence or projection routing) | the variant's doc comment, `docs/system-events.md`, and every `system-knowhow/*.md` naming that event (grep first). |
| `ThreadEvent::is_per_token_streaming`, or the trigger gate in `scheduler/mod.rs` that consumes it | `thread-events.md` "Triggerable" column and blocklist section, `coding-agent-events.md` § "Triggerability: blocklist semantics", and `triggers.md` if a new `on_event:` path opens. |
| `packages/lucidos-sdk/**` (`window.lucidos.*`: method, signature or namespace) | `js-sdk.md` § matching `lucidos.<namespace>` (see `.claude/rules/sdk.md`). A `lucidos.ui` helper replacing an app-drawn control also reaches `workspace-audit/apps.md` (`hand-rolled-ui` scan, hand-rolled-control check, § Remediation). |
| The app-facing shared CSS layer (`shared-components.css`, `surface.css`, `text-input.css`, each `include_str!`'d via `api/sdk.rs`) or an iframe-only file (`sdk_iframe.css`, `sdk_iframe_control_theme_parts.css`, `sdk_iframe_shapes.css`) | `js-sdk.md` § "Component classes" and § "Theme variables", with token VALUES matching the CSS. Keep the file split honest per `frontend-css.md`. A new class replacing an app-drawn control also reaches the hand-rolled-control check in `workspace-audit/apps.md`. |
| `appFrameSandbox.ts`, `api/app_reach.rs` (*route reach*) or `app-bridge.ts` (the *app bridge*) | `js-sdk.md` § Setup, and `workspace-audit/apps.md` "What an isolated app frame can no longer do" plus § Remediation. The audit OWNS those detection patterns ([ADR 0227](../../docs/adr/0227-app-frames-get-their-own-renderer-process.md)). |
| `core/apps.rs` (`AppReveal`) or `appFrameReveal.ts` (when the cover lifts, fuse lengths, load bar) | `js-sdk.md` § Showing the app once its content is ready, `workspace-audit/apps.md` "The ready signal" with its `ready-signal` scan and § Remediation, and the *ready signal* glossary entry. |
| `crates/lucidos-cli/**` (subcommand, flag or output shape) | `system-knowhow/lucidos-cli.md`. |
| `crates/lucidos-capabilities/**`, `capability_manifest/**` (the *capability parity manifest*) | [ADR 0018](../../docs/adr/0018-capability-parity-manifest.md). A new `cli` enum needs a `Command` variant and `run()` arm (`lucidos-cli`), a new `sdk` facade registers in `capabilities.test.ts`, and the grouped LLM handler under `engine/tools/` matches. Also `lucidos-cli.md` and `js-sdk.md`. |
| `api/plugins.rs`, `core/plugins.rs`, `engine/tools/plugins/` (manifest schema, install flow, plugin LLM tools) | `system-knowhow/plugins.md`, and `docs/taxonomy.md` § plugins if layout or install semantics changed. |
| `llm/tools/**`, `engine/tools/**` (LLM tool added, removed, renamed, or args changed) | `engine/agent_session/prompts.rs` (system prompts advertise tools), and `best-practices.md` or `intent-registry.md` if the tool's intent maps there. |
| `core/preference_catalog.rs` (the *preference catalog*) | `system-knowhow/preferences.md` (a `cargo test` sync test fails on an undocumented key), and the *preference* glossary entries if user-facing semantics shifted. |
| `bash_background.rs`, `bash.rs` (`wait_secs`), `runtime/python.rs` (`truncate_python_error`), or `agentic_loop/` (`derive_call_key`, the 3-strike `excluded` list) | `system-knowhow/running-python.md`: it documents the drain pattern, `wait_secs`, truncation and the repeated-call guard. |
| `scheduler/notifications.rs` (the `Tap` type: kind, sub-field, `Deserialize` strictness) | `js-sdk.md` § `lucidos.notifications`, and `workspace-audit/triggers.md` check "Old-form `tap` strings" with its § Remediation. The audit OWNS those patterns ([ADR 0214](../../docs/adr/0214-one-shot-migrations-are-a-notice-plus-an-audit-check.md)). |
| `api/history.rs`, `api/app_ui.rs` (HTTP shapes for events and the app UI bridge) | `js-sdk.md`, and `building-an-app.md` if the app-side contract shifts. |
| `api/proxy_pipeline_config.rs` and `proxy*.rs` siblings (the `data/config/apis.json` schema) | `building-an-auth-handshake.md`, and `best-practices.md` § `config/`. |
| `engine/agent_session/prompts.rs` (taxonomy and trigger sections, intent advertise-list, knowhow listing) | `intent-registry.md` if intents changed, and `workspace-audit.md` (its reference table names sections of this file by heading). |
| `engine/thread_queue/**` (*Thread Queue*: admission rules, `CapacityPolicy`, `ThreadQueueRequest`, boot requeue, thresholds) or `api/thread_queue.rs` | `thread-queue.md`, `.claude/rules/db.md` § Key tables and `docs/system-events.md` if the projection or `ThreadQueue*` events changed, and the *Thread Queue* / *capacity policy* glossary entries. |
| Notification and presence code (`scheduler/push.rs`, `core/device_presence.rs`, `api/presence*.rs`, `sw.js`, the presence, push and toast actions in `store/actions/`), or the native desktop half (`notifications.rs`, `native-push.ts`, `nativeWindow.ts`, `app_window.rs`) | `notifications.md` § matching `§N` (§2 fan-out matrix, §3 PresenceCheck, §4 in-app surface and its Tauri subsection), and `glossary.md` if "active device", "in-app surface" or "OS surface" shifted. |
| `net_config.rs` (engine and gateway), `crates/lucidos-tailscale/**`, `mobile.rs`, or `MobileAccessPage.tsx` | `remote-access.md` § Settings → Access, § Network bind, § Route B, and `README.md` § Remote access if the install-time `--bind` / `--tls-cert` contract changed. |
| `AddDeviceSection.tsx`, `PairingGate.tsx` (Add a device, code expiry, pairing screen) | `remote-access.md` § Settings → Access, § A phone installs before it pairs. |
| `deviceList.ts`, `pairedDevices.ts`, or the Devices section of `SettingsView.tsx` | `remote-access.md` § The list of devices lives in Settings → Devices, § What a device is called, and the *device* / *paired device* entries in `glossary.md`. |
| Any change renaming, retiring or shifting a term in `system-knowhow/glossary.md` or `docs/glossary.md` | The matching glossary entry, in the same commit. A new term gets a new entry in the right layer. Drift is a `/harden` failure. |

## A change that REMOVES a capability has no row above

The table pairs a surface with the doc that NAMES it. Removing a capability slips through, because the docs it falsifies INSTRUCT using it and name a different surface. [ADR 0227](../../docs/adr/0227-app-frames-get-their-own-renderer-process.md) is the worked case.

When a change narrows what a caller may do, **grep the corpus for the thing you took away**, not for the file you edited. Every doc that tells someone to use it is now wrong. Where there is no replacement to name, say so plainly and open a row in `docs/temporary-measures.md`.

**The doc and the code ship together, in the same commit.** Never land a TODO to "update knowhow later": the engine LLM reads the published `system-knowhow/*.md`, not TODOs.

## High-risk surfaces — new ones MUST ship with their doc

A new surface here cannot land without a matching `system-knowhow/*.md`, either a new file or a section appended to one:

- **`ThreadEvent` variant:** a row in `thread-events.md`, plus a deep-dive section if the payload is non-trivial.
- **`SystemEvent` variant:** a doc comment on the variant (`docs/system-events.md` for more). If user-meaningful, also in `thread-events.md` or `workspace-learning.md`.
- **`lucidos.*` SDK method or namespace:** a `## lucidos.<namespace>` section in `js-sdk.md` (signature, example, when-to-use).
- **`lucidos` CLI subcommand or flag:** a section in `lucidos-cli.md`.
- **Auth-pipeline shape in `apis.json`** (signer kind, layer, header or body mode): a worked example in `building-an-auth-handshake.md`.
- **Project term reused in prose, code or design docs:** a glossary entry. User-facing concepts go in `system-knowhow/glossary.md`, internal ones in `docs/glossary.md`, never both. See `.claude/rules/glossary.md`.
- **Scheduler `ThreadEvent` blocklist change:** flip the "Triggerable" column in `thread-events.md`, plus a note in `coding-agent-events.md` for CC-related entries.
- **Plugin manifest schema change** (keys, validation, install flow): the matching section in `plugins.md`.

## `/harden` enforcement

`/harden` reviews the diff against this rule. A surface change without its `system-knowhow/*.md` update is a hardening failure, like a failing test.

`/harden` Phase 4.5 runs the deterministic gates for every diff. `scripts/check-knowhow-refs.sh` rejects unresolved paths, knowhow ids, event names and severities. `scripts/check-retired-terms.sh` rejects unqualified retired terms from its registry.

## After-the-fact detection

`system-knowhow/workspace-audit.md` is the recipe a workspace runs to detect drift from older changes that skipped this rule. This rule prevents drift, and the audit cleans it up.

## Maintaining workspace-audit

`system-knowhow/workspace-audit.md` audits a workspace against current conventions. Each check lives in one *audit section* file under `workspace-audit/`, which the root names by id. It **references** other knowhow files and never restates their rules.

Its sources are the files in its § "Sources of truth" table, plus `triggers.md`, `docs/taxonomy.md`, and the engine system prompt's taxonomy and trigger section. Touch one and open the audit section that cites it, to check that:

- The reference table still names the right files and what they own.
- Any check naming a heading or filename in those sources still resolves.
- New rules warrant a new check, or an expanded one.
- Removed or renamed rules leave no dangling check.
- No check has grown a **copy** of a rule its source owns. Cite the section, name the severity, and stop.

`./scripts/check-knowhow-refs.sh` catches the mechanical half in `/harden`. A semantically stale check still needs a human.

`./scripts/check-audit-scans.sh` runs every `scan` line against the fixture lines in its `FIXTURES` block. Change a scan, and add a hit row for the form it must catch and a miss row for the form it must not. A new scan fails the gate until it has a hit row.

## Maintaining workspace-learning

`system-knowhow/workspace-learning.md` is the sibling recipe that reads runtime *events* and proposes improvements to apps, triggers, knowhow and scripts. Audit checks compliance, and learning surfaces where the rules might be wrong. Neither edits anything. 
The learning recipe queries event types by name. Rename, retire or add an event type that signals friction (failures, aborts, circuit-breaker trips, trigger errors, repeated user corrections), and check that:

- The event names under "What to walk" still exist and mean the same thing.
- New friction signals warrant a new pattern category.
- The completion event name (`WorkspaceLearningCompleted`) still matches what the recipe emits.
