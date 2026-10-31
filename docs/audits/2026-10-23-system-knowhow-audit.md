# System knowhow audit

- **Date**: 2026-10-23
- **Scope**: every file under `system-knowhow/` (41 markdown files, `oauth-providers.json`, `scripts/list-workspaces.sh`), plus the agent-facing text in the engine: the coding-agent prompts (`engine/agent_session/prompts.rs`), the chat system prompt (`engine/chat/process/system_prompt.rs`) and the LLM tool descriptions (`llm/tools/`, `crates/lucidos-capabilities/`).
- **Trigger**: ADR 0560 retired successor threads. The term still appears across comments, UI copy, tests and knowhow.
- **Follow-up**: [the plan](../plans/2026-10-23-system-knowhow-currency.md) and [the accepted ADR](../adr/0565-system-knowhow-stays-current-by-gates.md).

## Method

Two passes.

1. **A deterministic sweep.** A throwaway script read every backtick name in the corpus. It checked CLI commands against the clap tree and tool names against `llm/tool_names.rs`. It checked event-shaped names against `ThreadEvent`, `SystemEvent` and `LEGACY_TYPE_NAME_ALIASES`. The existing gates also ran: `check-knowhow-refs.sh` and `check-audit-scans.sh` both pass.
2. **A claim-by-claim read.** Seventeen reviewers each took a slice of the corpus. They checked tool names and arguments, event names and payloads, CLI commands and flags, preference keys, paths, behaviours and retired concepts against the source.

**Coverage is not total.** Each reviewer lists what it skipped, and § Not reached collects the gaps. A claim a reviewer could not resolve is not called wrong here.

## Severity

Every Lucidos user's agent reads these files as fact. So severity weighs two things: how many users reach the claim, and what happens when the agent acts on it.

| Severity | Meaning |
|---|---|
| **high** | An agent that follows the claim produces a wrong result **silently**, on a path any user can reach. |
| **medium** | The claim misleads on a common path. The agent fails loudly, or recovers after a wasted call. |
| **low** | Cosmetic, rare, or a pointer that is wrong while the behaviour it describes holds. |

## Findings

### High

| File:line | Claim | What the code does | Harm |
|---|---|---|---|
| `building-an-auth-handshake.md:353` | "`capabilities`: only `"replace_body"` is recognized" | A second capability exists: `read_prior_headers` (`api/proxy_wasm_signer.rs:68`), gated by the manifest and `granted_capabilities` (`:821`). | An agent never grants it, so the composed signer below fails. |
| `building-an-auth-handshake.md:567-582` | The composing example reads `prior_layer_outputs["script_handshake"]["headers"][…]` and grants nothing | Without the grant, each prior header value reads `"[withheld: grant read_prior_headers]"` (`proxy_wasm_signer.rs:70-75`). | The signer signs a placeholder. Upstream answers 401, and the cause is invisible. |

### Medium

