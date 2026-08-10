Harden the code: review changed code for reuse, quality, and efficiency, then fix any issues found. Check for bugs and CLAUDE.md compliance, then run the test suites for what was touched (iterating from Phase 1 if anything fails). Run this before finishing your session.

**No postpone option.** Never tell the user you're "postponing", "deferring", or "skipping" `/harden`. There is no such mode — if you say it, you're misleading them, because Apply will run hardening synchronously when the marker is `MISSING` (and the user waits at that point). Either Phase 0 reports `ALREADY_HARDENED` (say so and stop) or you run the full skill. The only honest answers are "already hardened — skipping" or "running hardening now".

## Phase 0: Check if Already Hardened

`lucidos hardened query` prints `FRESH`, `STALE`, or `MISSING` for the branch
in `$PWD`. `FRESH` means HEAD still matches the SHA recorded by the last
`/harden`; `STALE` means CC has committed since then and a re-run is needed.

```bash
[ "$(lucidos hardened query 2>/dev/null)" = "FRESH" ] && echo "ALREADY_HARDENED" || echo "NOT_HARDENED"
```

If the output is `ALREADY_HARDENED`, inform the user: "Already hardened — skipping." and stop. Do NOT re-run hardening.

## Phase 0.5: Detect Docs-Only Diff

Run `git diff main...HEAD --name-only`. If every changed file ends in `.md` or `.txt`, the diff is **docs-only**. In docs-only mode:

