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
  - "crates/lucidos-engine/src/engine/chat/process/system_prompt.rs"
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
  - "scripts/check-knowhow-generated.sh"
  - "scripts/check-knowhow-coupling.sh"
  - "docs/knowhow-surfaces.tsv"
  - "scripts/lib/check_knowhow*_test.sh"
  - "scripts/check-audit-scans.sh"
  - "scripts/lib/check_audit_scans_test.sh"
  - "scripts/check-retired-terms.sh"
  - "scripts/lib/check_retired_terms_test.sh"
---

# System-Knowhow Maintenance

> **`system-knowhow/` ships publicly.** Its files go verbatim to the public mirror, so every example uses generic placeholders. Definition and approved placeholders: `.claude/rules/no-private-data.md`.

`system-knowhow/*.md` is canonical for the engine LLM. Two halves:

1. **Drift prevention (this file).** A code change to a documented surface MUST update the matching `system-knowhow/*.md` in the same change.
2. **Recipe maintenance (below).** Changing one file may require the recipes that cite it (workspace-audit, workspace-learning) to follow.

**Frontmatter is billed on every request.** The engine splices each `name` and `description` into the chat agent's routing list. `system_knowhow_descriptions_stay_routing_sized` caps a description at 400 chars, and `always_loaded_context_stays_under_budget` caps the total. Write coverage plus routing phrases, and leave detail to the body.

## Drift prevention — required updates by surface

[The surface registry](../../docs/knowhow-surfaces.tsv) owns the path mappings and conditional review guidance.
Touch a declared surface and update its documentation in the same change.
The coupling gate checks this mechanical floor; review checks the registry's semantic requirements.
Term changes follow [the glossary rule](glossary.md).

## A change that REMOVES a capability needs a corpus search

The registry pairs a surface with the doc that names it.
Removing a capability can also falsify instructions that name a different surface.
[ADR 0227](../../docs/adr/0227-app-frames-get-their-own-renderer-process.md) is the worked case.

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

`scripts/check-knowhow-coupling.sh` checks committed `main...HEAD` changes against the surface registry and audit declarations.
It includes deleted and renamed source paths and rejects missing or malformed inputs.
It reads both endpoint declarations, so deleting a mapping cannot erase its obligation.

A listed documentation path in the diff satisfies the surface's mechanical check.
Documentation can be an embedded system prompt; a target cannot overlap its source globs.
Each endpoint must declare sources that exist in its own tree.
Review still checks conditional requirements and whether the documentation matches the behavior.

When documentation stays correct, add `Knowhow-unchanged: <surface>: <reason>` as a commit trailer.
Use the registry's surface id, or `audit/<filename stem>` for an audit section.
Each commit changing the source needs the trailer unless the branch updates the documentation.
A trailer from an earlier commit cannot cover later source changes.
A clean two-parent merge reuses prior trailers; its resolution edits need their own.

`make lint-knowhow` checks forward references, retired terms and audit scans without Cargo. `/harden` runs it for every diff, plus fixtures and a fresh comparison against the committed agent surface. Domain examples declare `<!-- domain-event: Name: reason -->` in the file that uses them.

## After-the-fact detection

`system-knowhow/workspace-audit.md` is the recipe a workspace runs to detect drift from older changes that skipped this rule. This rule prevents drift, and the audit cleans it up.

## Maintaining workspace-audit

`system-knowhow/workspace-audit.md` audits a workspace against current conventions. Each check lives in one *audit section* file under `workspace-audit/`, which the root names by id. It **references** other knowhow files and never restates their rules.

Each section declares `sources:` in its frontmatter as a nonempty JSON array of repository paths or globs.
Use tracked paths and the registry's glob syntax; include knowhow and code that the section checks.
The coupling gate requires the section to change when a declared source changes, or an `audit/<filename stem>` trailer explaining why it remains current.
Touch a source and open the audit section that cites it, to check that:

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