| File:line | Claim | What the code does |
|---|---|---|
| `building-an-auth-handshake.md:29, 567` | Prior outputs are "keyed by layer kind (`script_handshake`, `wasm_signer`)" | A static layer keys by its kind (`bearer`, `api_key`, …). A layer after index 0 gets `_<idx>` appended (`api/proxy_pipeline_builder.rs:185-202`). |
| `building-an-auth-handshake.md:101-103` | The HMAC key goes in `X-MBX-APIKEY` unless `key_header` overrides | The default is `X-API-KEY` (`api/proxy_pipeline_config.rs:59-60, 94-96`). |
| `thread-events.md:224` | `SessionEnded.reason` is `Shutdown` / `Panic` / `Closed` / `StaleResume` / `LegacyNonTerminal` | The enum serialises `snake_case` (`thread_events/session.rs:18-19`). A trigger condition on `"StaleResume"` never matches. |
| `thread-events.md` (no row) | `MessageHeld` and `HeldMessageReleased` are missing | Both are live, persisted and triggerable (`thread_events/event.rs:155-173`). |
| `thread-events.md:43, 48` | The trigger blocklist "drops two groups" | `is_subscribable` also drops a keep-alive `CodingAgentIdled` (`core/event_subscription/mod.rs:283-287`). A trigger on it silently skips those idles. |
| `thread-events.md:158` | The `origin.reason.kind` list | It lacks `successor_session`, which stamps a handoff's opening message (`thread_events/actor.rs:66-68`). No knowhow file names it. |
| `notifications.md:65, 438` | "the LLM `enable_push_notifications` tool" | No such tool. Push is `set_preference` on `push_notifications` (`llm/tools/notifications.rs:59-61`). |
| `plugins.md:710` | Re-run `install_plugin` with `overwrite=true` | `install_plugin` takes only `source` (`crates/lucidos-capabilities/src/lib.rs:2909`). It stages a confirm panel that lists the overwrites. |
| `plugin-setup.md:20` | Match the seed's cause on "`changed its setup instructions`" | With a prior version known, the seed reads "its setup instructions changed since version X" (`engine/tools/plugins/mod.rs:1766, 1783`). `plugins.md:236-238` has it right. |
| `intent-registry.md:20, 24` | Listed intents are loadable "by construction" | A standalone trigger file whose stem differs from its folder is listed but not loadable (`core/intents.rs:48-56, 104-115`). |
| `running-python.md:262` | A background traceback lands at `.lucidos/exhaust/<task_id>/stderr.txt` | The background path writes only `script.py`, under a fresh UUID (`engine/tools/python.rs:236-242`). |
| `building-knowhow.md:8` | Frontmatter rules "live in `docs/taxonomy.md`" and placement "in CLAUDE.md" | Neither file is in a user's workspace, and this file never states the rules. |
| `workspace-audit.md:298, 302-304` | "When a child's session ends, this audit thread resumes" | Since ADR 0451, a home-thread spawn defaults to `no-wake` (`llm/tools/threads.rs:48`). The remediation step never runs. The fan-out should pass `wake_mode: "wake-when-all-done"`. |
| `glossary.md:1331`, `js-sdk.md:1806, 1834` | "the `list_threads` LLM tool", "`count_threads`" | Both are back-compat aliases of the grouped `threads` tool (`crates/lucidos-capabilities/src/lib.rs:2018, 2033`). The model's schema does not offer them. |
| `coding-agent-events.md:404` | "the `spawn_thread` tool" | No such tool. The chat agent spawns with `run_thread` (`llm/tool_names.rs:108`). |
| `lucidos-cli.md:474-480` | A follow-up ack's `delivered_to` is one of four values | There is a fifth, `held` (`api/threads/follow_up.rs:96`). |
| `lucidos-cli.md:1785` | `hardened query` prints one of three words | It prints one of four: `FRESH`, `STALE`, `MISSING`, `NOT_NEEDED` (`crates/lucidos-cli/src/hardened.rs:32-35`). |
| `js-sdk.md:1909-1910` | `successor` / `successor_of` are "the successor thread it continued in" | Only legacy rows set them. A handoff stays in the thread, so the fields stay absent (ADR 0560). |
| `scripts/list-workspaces.sh:13-15` | A `.lucidos/ports` file means RUNNING | Nothing deletes the file on stop (`crates/lucidos-gateway/src/stack.rs:355-373`). Every workspace that ever ran shows as running. |

### Low