- Skip Phase 1 (`/code-review` looks for code-shaped bugs that don't apply to prose).
- Skip Phase 2 Agent 1 (no code logic to bug-check).
- Phase 2 Agents 2 and 3 (compliance, regression), Phase 3, Phase 4, Phase 5 still run.
- Phase 2.5 auto-skips for docs-only via its own packaged-runtime gate.
- Phase 4.5 already auto-skips for docs-only via its test-selection table.

Do NOT extend this fast path to "string-only" or "comment-only" `.rs` edits. Strings can carry format args, escape sequences, regexes, or be parsed at runtime — any `.rs` change keeps the full cycle.

## Phase 0.75: Planning-Invariant Backstop

If the diff is complex per `CLAUDE.md` (ADR/design-thread-backed, cross-layer, routing/topology/storage/security/migration/process, or otherwise non-local), verify that the session produced an implementation plan before the first code edit and that final verification maps back to its invariants.

This is a backstop, not the first time invariants should appear. Do not invent a late checklist to justify an already-written diff. If no implementation plan exists for a complex diff, flag it as a `CLAUDE.md` compliance issue in Phase 2 Agent 2 and create one from the available prompt/thread/docs before continuing with review/test work. If the plan exposes missing verification or a violated invariant, treat that as a real hardening finding and fix or verify it before Phase 5.

## Phase 1: Run /code-review

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this phase entirely and proceed to Phase 2.

### Phase 1 kickoff: launch Codex review in parallel (advisory, Claude Code only)

Before running the `code-review` skill, kick off a Codex review of the branch diff **in the background** so it overlaps Phases 1–3 and adds ~no wall-clock (median 97s across the 25 recorded runs on this repo, with a tail out to 11 min, so it fits inside the Claude review phases). It is a fourth reviewer running on the *same* cadence as the others: because `/harden` loops (Phase 4.5 failure → back to Phase 1), each iteration launches a fresh Codex review on the updated diff, exactly like `code-review` and the Phase 2 agents re-run.

This step is **advisory**: its findings feed the same validate→fix pipeline as every other reviewer (joined in Phase 3), but Codex being unavailable, slow, erroring, or timing out NEVER blocks the hardened marker.

- **Claude Code only.** A Codex-backed `/harden` run is already Codex reviewing this diff — skip this step and note "Codex review: skipped (Codex-backend run)".
- **Docs-only:** this whole phase is skipped, so Codex review is skipped too (it's a code reviewer, not a prose reviewer).

Resolve the companion script (installed with the `codex` plugin, do not hardcode a path) and launch the review in **one** Bash call, so the resolved path and the launch share a shell (variables do not persist across Bash tool calls). Run that Bash call with the tool's own `run_in_background: true` and join it in Phase 3 with `TaskOutput`. The companion's `--background` flag does NOT work for `review`: it is parsed and then ignored (only `task` honours it), so the flag is deliberately absent below and the parallelism comes from the Bash tool instead. Passing it bought nothing and cost the phase its whole premise, since the call actually blocked for the length of the review. `--base main` matches `/harden`'s diff base of `main...HEAD`:

```bash
CODEX_COMPANION=$(find "$HOME/.claude/plugins" -name codex-companion.mjs -path '*codex*' 2>/dev/null | sort | tail -1)
if [ -z "$CODEX_COMPANION" ]; then
  echo "Codex review: unavailable (plugin not installed), proceeding"
else
  echo "CODEX_COMPANION=$CODEX_COMPANION"
  ready=""
  for probe in 1 2 3; do
    if codex --version > /dev/null 2>&1 && codex app-server --help > /dev/null 2>&1; then ready=1; break; fi
    if [ "$probe" != 3 ]; then
      echo "Codex CLI not answering (probe $probe of 3), an update may be rewriting it, retrying in 30s"
      sleep 30
    fi
  done
  if [ -n "$ready" ]; then
    node "$CODEX_COMPANION" review --scope branch --base main --json
  else
    echo "Codex review: unavailable (CLI not answering after 3 probes), proceeding"
  fi
fi
```

**The probe loop is not defensive padding.** The companion runs exactly those two
checks before it will review, and collapses any failure of either into one generic
`Codex CLI is not installed or is missing required runtime support`, discarding the
real reason. An `npm install -g` of the Codex CLI empties and rewrites the directory
the `codex` symlink points into, so for a few seconds mid-update neither check
answers and the reviewer is written off for the whole run. Every recorded Codex
failure on this repo is that one error, each returning in under a quarter of a
second, against 25 reviews that completed. Retrying rides the window out instead,
and costs nothing on the normal path because the whole call is backgrounded.

**Record the Bash task id: that is the handle for the Phase 3 join.** There is no companion job id to capture, because the review runs in the foreground of that backgrounded shell and prints its findings only when it finishes. If the plugin was unavailable, or all three probes failed, there is nothing to join. Do NOT wait for the review here, continue immediately into the `code-review` skill below.

### Phase 1 review

Run the **repo-owned** `code-review` skill (`.claude/skills/code-review/SKILL.md`) at **medium** effort. It reviews the branch diff for correctness bugs at the high-confidence end of the precision/recall slider — fewer findings, very low false-positive rate, complementary to Phase 2's broader bug-detection agent.

- **Claude Code:** invoke via the Skill tool — `Skill skill: "code-review" args: "medium"`. The project skill overrides Claude Code's built-in of the same name, so this resolves to the repo copy.
- **Codex / any agent without a Skill tool:** the skill is NOT in your available-skills list (it lives only on disk). **Read `.claude/skills/code-review/SKILL.md` and follow its phases directly** against the branch diff at medium effort. Do not skip Phase 1 just because the Skill tool can't find it.

(Background: this phase used to invoke Claude Code's built-in `code-review` skill — the renamed `/simplify`. Built-in and plugin skills are invisible to Codex, which only sees skills on disk under `.claude/skills/`, so a Codex `/harden` run couldn't find it. The procedure is now vendored into `.claude/skills/code-review/` so both backends run the same phases; the apply step lives here in the harden orchestrator, not in the skill.)

When `code-review` returns its findings:

- **No bugs flagged:** record that, proceed to Phase 2.
- **Bugs flagged:** for each finding, read the cited file, confirm the bug is real (false-positive triage — same standard as Phase 3 validation), and **fix the real ones directly**. Skip findings that are false positives, depend on uncertain runtime state, or duplicate something Phase 2 will catch better.
- **Commit any fixes** before proceeding to Phase 2 — Phase 2's diff input needs to include them.

Do NOT pass `--comment` (that mode posts to GitHub PRs, which Lucidos does not use).

**Report Phase 1 in prose — the findings are structured data, not a message.**
The `code-review` skill reports its findings through a **structured channel** (its
Output section): the `ReportFindings` tool on Claude Code — which renders them
structurally, never as text — or, for backends without that tool, an in-band
array it hands back with an explicit `No findings.` in the empty case. Either
way the findings are the skill's handoff to *you*, NOT text for the reader:
translate the result into one sentence of prose ("Phase 1: no findings" or
"Phase 1 flagged N issues: …") and **never paste a findings array — empty `[]`/`{}`
or populated, fenced or inline — into your reply.** A bare `[]` in the chat is
meaningless noise; it is the recurring bug the structured-channel handoff exists
to prevent. The fix is deliberately at the **source** (the repo-owned skill),
NOT a frontend content-filter — see `docs/temporary-measures.md` § "code-review
findings array leaking into chat".

## Phase 2: Run Three Hardening Agents

Run `git diff main...HEAD` to get the current diff (including any Phase 1 fixes). Also run `git diff main...HEAD --name-only` to get the list of changed files. Run the three angles below:

**Subagents are optional — the angles are not.** Mirrors the `code-review` skill's contract:

- **Claude Code:** launch the three agents as parallel subagents via the Task tool (faster, independent perspectives).
- **Codex / any agent without a Task tool:** you have NO subagent capability — do NOT try to spawn agents, and do NOT improvise a "simulated parallel" pass (that interleaves output and stalls the turn, which is exactly how a Codex `/harden` run dies right after Phase 1). Run all three angles **yourself, inline and sequentially** — Agent 1, then Agent 2, then Agent 3 — in this same session, collecting findings as you go. The analysis and output are identical; only the execution is serial. Then continue to Phase 3 in the same turn — do not stop or idle until Phase 5 has written the marker.

### Agent 1: Bug Detection

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this agent (Agents 2 and 3 still run).

Scan the diff for bugs and incorrect logic that Phase 1's `code-review medium` would have missed at its high-confidence threshold. Tag each finding with a severity:

- 🔴 **Bug** — will break production, must fix before merging
- 🟡 **Nit** — worth fixing but not blocking

Do NOT use a "pre-existing" category. If a bug exists in a file touched by this branch, it must be classified as 🔴 or 🟡 based on severity — "it was already broken" is not a valid excuse to skip it.

Focus on:
- Code that will fail to compile or parse (syntax errors, type errors, missing imports, unresolved references)
- Code that will definitely produce wrong results regardless of inputs (clear logic errors)
- Security vulnerabilities in the changed code (injection, auth bypass, data exposure)

**HIGH SIGNAL ONLY.** Do NOT flag:
- Code style or quality concerns (already covered by Phase 1 `code-review`)
- Potential issues that depend on specific inputs or state
- Subjective suggestions or improvements
- Anything listed in `docs/code-review-priors.md` — the ledger of patterns
  already flagged by past reviews and dismissed with evidence (guarded byte
  slices, documented catch-silencers, deliberate `[]`-until-loaded filters,
  …). Include the file in the agent's prompt. Re-flagging a prior requires
  NEW evidence that the guard/contract changed, not re-derivation of the
  original suspicion.

### Agent 2: CLAUDE.md Compliance

Check the changes against all applicable CLAUDE.md files (root and any in directories containing modified files). Flag only clear, unambiguous violations where you can quote the exact rule being broken.

This includes **`.claude/rules/no-private-data.md`** — flag any private/personal/company-internal data the diff introduces into a shipping file (everything except `docs/plans/**` and `WORKSPACES.md` ships publicly, test fixtures and comments included). That rule is the single source of truth for the definition, the attribution carve-out, and the approved placeholders; flag against it and name the placeholder to use. (The `code-review` skill from Phase 1 carries the same check as a review angle — this agent is the compliance-side backstop.)

It ALSO includes **`.claude/rules/temporary-measures.md`** — the **temporary-measures & marker-hygiene** check (one check, two faces). Apply the inclusion test to the diff: *does it add something meant to go away with a concrete condition for when?* If the diff introduces an impermanent thing — a `remove after X` / `diagnostic-only` / `temporary` / `workaround until …` comment, a new feature flag / kill-switch, a sunset back-compat shim, **OR** a bare `TODO` / `FIXME` / `HACK` / `XXX` marker — it MUST have a matching row in `docs/temporary-measures.md` (in the right typed section, with a concrete removal condition; for a measure, a parent-investigation id). Flag any such addition that lacks a registry row. **The escape valve is to register it — not to delete or reword the marker.** This closes the exact loophole the rule exists for: rewording a `TODO: remove after X` into a plain `// remove after X` comment dodged tracking, so a plain impermanence comment is treated the same as a raw marker — both need a row. Conversely, do NOT flag things on the rule's OUT list (permanent back-compat / old-data tolerance, site-local `#[allow(...)]` / `@ts-expect-error` / `eslint-disable` suppressions, ADR-recorded design decisions, open-ended tech debt) — those are tracked elsewhere or not at all.

Do NOT flag:
- General best practices not mentioned in CLAUDE.md
- Issues silenced by lint ignore comments
- Pedantic nitpicks

### Agent 3: Regression Check

For each modified file, run `git log --oneline -10 <file>` to see recent history. Check if the current changes revert, contradict, or undermine recent fixes or intentional refactors. Only flag clear regressions where you can point to a specific prior commit that the new change undoes.

## Phase 2.5: Packaged-Runtime Dependency & Fail-Fast Check

**Gate:** run this phase only when `git diff main...HEAD` touches a packaged-runtime surface; otherwise note "Phase 2.5: not applicable" and skip. The trigger surfaces are any diff that:

- adds or changes a subprocess spawn (`Command::new(...)`, `tokio::process::Command::new(...)`), an MCP server `command:` entry, a `--permission-prompt-tool` / hook `command`, or any other place that shells out by name;
- reads a file/asset from disk at runtime, or flips an asset between `include_str!`/`include_bytes!` (baked) and disk-read (staged);
- adds or reads a `LUCIDOS_*_DIR` / `*_BIN` env var, or a `current_exe()`-relative path walk;
- edits the packaging / delivery / install contract: `scripts/lib/stage_runtime.sh` (`RESOURCE_NAMES`, `stage_runtime_assemble`), `scripts/build-dmg.sh`, `scripts/build-headless.sh`, `scripts/lib/service.sh`, `install.sh`, `crates/lucidos-app/src/desktop.rs` (`spawn_gateway` env), or the gateway's engine / embedded-Postgres provisioning.

**Why:** the packaged macOS `.app` / headless tarball / `install.sh` service run under a minimal launchd/Finder PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) and stage only a fixed `RESOURCE_NAMES` set — NOT the dev `target/{debug,release}/` tree with every binary side-by-side and a rich shell PATH. A dependency that resolves in dev can be absent or unreachable when packaged, and the failure typically surfaces as a cryptic mid-stream tool error or an indefinite boot-splash hang instead of a clear message. (The triggering incident: the `lucidos` CLI — needed for CC's permission MCP server — was not staged, so the first tool call died with `MCP tool … not found`. Worked catalog of this whole class: `data/artifacts/audits/packaged-app-bundle-and-failfast-audit.md`.)

For each runtime dependency the diff adds or changes, confirm BOTH:

- **Pattern A — staged + resolved, never PATH-dependent.** A binary/asset/dir the runtime requires must be (a) staged by EVERY delivery vehicle (`stage_runtime.sh` `RESOURCE_NAMES` + `stage_runtime_assemble`, surfaced in `desktop.rs::bundled_resources` / the service env) and (b) resolved by absolute path or a guaranteed-set env var — never a bare command name relying on PATH. A genuinely external user-install (`git`, `claude`, `codex`, `node`, `npx`, a non-bundled `psql`) is acceptable ONLY if it is resolved like `resolve_claude_binary` (probe absolute locations first) AND its absence fails fast with an actionable message. Flag any bare `Command::new("name")` / `command: "name"` whose target is not on the launchd minimal PATH and not absolute-resolved, and any disk-read asset no vehicle stages.
- **Pattern B — fail fast, don't degrade.** A missing dep / `None` cli-dir / `Err` spawn / missing-file / unreachable-process must surface as an immediate, descriptive error at spawn or boot ("X not found at <path> — <feature> unavailable"), NOT a `log!`-and-proceed, a silent stub, or an unbounded wait. Flag any resolution that returns `None`/`Err` then proceeds anyway, any health check that reports healthy while a required surface is broken, and any "is the env var set?" check that never verifies the path actually exists.

If the diff edits `RESOURCE_NAMES` or a staging/service/spawn-env path, also confirm the change is mirrored across ALL delivery vehicles (dmg, headless, install service) and the `desktop.rs` ↔ `service.sh` env contract — they must not drift. Validate flagged items in Phase 3 and fix real ones in Phase 4 like any other finding.

## Phase 3: Validate Findings

### Join the Codex review (if launched in Phase 1)

If you launched a background Codex review in Phase 1, join it now with `TaskOutput` on the Bash task id you recorded there. Its stdout is the review itself, not a status line, so there is nothing further to poll.

- **Completed:** fold Codex's findings into the validation set below. Treat each finding exactly like one from the other reviewers (confirm against source, fix real 🔴 in Phase 4, discard false positives, log recurring dismissals to `docs/code-review-priors.md`). Codex frequently returns "no actionable bugs": record that outcome and move on.
- **Still running:** give it a bounded wait with `TaskOutput` `block: true`, until it completes or until ~5 minutes have elapsed *since it was launched in Phase 1* (usually it is already done, since Phases 1 to 2 ran in parallel with it). Remember the probe loop can hold the task for up to a minute before the review even starts.
- **Failed / unavailable / plugin not installed:** it is advisory. Note "Codex review: unavailable (advisory), proceeding" and continue. NEVER block the marker or stall the turn on Codex. (If a prior iteration's Codex task is still running when a new one launches, you may abandon the stale one.)

Then validate every finding (Codex's included) per the rest of this phase.

### Validate every finding

Once all three angles are done (parallel subagents joined, or — for Codex / any agent without subagents — your own three inline passes complete), validate each issue found. **Per the same subagents-are-optional rule:** Claude Code launches a parallel validation subagent per finding; Codex / any agent without a Task tool validates each finding **inline and sequentially** in this same session. Either way the validator must:
- Read the relevant source files (not just the diff)
- Confirm the issue actually exists in the code
- Discard findings that are false positives or depend on assumptions about runtime state

Only issues confirmed by validation proceed to the report.

When validation dismisses a finding about a **pattern likely to be re-flagged
by future reviews** (a guarded construct that looks unguarded, a documented
deliberate behavior that looks like a bug), add a pattern-based entry to
`docs/code-review-priors.md` in the same change — that ledger is what keeps
the next review round from re-litigating it. One-off misreadings of the diff
don't need an entry; recurring-shaped ones do.

## Phase 4: Report and Fix

- If **no validated issues**: report "No bugs or compliance issues found."
- If **validated issues found**: list each issue grouped by severity (🔴 Bug, 🟡 Nit) with file, line, and description. Fix 🔴 bugs directly. Ask the user about 🟡 nits.

Commit any fixes from this phase before proceeding to Phase 4.5.

## Phase 4.5: Verify Tests Pass

### Always first: the em-dash gate

```bash
./scripts/check-em-dashes.sh
```

Runs for **every** diff, with no fast path: the docs-only skip below does NOT apply to it, because prose is exactly where em dashes come from. It is diff-scoped and added-lines-only, so the ~29,000 pre-existing ones in the tree never fire it (see `.claude/rules/no-em-dashes.md`). A non-zero exit is a hardening failure like any other: fix the flagged lines, commit, and return to Phase 1.

This is also the layer that covers **Codex**, which has no `PreToolUse` hooks and therefore never met the write-time gate.

### Also always: the ADR gate

```bash
./scripts/check-adrs.sh
```

Also runs for **every** diff, not only ones touching `docs/adr/`. It is a
whole-tree consistency check costing milliseconds, and running it
unconditionally is what catches a duplicate ADR number that arrived through a
**merge** rather than through this branch's own edits.

`docs/adr/index.md` carries `merge=union` (see `.gitattributes`), which is what
stops two branches appending an ADR line from conflicting, and that conflict was
a recurring, guaranteed tax before 2026-08-04. Union keeps both lines but
neither orders nor deduplicates them, so this gate covers the rest: a duplicate
number (silent, because two ADRs with different filenames merge clean), an ADR
with no index line or an index line with no ADR, an index left out of order by a
union merge, and a missing required section.

Out of order is the one it can repair: `./scripts/check-adrs.sh --fix`. A
duplicate number is reported with both paths and the next free number, never
auto-renumbered, because deciding which references are live and which are
historical narration needs judgment. New ADRs come from `./scripts/adr-new.sh`,
which allocates across `main`, every unmerged branch, and the working tree, and
therefore cannot collide.

### Also always: the context-budget gate

```bash
./scripts/check-context-budget.sh
```

Also runs for **every** diff, and docs-only diffs least of all are exempt: a
docs-only diff is exactly the change that grows this set, so skipping it there
would exempt the only change that can break it.

Two arms, both hard. **Size**: the always-loaded instruction set (`CLAUDE.md`
plus every unscoped `.claude/rules/*.md`) must stay at or under
`CONTEXT_BUDGET_CEILING`. Every byte is paid on every request of every session,
before the agent has read a line of code, and the set grew 98% in the seven
weeks to 2026-08-06 because everyone appends and nobody deletes.
**Membership**: the resident set must be exactly the declared list. That arm is
the regression detector for a rule meant to be path-scoped that silently is
not, which is not a hypothetical: every rule file used the `globs:` key (a
Cursor convention Claude Code ignores) until 2026-07-25, so the whole set was
resident in every session and nothing said so.

Whole-tree, not diff-scoped, unlike the em-dash gate above. What a session
loads today does not depend on which branch grew it, so a merge that pushes the
total over is caught by the next branch to run the gate, the same reasoning as
`check-adrs.sh`.

`./scripts/check-context-budget.sh --report` prints the set without failing.
Fixing a size failure means moving content, not raising the number: reference
material to a skill (loads on invocation), a convention to a path-scoped rule
(loads on a matching Read), maintainer prose to `docs/agent-config.md` (never
loads). Raising `CONTEXT_BUDGET_CEILING` is allowed and is a deliberate act:
say in the commit message what became worth paying for on every request.

### Also always: the mirrored-rule gate

```bash
./scripts/check-prompt-mirror.sh
```

Also whole-tree and also unconditional, and a docs-only diff is again the one
that can break it.

Two instruction surfaces reach every coding-agent session, and
`docs/agent-config.md` § Which surface owns a rule splits them so a rule lands
on exactly one. Exactly one rule cannot: the process-safety prohibition
(ADR 0025) binds both a session with no Lucidos checkout, which only the engine
system prompt reaches, and a hand-run `claude`, which only `CLAUDE.md` reaches.
So it is stated on both surfaces on purpose, and this gate fails if either half
loses the prohibition.

Shell rather than a Rust test because the failure mode is a `CLAUDE.md`-only
edit, which never triggers `cargo test`. It also reaches Codex, which has no
hooks. Fix a failure by restoring the missing half, never by deleting the other
one. Adding a *second* mirror needs the proof spelled out in
`scripts/lib/prompt_mirror_scan.sh`.

### Also always: registered hooks can actually run

```bash
./scripts/lib/hooks_registered_test.sh
```

Asserts `.claude/settings.json` is valid JSON and that every hook it registers
resolves to a file that exists and is executable. Milliseconds, and
unconditional for the same reason as the two gates above.

It exists because a hook committed at mode 100644 is invisible when it fails:
several events ignore the hook's exit code by design, and hook stdout never
reaches the transcript, so a permission error goes nowhere. That shipped on
2026-08-06 with `log-instructions-loaded.sh`. A hook that silently never runs
looks exactly like a hook that was never added.

### Then the test suites

**Join the Codex review before you start them.** The engine suite includes
`runtime::codex::driver_tests`, which drives a real `codex app-server`, and a
Codex review is another client of the same CLI. Run them together and those
tests fail: on 2026-08-10 a merge-hardening run overlapped the two and got
twelve failures in that module out of 5,325, on a run that took 539s against
the usual 82s, and all seven passed in isolation immediately after. The
failures look like real breakage in the diff and are not, so the cost is a
wasted debugging pass on a red suite that was never red.

Phase 1's review is already joined in Phase 3, so the ordinary flow is safe.
The way in is launching a SECOND review (a later `/harden` iteration, or a
re-review after fixes) and then starting the suite while it runs. If one is in
flight, wait for it. Never treat a `runtime::codex::driver_tests` failure as a
finding until you have re-run that module alone with no Codex process active.

Run the test suites for the layers touched on this branch.

Pick suites by `git diff main...HEAD --name-only`, applying the CLAUDE.md test-selection table:

- `.rs`, `Cargo.toml`, `Cargo.lock`, `.sql` → `make lint && make test`
- `.sh`, `.shellcheckrc`, `Makefile` → `make lint`
- `.ts`, `.tsx` → `cd crates/lucidos-app && npx tsc --noEmit && npm test`
- `.css` under `crates/lucidos-app/src/` → `cd crates/lucidos-app && npx vite build`
- `crates/lucidos-engine/src/api/sdk_iframe.css` → `cd crates/lucidos-app && npm test`
- Docs-only → skip
- Mixed → run both **in parallel**

**CSS is not a skip.** Until 2026-08-05 this table said "CSS-only → skip", and
nothing else in the gate parses CSS: `tsc` ignores it, Vitest never built it.
So a syntax error passed every phase and landed on `main`, where it kills the
checkout-shared build-watch's `vite build`. The watch keeps serving the previous
`dist/` and republishes nothing, so the next frontend-only Apply times out in
`engine::frontend_refresh` and the user gets "Frontend change applied but not
served yet", naming the build-watch instead of the CSS file that broke it, for a
change that may not touch CSS at all. `npx vite build` is sub-second and is the
exact command the build-watch runs, so it fails on precisely what the watch will
fail on. Add it to the parallel launch when the diff also touches Rust or TS.

The two CSS surfaces need **different** gates, which is why they are separate
rows. `sdk_iframe.css` is `include_str!`d by `api/sdk.rs` and served to every
app iframe, so it is outside the Vite graph and `vite build` never reads it; a
syntax error there ships silently as app chrome that stops being styled. It is
covered instead by `styles/__tests__/engine-served-css-parses.test.ts`, a
postcss parse under the ordinary Vitest run. Neither gate subsumes the other:
`vite build` resolves `@import` (which a per-file parse cannot), and the guard
reaches a file `vite build` cannot see.

**`make lint`, not `cargo check` — this is THE per-change lint gate.** Lucidos is
not PR-based: Apply merges the branch into `main` directly, so there is no CI
stage between a change and `main`. `/harden` *is* that stage — it runs before
every push (`.claude/hooks/pre-push.sh`) and synchronously at Apply when the
marker is missing. Until 2026-07-29 this table said `cargo check`, which
compiles but runs **no clippy lints**, so the only thing that ever ran the
warnings-as-errors gate was the nightly — and the 2026-07-26 nightly duly found
NINETEEN lints accumulated across three weeks of ordinary commits. `make lint`
(ShellCheck, then `cargo fmt --all --check`, then clippy with `CLIPPY_FLAGS`;
see the `Makefile`) strictly supersedes `cargo check`: same compile, plus the
lint set, plus every tracked `*.sh`, plus a rustfmt-clean tree. It is the single
canonical invocation; never restate its flags here.

When the diff is mixed, kick the Rust and TS suites off concurrently — they're independent toolchains (cargo vs npm) with no shared state, so running them serially wastes wall-clock. Use the Bash tool's `run_in_background: true` for each, then `TaskOutput` to join. (Codex / any agent without a background-Bash + `TaskOutput` tool: run the two suites **sequentially** instead — `cargo …` then `npm …`. Parallelism is only a wall-clock optimization; sequential gives identical correctness.) Pattern:

```
# Launch both in parallel
Bash(cmd="make lint && make test", run_in_background=true)  → task_id A
Bash(cmd="cd crates/lucidos-app && npx tsc --noEmit && npm test", run_in_background=true)  → task_id B
# Then TaskOutput on A and B until both finish
```

**Never pipe the test command through `| tail` / `| head` / `| grep` to trim output.** Under zsh / bash a pipeline reports the *last* command's exit code, not cargo's — so `cargo test ... | tail` exits 0 even when a Rust test failed, and Phase 4.5 reports a false PASSED on a red run (this has actually shipped a failing nightly). Run each suite un-piped (the `run_in_background` + `TaskOutput` pattern above already preserves the real exit), or if you must trim, redirect to a log and capture `$?` first: `make test > /tmp/t.log 2>&1; echo "EXIT: $?"` then read the log. A "tests pass" claim needs the real exit code AND the `test result: ok.` / `0 failed` line — see `/clean-build`'s "Reading exit codes honestly" section for the full mechanism.

If everything passes, proceed to Phase 5.

If anything fails: fix the failures (or the code that caused them), commit the fixes, and **return to Phase 1**. Fixes are new code that hasn't been reviewed by `/code-review` or the hardening agents — re-run the cycle on the updated diff. Iterate until tests pass on a fully-hardened diff.

## Phase 5: Create Marker

After all phases complete (and any bug fixes are applied), record this branch's
HEAD as hardened in the parent workspace's DB:

```bash
lucidos hardened mark
```

All the git inspection and the HTTP call live in that subcommand, so this stays
a stable one-liner even when the storage scheme changes.

State lives in the `hardened_branches` DB table (keyed by repo root + branch),
not on disk — do not look for or manage any marker files.

Inform the user: "Hardening complete. Session can finish."
