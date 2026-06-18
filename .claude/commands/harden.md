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
- Phase 4.5 already auto-skips for docs-only via its test-selection table.

Do NOT extend this fast path to "string-only" or "comment-only" `.rs` edits. Strings can carry format args, escape sequences, regexes, or be parsed at runtime — any `.rs` change keeps the full cycle.

## Phase 0.75: Planning-Invariant Backstop

If the diff is complex per `CLAUDE.md` (ADR/design-thread-backed, cross-layer, routing/topology/storage/security/migration/process, or otherwise non-local), verify that the session produced an implementation plan before the first code edit and that final verification maps back to its invariants.

This is a backstop, not the first time invariants should appear. Do not invent a late checklist to justify an already-written diff. If no implementation plan exists for a complex diff, flag it as a `CLAUDE.md` compliance issue in Phase 2 Agent 2 and create one from the available prompt/thread/docs before continuing with review/test work. If the plan exposes missing verification or a violated invariant, treat that as a real hardening finding and fix or verify it before Phase 5.

## Phase 1: Run /code-review

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this phase entirely and proceed to Phase 2.

Run the **repo-owned** `code-review` skill (`.claude/skills/code-review/SKILL.md`) at **medium** effort. It reviews the branch diff for correctness bugs at the high-confidence end of the precision/recall slider — fewer findings, very low false-positive rate, complementary to Phase 2's broader bug-detection agent.

- **Claude Code:** invoke via the Skill tool — `Skill skill: "code-review" args: "medium"`. The project skill overrides Claude Code's built-in of the same name, so this resolves to the repo copy.
- **Codex / any agent without a Skill tool:** the skill is NOT in your available-skills list (it lives only on disk). **Read `.claude/skills/code-review/SKILL.md` and follow its phases directly** against the branch diff at medium effort. Do not skip Phase 1 just because the Skill tool can't find it.

(Background: this phase used to invoke Claude Code's built-in `code-review` skill — the renamed `/simplify`. Built-in and plugin skills are invisible to Codex, which only sees skills on disk under `.claude/skills/`, so a Codex `/harden` run couldn't find it. The procedure is now vendored into `.claude/skills/code-review/` so both backends run the same phases; the apply step lives here in the harden orchestrator, not in the skill.)

When `code-review` returns its findings:

- **No bugs flagged:** record that, proceed to Phase 2.
- **Bugs flagged:** for each finding, read the cited file, confirm the bug is real (false-positive triage — same standard as Phase 3 validation), and **fix the real ones directly**. Skip findings that are false positives, depend on uncertain runtime state, or duplicate something Phase 2 will catch better.
- **Commit any fixes** before proceeding to Phase 2 — Phase 2's diff input needs to include them.

Do NOT pass `--comment` (that mode posts to GitHub PRs, which Lucidos does not use).

**Report Phase 1 in prose — never reproduce the raw findings JSON.** The
`code-review` skill's Output section tells you to emit its findings as a JSON
array, and to `return []` when nothing survives. That array is the skill's
*internal* contract for handing results back to this orchestrator — it is NOT
for the reader. Translate it into one sentence of prose ("Phase 1: no findings"
or "Phase 1 flagged N issues: …") and **do not paste the array — empty `[]`,
`{}`, or populated — into your reply, fenced or inline.** A bare `[]` in the
chat is meaningless noise to the user. (This is the source we own: the `[]` you
see rendered in old threads is this array leaking through; suppressing it here
is why the frontend no longer carries a strip-the-empty-array workaround.)

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

Do NOT flag:
- General best practices not mentioned in CLAUDE.md
- Issues silenced by lint ignore comments
- Pedantic nitpicks

### Agent 3: Regression Check

For each modified file, run `git log --oneline -10 <file>` to see recent history. Check if the current changes revert, contradict, or undermine recent fixes or intentional refactors. Only flag clear regressions where you can point to a specific prior commit that the new change undoes.

## Phase 3: Validate Findings

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

Run the test suites for the layers touched on this branch.

Pick suites by `git diff main...HEAD --name-only`, applying the CLAUDE.md test-selection table:

- `.rs`, `Cargo.toml`, `Cargo.lock`, `.sql` → `cargo check && cargo test -p lucidos-engine`
- `.ts`, `.tsx` → `cd crates/lucidos-app && npx tsc --noEmit && npm test`
- CSS-only / docs-only → skip
- Mixed → run both **in parallel**

When the diff is mixed, kick the Rust and TS suites off concurrently — they're independent toolchains (cargo vs npm) with no shared state, so running them serially wastes wall-clock. Use the Bash tool's `run_in_background: true` for each, then `TaskOutput` to join. (Codex / any agent without a background-Bash + `TaskOutput` tool: run the two suites **sequentially** instead — `cargo …` then `npm …`. Parallelism is only a wall-clock optimization; sequential gives identical correctness.) Pattern:

```
# Launch both in parallel
Bash(cmd="cargo check && cargo test -p lucidos-engine", run_in_background=true)  → task_id A
Bash(cmd="cd crates/lucidos-app && npx tsc --noEmit && npm test", run_in_background=true)  → task_id B
# Then TaskOutput on A and B until both finish
```

**Never pipe the test command through `| tail` / `| head` / `| grep` to trim output.** Under zsh / bash a pipeline reports the *last* command's exit code, not cargo's — so `cargo test ... | tail` exits 0 even when a Rust test failed, and Phase 4.5 reports a false PASSED on a red run (this has actually shipped a failing nightly). Run each suite un-piped (the `run_in_background` + `TaskOutput` pattern above already preserves the real exit), or if you must trim, redirect to a log and capture `$?` first: `cargo test -p lucidos-engine > /tmp/t.log 2>&1; echo "EXIT: $?"` then read the log. A "tests pass" claim needs the real exit code AND the `test result: ok.` / `0 failed` line — see `/clean-build`'s "Reading exit codes honestly" section for the full mechanism.

If everything passes, proceed to Phase 5.

If anything fails: fix the failures (or the code that caused them), commit the fixes, and **return to Phase 1**. Fixes are new code that hasn't been reviewed by `/code-review` or the hardening agents — re-run the cycle on the updated diff. Iterate until tests pass on a fully-hardened diff.

## Phase 5: Create Marker

After all phases complete (and any bug fixes are applied), record this branch's
HEAD as hardened in the parent workspace's DB:

```bash
.claude/hooks/mark-harden.sh
```

State lives in the `hardened_branches` DB table (keyed by repo root + branch),
not on disk — do not look for or manage any marker files.

Inform the user: "Hardening complete. Session can finish."