| File:line | Claim | What the code does |
|---|---|---|
| `building-an-auth-handshake.md:101, 320` | HMAC "appends `timestamp=<ms>` first" | Only when `timestamp_param` is set. It has no default (`proxy_pipeline_config.rs:65-66`). |
| `building-an-app.md:347` | The built-in proxy list | The `typesafe` proxy already exists (`llm/tools/proxy.rs:21`). Add it to the documented list; no new proxy is proposed. |
| `js-sdk.md:1055-1078` | "The whole reachable set" of app routes | It lacks `/fonts`, `/fonts/:file` and `/model-prices` (`api/app_reach.rs:157, 160, 217`). |
| `js-sdk.md:2116` | The `settings_view` values | It lacks `intelligence` (`packages/lucidos-sdk/src/generated/navigate-targets.ts`). |
| `js-sdk.md:1848` | `carried_by_successor` listed without a qualifier | Legacy successor threads only (ADR 0560). |
| `thread-events.md:158, 420, 460, 1260` | `successor_residue` and `carried_by_successor` read as current | Both fire only for a legacy successor thread (`engine/successor.rs:537-545`). |
| `glossary.md:1983, 2077` | A thread "that continued in a *successor thread*", "each with its successors" | Legacy only. A successor session is the same thread. |
| `glossary.md:208` | "The judge model defaults to Gemini 3 Flash" | It defaults to `gemini-3.1-flash-lite` (`core/preference_catalog.rs:355`). |
| `triggers.md:16-17` | "The CLI mirrors them as `lucidos triggers …`" | `pause` and `resume` are LLM-only. The CLI uses `update --paused`. |
| `triggers.md:695` | "another listens via `on_event`" | The field is `on` (`engine/tools/scheduler.rs:158`). |
| `thread-queue.md:266-267` | "the `list_thread_queue` LLM tool" | An alias. The model sees `thread_queue` with `list` and `update_policy`. |
| `plugins.md:609, 720, 737, 742` | `engine/tools/plugins.rs::detect_source`, `execute_check_plugin_updates`, `execute_update_plugin` | None resolve. The code is `plugins/source.rs`, `check_plugin_updates_impl` (`plugins/registry.rs:721`) and `stage_install_request` (`plugins/mod.rs:237`). |
| `workspace-audit/plugins.md:56` | `registry.rs::execute_check_plugin_updates` | Same: `check_plugin_updates_impl`. `check-knowhow-refs.sh` does not resolve `::symbol` suffixes. |
| `workspace-learning.md:60` | Circuit-breaker trips, "see `.claude/rules/frontend.md`" | The row names no event to query, and cites a repo-only file. |
| `notifications.md:10, 823, 831, 907-912, 933-934, 950-959` | Tests named `s2_scenario_*`, `s4_row*_…` | None exist. They appear only in an old plan. |
| `notifications.md:275` | `bootstrappingThreadId` | Renamed: `awaitedThreadId` / `releaseAwait` (`store/actions/threads.ts:243-280`). |
| `notifications.md:191, 335` | The toast's first gate is `shellIsActive()` | The first gate is `workspaceUnavailable()` (`store/actions/in-app-notification-toast.ts:179-182`). |
| `coding-agent-events.md:562-572` | The turn-gap table | It lacks `ChildThreadDetached` (`agent_session/turn_gap.rs:161`). |
| `running-python.md:249` | "three ERRORs in a row … trip it" | It warns at 3 and force-breaks at 5, and covers `run_python_background` too (`agentic_loop/helpers.rs:1810-1833`). |
| `building-knowhow.md:22` | A shared fact "may belong in `docs/` or `system-knowhow/`" | A user's agent can write neither. |
| `lucidos-cli.md:1606, 1677` | "a build → apply pipeline", "a build-then-apply script" | Wording left over from the build chain. The commands work. |
| `lucidos-cli.md:1478-1479` | `[1m]` raises only `claude-*` to 1M | Any id carrying `[1m]` gets 1M. |
| `backups.md:55` | `backup_retention` is `1`–`50` | The preference takes 1–1000. Only the UI stops at 50 (`core/preference_catalog.rs:1367-1369`). |
| `remote-access.md:845` | "workspace picker → Settings → Network access" | A **Network access** popover from the header gear (`picker/NetworkAccessPopover.tsx:108`). |
| `remote-access.md:201` | "Settings > Webhooks" | Correct page, but `>` where the doc uses `→`. |
| `workspace-fonts.md:9, 93, 104`; `themes.md:20, 320`; `preferences.md:303-305` | "Settings → Font", "Settings → Appearance → …" | The page is "Appearance & Behavior → Typography" (`components/search/searchIndex.ts:221-242`). |
| `preferences.md:304` vs `themes.md:316` | What `theme_effects: reduce` keeps | `preferences.md` omits `font-weight`. The catalog keeps it (`core/themes/theme-parts.json`). |
| `scripts/list-workspaces.sh:5` | `WORKSPACES_DIR="$HOME/workspaces"` | A packaged install registers workspaces in `config/workspaces.json`. |

### Engine prompts and tool descriptions

No stale claim found. Every CLI command, flag, knowhow id and tool name in the prompts resolves. `prompts.rs:864-868` already teaches ADR 0560. One help string disagrees with its doc: `lucidos mcp start --help` says "From a thread, it is enabled there too", but the route enables none (`api/mcp.rs:200-222`). The doc is right and the help is wrong.

### Stale code comments found on the way

- `thread_events/event.rs:552-554`: says `SessionStarted.coding_agent` is locked in by `COALESCE`. The projection lets a newer value win (`event_bus_projection_thread.rs:456`).
- `core/preference_catalog.rs:1339-1340`: says an enabled backup schedule generates a key. It skips the backup and asks for a key (`scheduler/backup.rs:232-248`).

## The successor thread term, repo-wide

`git grep -i "successor thread"` outside `docs/plans` and `docs/adr` finds 54 lines. They fall in three groups.

| Group | Where | Treatment |
|---|---|---|
| **A path that still runs, for legacy rows** | `HandedOffCard.tsx`, `SuccessorNotice.tsx`, `threads.ts`, `thread-meta.ts`, `proposal_hold.rs`, `standing_apply.rs`, `successor.rs`, `spawn_context.rs`, `worktree.rs`, `plan_marker.rs`, `parent_callback.rs`, `wake_decision.rs`, `cause.rs`, `actor.rs`, `changes_projection.rs`, `store/threads/mod.rs`, `thread_reach.rs`, the CSS comments, the tests, the knowhow rows above | Say **legacy successor thread** every time. A reader must never take it for the current handoff. |
| **User-facing strings** | `successor.rs:27` (`ALREADY_HANDED_OFF`), `proposal_hold.rs:189`, `standing_apply.rs:285`, `engineEventExplainers.ts:122`, `thread_reach.rs:118, 143`, `api/threads/successor.rs:437` | Reword to name the legacy case. |
| **History** | `CHANGELOG.md:105, 125`, the migration, the ADRs, the plan filename cited in comments | Keep as written. `CHANGELOG.md` gets a new entry instead. |

The glossary already carries *Successor thread*, *Build chain* and *Handoff consent* as retired entries. Those stay, as the sanctioned place a retired term is defined.

## Not reached

Each reviewer named what it skipped. Phase 1 of the plan reads these before it closes.

- `js-sdk.md`: `lucidos.oauth` error codes and refresh window, the proxy header passthrough list, behaviour prose under the UI helper sections, theme variable values.
- `lucidos-cli.md`: `build-slot` niceness, quoted response texts, `threads archive` / `detach` 409 reasons, `spawn-thread` image limits, proxy `auth.type` modes, webhook JSON shapes, `/threads/:id/context` fields.
- `glossary.md`: prose of lines 1-104, 660-880, 926-1075; runtime thresholds in the second half.
- `thread-events.md`: payload sections from line 1043 to 1475.
- `coding-agent-events.md` lines 215-540; `orchestrating-sub-threads.md` lines 63-160.
- `plugins.md`: media, widgets, marketplaces, private repos, uninstall; `oauth-providers.md` alias, scopes and Entra sections.
- Whether a user's agent can write the shipped `oauth-providers.json`, as `oauth-providers.md:47` tells it to.
- `notifications.md` §6, §7, the history section, the Tauri internals.
- `triggers.md` lines 400-700 and 880-940; `thread-queue.md` ordering and restart.
- `remote-access.md` lines 1-39, 60-84, 206-270, 320-400, 700-830.
- `workspace-audit/apps.md`, the body of `workspace-audit/prompt-footprint.md`, and a gap sweep for conventions with no check.
- `best-practices.md`: credential injection, the agent DB role, command guard lanes.
- Line-by-line reads of `prompts.rs` and `system_prompt.rs` (checked by grep), and per-argument schema text in `llm/tools/`.

## What the existing gates missed, and why

These classes overlap and group several claims per finding. They do not
partition the severity tables above.

| Finding class | Existing gate | Why it passed |
|---|---|---|
| A retired term written as current | none | No list of retired terms exists. |
| A tool name that is an alias or does not exist | none | `check-knowhow-refs.sh` checks paths and events, not tools. |
| An event or field missing from `thread-events.md` | `/harden` judgment | No test compares the enum with the rows. |
| An enum value written in the wrong case | none | The doc restates a value the code generates. |
| A `path.rs::symbol` that no longer resolves | arm 1 | Arm 1 strips the `::symbol` part. |
| A CLI value list, or a flag, that drifted | `/harden` judgment | Nothing reads clap. |
| Settings paths and UI labels | none | No machine source for labels exists outside `searchIndex.ts`. |
| Semantic drift: a default, a mechanism, a wake mode | `/harden` judgment | Only a reader catches it. |

The first six rows are mechanical. The plan turns each into a gate. The last two need judgment. For those, the plan ties a change to its doc, so the reader looks at the right moment.


## Phase 1 outcomes

The original tables above preserve the audit baseline. All 49 content findings
are fixed. The repo-wide terminology sweep belongs to Phase 2.

Rows below follow the original order within each severity table.

| Finding | Outcome |
|---|---|
| High 1: signer capabilities | Documented `read_prior_headers` and both grant locations. |
| High 2: composing signer | Added manifest and pipeline grants; explained placeholder signatures and upstream 401s. |
| Medium 1: prior-output keys | Documented static kinds and zero-based suffixes after the first layer. |
| Medium 2: HMAC key header | Corrected the default to `X-API-KEY`. |
| Medium 3: session-end values | Listed snake_case wire values. |
| Medium 4: held messages | Added `MessageHeld` and `HeldMessageReleased` rows. |
| Medium 5: trigger blocklist | Added the keep-alive idle exclusion. |
| Medium 6: origin reasons | Added `successor_session`. |
| Medium 7: push tool | Replaced the nonexistent tool with `preferences(action="set")`. |
| Medium 8: plugin overwrite | Documented the staged confirmation panel. |
| Medium 9: setup seed | Matched both current seed phrasings. |
| Medium 10: intent loading | Distinguished listing from folder-based lookup. |
| Medium 11: background traceback | Documented staged `script.py` and the distinct run UUID. |
| Medium 12: knowhow frontmatter | Stated workspace rules and included a YAML example. |
| Medium 13: audit child wake | Specified child relation and `wake-when-all-done`. |
| Medium 14: thread tool aliases | Used the offered grouped `threads` tool. |
| Medium 15: spawning tool | Corrected the name to `run_thread`. |
| Medium 16: follow-up acknowledgement | Added `held`. |
| Medium 17: harden query | Added `NOT_NEEDED`. |
| Medium 18: successor fields | Qualified both fields as legacy. |
| Medium 19: workspace health | Replaced ports-file guessing with gateway inventory and health. |
| Low 1: HMAC timestamp | Made injection conditional on `timestamp_param`. |
| Low 2: built-in proxies | Added the existing `typesafe` proxy. |
| Low 3: app routes | Added fonts and model prices. |
| Low 4: Settings navigation | Added `intelligence`. |
| Low 5: carried reason | Qualified `carried_by_successor` as legacy. |
| Low 6: residue reasons | Qualified legacy successor-thread paths. |
| Low 7: glossary successor prose | Qualified legacy thread continuations. |
| Low 8: judge default | Corrected it to `gemini-3.1-flash-lite`. |
| Low 9: trigger CLI | Used `update --paused` for pause and resume. |
| Low 10: trigger groups | Corrected the field to `on`. |
| Low 11: queue tool | Used the grouped `thread_queue` tool. |
| Low 12: plugin pointers | Corrected module paths and function names. |
| Low 13: plugin audit pointer | Corrected it to `check_plugin_updates_impl`. |
| Low 14: breaker evidence | Named forced-break `ResponseGenerated` evidence and preceding errors; warnings are not persisted. |
| Low 15: notification tests | Replaced obsolete names with the current test-file coverage map. |
| Low 16: notification await state | Named `awaitedThreadId` and `releaseAwait`. |
| Low 17: toast gate | Added the first `workspaceUnavailable()` guard. |
| Low 18: turn-gap events | Added `ChildThreadDetached`. |
| Low 19: repeated-call guard | Documented skipped warnings on attempts three and four, then a break on five. |
| Low 20: writable knowhow | Distinguished workspace knowhow from read-only shipped docs. |
| Low 21: build-chain prose | Removed obsolete pipeline wording. |
| Low 22: context fallback | Applied `[1m]` to every id and GPT fallback to major versions five onward. |
| Low 23: backup retention | Documented 1–1000 and the fixed UI dropdown choices. |
| Low 24: network access | Named the picker header gear popover. |
| Low 25: webhook navigation | Used the canonical arrow separator. |
| Low 26: typography navigation | Named Appearance & Behavior and Typography. |
| Low 27: reduced theme effects | Included retained font weight. |
| Low 28: workspace location | Used gateway registration instead of a fixed home directory. |

The two code comments now describe current backend projection and backup-key
behaviour. The MCP start help now distinguishes starting from thread enablement.

### Coverage completed and further corrections

Every area in § Not reached received a read. Source checks covered the live
contracts; historical timings and private artifacts remain historical evidence.

| Area | Checks and corrections |
|---|---|
| SDK OAuth, proxy, UI and themes | Verified errors, refresh window and theme values. Replaced legacy proxy examples; corrected header forwarding, iframe sharing and tooltip scrolling. |
| CLI gaps | Verified build-slot resource settings, response texts, image limits, webhook shapes and context fields. Corrected auth pipelines and detach status distinctions. |
| Glossary gaps | Read the listed prose and runtime thresholds. Corrected backend lifetime to one session; a successor session can change it. |
| Thread-event payloads | Added `auxiliary` to producers. Corrected threshold scaling to 3/5, mapping 150,000 to 90,000. |
| Coding-agent events and orchestration | Read the listed sections. Fixed the domain-event example to pass `--summary` and `--payload`. |
| Plugins and OAuth | Read media, widgets, marketplaces, private sources, uninstall, aliases, scopes and Entra. Replaced the impossible shipped-registry edit with explicit endpoint arguments. |
| Notification internals and history | Read the remaining sections. Corrected failed native-window reads and old-client push fallback; retained historical chronology. |
| Triggers and queue | Read script, group, persistence and restart sections. Clarified queue delegation to cause-dependent recovery. |
| Remote access | Verified listener rules and Mobile Access budgets. Corrected widened-engine credential requirements and cancellation after a mapping is configured. |
| Workspace audit apps | Read scans, checks and remediation against the SDK, app reach, reveal and manifest contracts. Existing scans require judgment for dynamic calls and CSS. |
| Workspace prompt footprint | Checked report fields, totals, use verdicts and load evidence against `engine/prompt_footprint.rs`. |
| Best practices | Checked secret mentions, agent-role column grants, read-only transactions and command-guard lanes. |
| Engine prompts and tool schemas | Read production prompt and argument text. Corrected app layout, trigger layout, tracking exclusions, OAuth modal sequencing and toast suppression. |

The deterministic sweep was rerun. Its five CLI misses are hidden clap commands;
the `update_app` hit explicitly says no such tool exists. Retired aliases are
labelled historical or retired. Other event-shaped hits are domain-event
examples, enum values, a native window event, or historical names.

The new workspace-list tests cover stale ports, registry-only workspaces,
malformed health, redirects, nonlocal origins and token masking. The old helper
fails the stale-ports regression. The replacement passes all eight tests, including private HTTPS trust and clean transport failures.


### Phase 1 review corrections

Claude's hardening review identified four further corrections:

- The prompt now labels `trigger.toml` as a read-only projection and preserves
  the optional `<slug>.md` intent.
- Held-message rows now live in the coding-agent table and explicitly name
  that scope.
- Codegen carries the corrected session-backend comment into the tracked
  TypeScript event wire type.
- The workspace helper accepts `--ca-file` for private HTTPS trust. Its TLS
  regression proves an untrusted certificate fails before credentials are sent.


The phase review also refined warning evidence and attempt counts, notification
suppression outcomes, tracked-content wording, backup choices and proxy header
coverage. New push examples use the offered grouped preferences tool.
The layer-namespace comment now matches the corrected composition guide.
The helper is discoverable from `best-practices.md`; its Python regression
fixture now runs through the ordinary Vitest suite, outside shipped knowhow.


The final phase review confirmed all ten follow-up fixes. Its two low notes
were addressed too: vendor header families apply to every proxy entry, and
helper regressions live in `src/__tests__/`, outside the wire-contract suite.
The regression harness requires Python 3 and OpenSSL for its local TLS fixture.


Hardening also corrected the breaker STOP message and log labels: they no
longer claim that skipped attempts executed and failed. The nearby comments
and frontend rule now use the same attempt semantics. Breaker thresholds and
control flow remain unchanged.


The full engine run exposed two prompt-size regressions and a missing
notification warning phrase. The app-root correction now costs no extra bytes.
The OAuth and tracking guidance is shorter while preserving its rules.
Notification guidance retains the warning and all three existing assertions.
Both prompt ceilings remain unchanged.

## Phase 2 outcomes

The retired-term gate first rejected 72 unqualified mentions in the real tree.
The sweep fixes current wording and qualifies compatibility paths as legacy.
Generic flow fixtures use the current knowhow tool; dedicated replay tests keep
the retired tool's stored spelling.

Six terms live in `docs/retired-terms.tsv`. History, retired glossary headings
and the generated legacy approval verb have reasoned exceptions in
`docs/retired-term-exceptions.tsv`. Migration files and serialized identifiers
keep their spelling. Codegen updates documentation from Rust comments.

Thirteen hermetic fixtures cover qualification, history, literal exceptions,
malformed or missing inputs, tracked-file changes and nested working directories.
Review added quoting, wrapped-phrase, encoding, anchor and backreference regressions.
The gate scans whole content before splitting matched files into source lines.

The gate passes from the root and from `crates/lucidos-app`.
Removing the single new changelog entry restores the original bytes exactly.
Context-budget checks pass without raising any ceiling.
