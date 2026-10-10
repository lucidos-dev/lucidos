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

## Phase 0.3: Catch Up With main

Merge `main` into the branch before you review or test anything. Then this
one run certifies the work and the merge together. Otherwise a conflict at
Apply spawns a resolver that hardens everything a second time (ADR 0345).

Run it once per `/harden`, and only after Phase 0 printed `NOT_HARDENED`. A
merge into a `FRESH` branch would make its marker stale.

1. If `git rev-parse -q --verify MERGE_HEAD` succeeds, a merge is already in
   progress. Go to step 4 and finish it, since any commit now concludes it.
2. Commit your own uncommitted work, as its own commit. It must not ride
   inside the merge commit.
3. Merge:

   ```bash
   git merge main --no-edit
   ```

   A clean merge commits itself, and "Already up to date" needs nothing.
   Either way, go on to Phase 0.4.
4. On a conflict, resolve every file. Read both sides and keep what each one
   meant, then `git add` the file. Ask the user when a conflict is ambiguous.
5. Make sure no marker survives. This must print nothing, since `git add`
   accepts a file that still holds markers:

   ```bash
   git diff --cached --name-only --diff-filter=d -z \
     | xargs -0 grep -lE '^(<<<<<<<|>>>>>>>)( |$)' --
   ```

   Then `git commit --no-edit`.

Then go on to Phase 0.4. The diff against `main` is now the branch's own
change, and Phase 0.4 lists this merge when the branch was hardened before.
Phase 4's merge for a failure that also fails on `main` still applies.

## Phase 0.4: Detect a Merge-Only or Incremental Diff

A branch hardened once needs only its new lines reviewed, not its whole diff
again. The earlier run already reviewed every line the branch wrote
(ADR 0295, ADR 0315).

```bash
./scripts/harden-scope.sh "$(lucidos hardened sha 2>/dev/null)"
```

The first line is the answer:

- `MERGE_ONLY`: every commit since the hardened SHA that is not on `main` is
  a merge of `main`.
- `INCREMENTAL`: the branch also has commits of its own since then, such as a
  fix made during a merge session.
- `FULL <reason>`: go on to Phase 0.5 as usual.

The lines after it name what to review: `commit <sha>` (own commits, oldest
first), `merge <sha>`, `overlap <path>`, and `codex-base <sha>`. The review
covers these inputs, and nothing else:

- **The new lines:** one file holding each commit's patch and each merge's
  resolution. `git show --remerge-diff` shows what the resolver wrote against
  git's own conflicted result, including edits to files that merged clean.

  ```bash
  mkdir -p .lucidos && {
    for c in <each commit sha>; do git show --format='commit %H %s' "$c"; done
    for m in <each merge sha>; do git show --remerge-diff --format= "$m"; done
  } > .lucidos/review-target.diff
  ```
- **The overlap:** each `overlap` path is a file the branch changed that a
  merge also changed. Read the branch's change there against main's new code.
  A semantic conflict can merge clean.

The phases then run as follows:

- Phases 0.5 and 0.6 are skipped. Phase 0.75 is skipped for `MERGE_ONLY`,
  which the earlier run settled. For `INCREMENTAL` it checks the new commits.
- Phase 1 kickoff starts the early suite run unchanged. With a `codex-base`
  line, the Codex review runs against that base. `INCREMENTAL` without one
  runs it against `main`, which after Phase 0.3 is the branch's own change. A
  hardened SHA as the base would pull in all of main's new code. `MERGE_ONLY`
  skips it.
- Phase 1 runs `code-review` with `.lucidos/review-target.diff` as its
  target. An empty file means a clean merge with nothing resolved: note
  "Phase 1: clean merge" and move on.
- Phase 2 runs all three angles **inline**, as the small-diff tier does. Bug
  detection and compliance read the review target. The regression angle reads
  the overlap paths and each commit's files.
- Phase 2.5 gates on the review target. Phase 3 validates inline.
- **Phases 4, 4.5 and 5 run unchanged.** Every suite `harden-suites.sh`
  selects for the branch still runs, because a merge that mixes sides needs
  the full suite (`CLAUDE.md`).

**Re-run the check on every iteration.** A Phase 4 fix is a new commit, and
the marker moves only in Phase 5. So after a merge-only pass, the next pass
answers `INCREMENTAL` and reviews the fix together with the merges.

## Phase 0.5: Detect Docs-Only Diff

Run `git diff main...HEAD --name-only`. If every changed file ends in `.md` or `.txt`, the diff is **docs-only**. In docs-only mode:

- Skip Phase 1 (`/code-review` looks for code-shaped bugs that don't apply to prose).
- Skip Phase 2 Agent 1 (no code logic to bug-check).
- Phase 2 Agents 2 and 3 (compliance, regression), Phase 3, Phase 4, Phase 5 still run.
- Phase 2.5 auto-skips for docs-only via its own packaged-runtime gate.
- Phase 4.5 still runs `scripts/harden-suites.sh`, which picks the suites a docs-only diff needs. A `system-knowhow/**` edit runs the always-loaded budget tests, and a compiled-in file such as `CHANGELOG.md` runs the Rust suite.

Do NOT extend this fast path to "string-only" or "comment-only" `.rs` edits. Strings can carry format args, escape sequences, regexes, or be parsed at runtime — any `.rs` change keeps the full cycle.

## Phase 0.6: Detect a Small Diff

A small diff keeps every angle but runs Phase 2 and Phase 3 inline, even on
Claude Code. Three subagents re-reading a one-token diff cost more than the
review itself. The check ignores `.md` and `.txt` files, so a plan or glossary
edit alongside a small fix does not push it over.

```bash
files=$(git diff main...HEAD --name-only -- . ':(exclude)*.md' ':(exclude)*.txt')
lines=$(git diff main...HEAD --numstat -- . ':(exclude)*.md' ':(exclude)*.txt' \
  | awk '{ s += ($1 == "-" ? 0 : $1) + ($2 == "-" ? 0 : $2) } END { print s + 0 }')
n=$(printf '%s\n' "$files" | grep -c .)
if [ "$n" -le 3 ] && [ "$lines" -le 60 ] \
  && ! printf '%s\n' "$files" | grep -qE '\.(rs|sql|sh)$|(^|/)(Cargo\.(toml|lock)|Makefile)$'; then
  echo "SMALL_DIFF"
else
  echo "FULL_DIFF"
fi
```

In small-diff mode:

- Phase 1 runs unchanged, Codex review included. It is the phase that finds
  real bugs in small CSS diffs, such as a forced-colors regression.
- Phase 2 runs all three angles **inline and sequentially**, as the Codex
  bullet there describes.
- Phase 3 validates each finding inline rather than with a subagent per finding.
- Phase 2.5, Phase 4, Phase 4.5 and Phase 5 run unchanged.

**Re-run the check on every iteration.** A fix that grows the diff past the
threshold sends the next pass through the full procedure.

A `.rs`, `.sql`, `.sh`, `Cargo.toml`, `Cargo.lock` or `Makefile` change is never
small, whatever its size. The reason is the one Phase 0.5 gives for `.rs`
strings, and it also keeps every Phase 2.5 surface out of this mode.

## Phase 0.75: Planning-Invariant Backstop

If the diff is complex per `CLAUDE.md` (ADR/design-thread-backed, cross-layer, routing/topology/storage/security/migration/process, or otherwise non-local), verify that the session produced an implementation plan before the first code edit and that final verification maps back to its invariants.

This is a backstop, not the first time invariants should appear. Do not invent a late checklist to justify an already-written diff. If no implementation plan exists for a complex diff, flag it as a `CLAUDE.md` compliance issue in Phase 2 Agent 2 and create one from the available prompt/thread/docs before continuing with review/test work. If the plan exposes missing verification or a violated invariant, treat that as a real hardening finding and fix or verify it before Phase 5.

## Phase 1: Run /code-review

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this phase entirely and proceed to Phase 2.

### Phase 1 kickoff: start the early suite run (Claude Code only)

The Phase 4.5 test suites start now and run alongside the review phases
(ADR 0292). Format first, so the start commit is the one the suites test:

```bash
./scripts/harden-suites.sh stop
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "Uncommitted changes: commit them, then re-run this step."
elif git diff main...HEAD --name-only | grep -q '\.rs$'; then
  make fmt && { git diff --quiet || git commit -qam "style: rustfmt"; }
fi
```

Then launch the suites in their own Bash call, behind a shell `&` with its
output redirected. Claude Code's own background mode is off under Lucidos
(ADR 0358), so the shell does the backgrounding:

```bash
mkdir -p .lucidos && ./scripts/harden-suites.sh start --early > .lucidos/harden-suites-start.log 2>&1 &
```

- **Codex-backend runs skip this step.** They have no background Bash, so they
  start the suites at Phase 4.5 and wait at once.
- **`stop` first is not padding.** It ends a run left over from an earlier
  iteration, since two cargo runs in one worktree can OOM the host.
- **The script decides what is selected.** A diff that selects nothing prints
  so, and Phase 4.5's verdict is then an empty PASS.

**While the early run is going, two rules hold until the Phase 4.5 join:**

- **Stop it before the first fix outside `HARDEN_SAFE_PATHS`.** That regex
  in `scripts/lib/harden_suites.sh` names the plans and ADR folders, the priors
  ledger and the temporary-measures registry. Run
  `./scripts/harden-suites.sh stop` before editing anything else. Tests read
  the tree while they run, so an edit mid-run tests a mix of old and new code.
  Each suite records what changed when it exits, so the verdict voids a run
  whose edit is still there. An edit made and undone mid-run leaves no trace,
  which is why the stop is the rule.
- **Commit with explicit paths, never `git commit -a`.** An engine test
  briefly rewrites the two `VERSION` files and restores them. An `-a` commit
  during the run can capture that temporary content.

### Phase 1 kickoff: launch Codex review in parallel (advisory, Claude Code only)

Before running the `code-review` skill, kick off a Codex review of the branch diff **in the background** so it overlaps Phases 1–3 and adds ~no wall-clock (median 97s across the 25 recorded runs on this repo, with a tail out to 11 min, so it fits inside the Claude review phases). It is a fourth reviewer running on the *same* cadence as the others: because `/harden` loops (Phase 4.5 failure → back to Phase 1), each iteration launches a fresh Codex review on the updated diff, exactly like `code-review` and the Phase 2 agents re-run.

This step is **advisory**: its findings feed the same validate→fix pipeline as every other reviewer (joined in Phase 3), but Codex being unavailable, slow, erroring, or timing out NEVER blocks the hardened marker.

- **Claude Code only.** A Codex-backed `/harden` run is already Codex reviewing this diff — skip this step and note "Codex review: skipped (Codex-backend run)".
- **Docs-only:** this whole phase is skipped, so Codex review is skipped too (it's a code reviewer, not a prose reviewer).
- **Merge-only or incremental (Phase 0.4):** with a `codex-base <sha>` line, prefix the script in the launch call below with `CODEX_BASE=<sha>`. `INCREMENTAL` without one keeps the default base `main`. `MERGE_ONLY` skips this step: note "Codex review: skipped (merge-only)", since the earlier run already covered the branch.

`./scripts/harden-codex-review.sh` runs the review. It resolves the companion installed with the `codex` plugin, so never hardcode a path. Run the launch below as **one** Bash call. The subshell runs behind a shell `&`, because Claude Code's own background mode is off under Lucidos (ADR 0358). It writes the review to `.lucidos/codex-review.out` and drops `.lucidos/codex-review.done` when it ends, which is what Phase 3 joins on: Claude Code has no blocking wait tool. `CODEX_BASE` defaults to `main`, matching `/harden`'s diff base of `main...HEAD`:

```bash
mkdir -p .lucidos && rm -f .lucidos/codex-review.done
( ./scripts/harden-codex-review.sh > .lucidos/codex-review.out 2>&1; touch .lucidos/codex-review.done ) > /dev/null 2>&1 &
```

The script header documents what it guarantees:

- **The last line of the output is always one status line**: a verdict, `NO VERDICT` with the upstream cause, or `unavailable`.
- **It probes the CLI up to three times first.** An `npm install -g` of the Codex CLI breaks both of the companion's readiness checks for a few seconds.
- **It retries a reviewer that returns no verdict once.**
- **It never passes the companion's `--background` flag**, which `review` parses and then ignores. The parallelism comes from the shell `&`.

**The `.done` marker is the handle for the Phase 3 join.** There is no companion job id to capture. Do NOT wait for the review here, continue immediately into the `code-review` skill below.

### Phase 1 review

Run the **repo-owned** `code-review` skill (`.claude/skills/code-review/SKILL.md`) at **medium** effort. It reviews the branch diff for correctness bugs at the high-confidence end of the precision/recall slider — fewer findings, very low false-positive rate, complementary to Phase 2's broader bug-detection agent.

- **Claude Code:** invoke via the Skill tool — `Skill skill: "code-review" args: "medium"`. The project skill overrides Claude Code's built-in of the same name, so this resolves to the repo copy.
- **Codex / any agent without a Skill tool:** the skill is NOT in your available-skills list (it lives only on disk). **Read `.claude/skills/code-review/SKILL.md` and follow its phases directly** against the branch diff at medium effort. Do not skip Phase 1 just because the Skill tool can't find it.

(Background: this phase used to invoke Claude Code's built-in `code-review` skill — the renamed `/simplify`. Built-in and plugin skills are invisible to Codex, which only sees skills on disk under `.claude/skills/`, so a Codex `/harden` run couldn't find it. The procedure is now vendored into `.claude/skills/code-review/` so both backends run the same phases; the apply step lives here in the harden orchestrator, not in the skill.)

When `code-review` returns its findings:

- **No bugs flagged:** record that, proceed to Phase 2.
- **Bugs flagged:** for each finding, read the cited file and confirm the bug is real, to the Phase 3 validation standard. Then **fix the real ones directly**. Skip findings that are false positives, depend on uncertain runtime state, or duplicate something Phase 2 will catch better. Run `./scripts/harden-suites.sh stop` before the first edit, per the kickoff rules above.
- **Commit any fixes** before proceeding to Phase 2, since Phase 2's diff input needs to include them. Name the files: `git add <file>`, never `git commit -a`.

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

- **Small diff (Phase 0.6), merge-only or incremental (Phase 0.4):** run the three angles inline, as the Codex bullet below describes, on any backend.
- **Claude Code:** launch the three agents as parallel subagents, all three in ONE assistant message (faster, independent perspectives). Under Lucidos each `Agent` call blocks and hands you its report inline, because Claude Code's background mode is off (ADR 0358). Do not pass `run_in_background`: the parameter does not exist, so the call fails. One message keeps the three parallel. Never wait by launching a filler agent, sleeping, or asking a placeholder question.
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

It ALSO includes **`.claude/rules/temporary-measures.md`**: the **temporary-measures & marker-hygiene** check (one check, three faces). Apply the inclusion test to the diff: *does it add something meant to go away with a concrete condition for when?*

The first two faces are an impermanent thing and a bare marker. The impermanent thing is a `remove after X` / `diagnostic-only` / `temporary` / `workaround until …` comment, a new feature flag or kill-switch, or a sunset back-compat shim. The marker is a bare `TODO` / `FIXME` / `HACK` / `XXX`. Either MUST have a matching row in `docs/temporary-measures.md`: the right typed section, a concrete removal condition, and for a measure a parent-investigation id. Flag any such addition that lacks a row. **The escape valve is to register it, not to delete or reword the marker.**

This closes the loophole the rule exists for. Rewording a `TODO: remove after X` into a plain `// remove after X` comment dodged tracking. So a plain impermanence comment counts the same as a raw marker, and both need a row. Do NOT flag things on the rule's OUT list, which are tracked elsewhere or not at all: permanent back-compat or old-data tolerance, site-local suppressions (`#[allow(...)]`, `@ts-expect-error`, `eslint-disable`), ADR-recorded design decisions, and open-ended tech debt.

The third face is the **defending comment**, per `.claude/rules/prose.md` § "What a comment is for". Flag any added or changed comment that argues a shortcut, workaround or known-wrong behaviour is acceptable ("fine for now", "good enough", "deliberately skipped"). It is the same laundering one step further: the impermanence is gone from the wording, not from the code. Fix the root cause, or register the gap with a removal condition, or record it as a known limitation in a plan or ADR. A comment that states an invariant ("callers must hold the lock") is not a finding. Diff-scoped only: never flag an untouched comment in the existing tree.

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
- edits the packaging / delivery / install contract: `scripts/lib/resource_contract.sh` (`resource_contract_names`, the one `RESOURCE_NAMES` source), `scripts/lib/stage_runtime.sh` (`stage_runtime_assemble`), `scripts/build-dmg.sh`, `scripts/build-headless.sh`, `scripts/lib/service.sh`, `install.sh`, `crates/lucidos-app/src/desktop.rs` (`spawn_gateway` env, the `*_RESOURCE_NAME` constants), or the gateway's engine / embedded-Postgres provisioning.

**Why:** the packaged macOS `.app` / headless tarball / `install.sh` service run under a minimal launchd/Finder PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) and stage only a fixed `RESOURCE_NAMES` set — NOT the dev `target/{debug,release}/` tree with every binary side-by-side and a rich shell PATH. A dependency that resolves in dev can be absent or unreachable when packaged, and the failure typically surfaces as a cryptic mid-stream tool error or an indefinite boot-splash hang instead of a clear message. (The triggering incident: the `lucidos` CLI — needed for CC's permission MCP server — was not staged, so the first tool call died with `MCP tool … not found`. Worked catalog of this whole class: `data/artifacts/audits/packaged-app-bundle-and-failfast-audit.md`.)

For each runtime dependency the diff adds or changes, confirm BOTH:

- **Pattern A: staged + resolved, never PATH-dependent.** A binary, asset or dir the runtime requires must be staged by EVERY vehicle, from `resource_contract.sh`'s `resource_contract_names` through `stage_runtime_assemble`, `desktop.rs::bundled_resources` and the service env. It must also be resolved by absolute path or a guaranteed-set env var, never by a bare command name on PATH. A genuinely external user-install (`git`, `claude`, `codex`, `node`, `npx`, a non-bundled `psql`) is acceptable on two conditions. It resolves like `resolve_claude_binary`, absolute locations first, and its absence fails fast with an actionable message. Flag any bare `Command::new("name")` / `command: "name"` whose target is not on the launchd minimal PATH and not absolute-resolved, and any disk-read asset no vehicle stages.
- **Pattern B — fail fast, don't degrade.** A missing dep / `None` cli-dir / `Err` spawn / missing-file / unreachable-process must surface as an immediate, descriptive error at spawn or boot ("X not found at <path> — <feature> unavailable"), NOT a `log!`-and-proceed, a silent stub, or an unbounded wait. Flag any resolution that returns `None`/`Err` then proceeds anyway, any health check that reports healthy while a required surface is broken, and any "is the env var set?" check that never verifies the path actually exists.

If the diff edits the resource set or a staging/service/spawn-env path, run `./scripts/lib/resource_contract_test.sh` and both `--check` paths. The contract is one list, checked against the two launchers (ADR 0121). So a resource added to `resource_contract_names` alone is red until `service.sh` and `desktop.rs` resolve it too. Validate flagged items in Phase 3 and fix real ones in Phase 4 like any other finding.

## Phase 3: Validate Findings

### Join the Codex review (if launched in Phase 1)

If you launched a background Codex review in Phase 1, join it now: `.lucidos/codex-review.done` exists once it has ended. The last line of `.lucidos/codex-review.out` is the status line, and everything above it is the review. Read the status line first: it decides which case below applies.

- **`verdict returned`:** fold Codex's findings into the validation set below. Treat each like any other reviewer's finding: confirm it against source, fix a real 🔴 in Phase 4, discard a false positive. Log recurring dismissals to `docs/code-review-priors.md`. Codex frequently returns "no actionable bugs": record that outcome and move on.
- **Still running:** give it one bounded foreground wait on the marker, with `timeout: 600000`: `for i in $(seq 1 300); do [ -f .lucidos/codex-review.done ] && break; sleep 1; done`. The default timeout would kill the loop at 2 minutes. Cap the wait at ~5 minutes *since it was launched in Phase 1*. Usually it is already done, since Phases 1 to 2 ran in parallel with it. Remember the probe loop can hold the task for up to a minute before the review even starts, and a retry runs a second review.
- **`NO VERDICT`:** the reviewer started and failed twice. That is a lost review angle, never a pass and never "unavailable". Say so now, and quote the status line verbatim in the Phase 4 report, upstream cause included. It is still advisory, so proceed.
- **`unavailable`, or abandoned after the wait:** it is advisory. Note "Codex review: unavailable (advisory), proceeding" and continue. NEVER block the marker or stall the turn on Codex. (If a prior iteration's Codex task is still running when a new one launches, you may abandon the stale one.)

**A reviewer that returns nothing is configuration, not weather.** The companion
folds the real cause into one generic sentence. The status line quotes it back
from the reviewer's stderr, its JSON payload, or Codex's own log. This has
bitten once. The CLI moved its default model to one the local login could not
reach, and every review then 403'd. `codex login status` names the login, and a
reachable `model` in `~/.codex/config.toml` fixes it.

Then validate every finding (Codex's included) per the rest of this phase.

### Validate every finding

Once all three angles are done, validate each issue found. (Done means the parallel subagents are joined, or, for Codex and any agent without subagents, your own three inline passes are complete.) **Per the same subagents-are-optional rule:** Claude Code launches a parallel validation subagent per finding, all in ONE message. Codex and any agent without a Task tool validate each finding **inline and sequentially** in this same session. So do a small diff (Phase 0.6) and a merge-only or incremental run (Phase 0.4). Either way the validator must:
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
- **Either way, name what the Codex angle returned.** A `NO VERDICT` status line goes in verbatim, so the report never reads as four reviewers when it had three.

**A failure that also fails on `main` is main's bug, and another thread may be fixing it.** Run `git merge main --no-edit` first, since main moves every few minutes. Fix it here only if it still fails after that merge.

Commit any fixes from this phase before proceeding to Phase 4.5, naming the files. A fix outside `HARDEN_SAFE_PATHS` stops the early suite run first (`./scripts/harden-suites.sh stop`), per the Phase 1 kickoff rules. A new entry in `docs/code-review-priors.md` needs no stop.

## Phase 4.5: Verify Tests Pass

### Always first: the em-dash gate

```bash
./scripts/check-em-dashes.sh
```

Runs for **every** diff, with no fast path: the docs-only skip below does NOT apply to it, because prose is exactly where unspaced em dashes come from. It is diff-scoped and added-lines-only, so the ~29,000 pre-existing ones in the tree never fire it (see `.claude/rules/em-dashes.md`). A non-zero exit is a hardening failure like any other: fix the flagged lines, commit, and return to Phase 1.

This is also the layer that covers **Codex**, which has no `PreToolUse` hooks and therefore never met the write-time gate.

### Also always: the prose gate

```bash
./scripts/check-prose.sh
```

Runs for **every** diff, with no fast path, for the same reason as the em-dash gate above: prose is what it measures, so a docs-only skip would exempt the diff it exists for.

Four limits, all diff-scoped and added-lines-only (see `.claude/rules/prose.md`):

- a comment block of at most 20 lines
- a sentence of at most 25 words
- a paragraph of at most 6 sentences
- no ISO date inside a comment (a dated *path* is fine, and linking a plan is what the rule asks for)

The tree's 143,575 existing comment lines never fire it. A non-zero exit is a hardening failure like any other: fix the flagged lines, commit, and return to Phase 1.

Three further rules in that file are **not** machine-checked, because each needs part-of-speech tagging: a 20-word limit for an imperative step, active voice, and 3-word noun clusters. Those are a `code-review` angle, so Phase 1 covers them and this gate does not pretend to.

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

### Also always: no build script bakes a checkout path

```bash
./scripts/check-build-script-paths.sh
```

Whole-tree and unconditional, like the two gates above. Milliseconds, and a
merge is a way this regresses without any edit on the branch.

A cargo build script that reads `env!("CARGO_MANIFEST_DIR")` remembers the
checkout it was COMPILED in. Two checkouts of one package share a `-C metadata`
hash, so a shared `CARGO_TARGET_DIR` hands the artifact to whichever builds
next. The baked path then names another tree, or a deleted one (ADR 0079).

Deterministic rather than a review habit because two of the three failures are
**silent**: a frozen `GATEWAY_BUILD_ID`, and an app stamped `0000.00.00.0`. Only
the engine's panics, which is how this was found at all. Fix a failure by
reading the variable at run time, never by exempting the file. The gate is
scoped to a `build.rs` beside a `Cargo.toml`, so ordinary source keeping
compile-time `env!` (`crates/lucidos-engine/src/paths.rs`) is untouched.

### Also always: the e2e workflow keeps its limits

```bash
./scripts/check-e2e-workflow.sh
```

Whole-tree, unconditional, milliseconds. `.github/workflows/e2e.yml` runs a
stripped tree on the public mirror (ADR 0382), so its limits are security
limits: no secret, `contents: read` only, triggers only on `e2e/**` and `main`,
caches saved only on `main`, one-day artifacts, and `mobile-webkit` on macOS.
It also fails when another workflow starts triggering on `e2e/`. Fix a failure
by restoring the limit, never by loosening the check.

### Also always: system-knowhow points at things that exist

```bash
./scripts/check-knowhow-refs.sh
```

Whole-tree and unconditional, like the gates above, and milliseconds.

`system-knowhow/` ships to every install, and the engine LLM reads it as fact
rather than as a link it can shrug off. Four arms:

- a backtick-quoted repo path that does not exist
- a sibling knowhow file or id that does not resolve
- an event name in the audit or learning recipe that no engine enum has
- a severity word in `workspace-audit.md` outside its own legend

Unconditional because the usual author of this drift is the OTHER side of the
diff. A module becomes a directory, an event gets renamed, and a knowhow file
nobody edited starts lying. Two module renames sat stale in three files that
way, and `workspace-audit.md` spent months calling `ContextAssembled` a retired
event rename. It never was one.

Fix a failure by correcting the pointer or the name. Arm 3 keeps a short list
of PascalCase names in those recipes that are genuinely not engine events. A
recipe naming another real Rust type belongs on it. A name the recipe calls an
EVENT never does: widening the list to silence one is the single misuse.

The rule this backs is `.claude/rules/system-knowhow.md`, which also owns the
half no script can see: a check whose names all resolve but whose meaning has
gone stale.

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

### Then the test suites: join, and read the verdict

`scripts/harden-suites.sh` owns the suites: which ones run, their commands,
the early suite run, and whether its result still counts (ADR 0292). Join it
in the foreground with `timeout: 600000`:

```bash
./scripts/harden-suites.sh wait
```

`wait` joins the run, then runs the two Codex driver modules alone, then
prints one line per suite and a verdict. Act on its exit status:

| Exit | Verdict | Do this |
|---|---|---|
| 0 | PASS | Proceed to Phase 5. |
| 1 | FAIL | Read the named log, fix, commit, return to Phase 1. |
| 2 | RERUN | Format, start a normal run, and `wait` again (below). |
| 3 | still running | Re-issue `wait`. It says if the Codex review is what it waits on. |

**A start that refused shows up as MISSING.** `start` writes to
`.lucidos/harden-suites-start.log`, so read that log before starting again.

**RERUN means the early result no longer describes the branch.** A fix
outside `HARDEN_SAFE_PATHS` landed, or the run was stopped, or no run exists.
That is the normal outcome whenever review fixed code, and it costs no more
than the old order did. Run the kickoff's format block, then start a normal
run behind a shell `&`, as at the kickoff, and `wait` again:

```bash
./scripts/harden-suites.sh start > .lucidos/harden-suites-start.log 2>&1 &
```

**Codex-backend and docs-only runs start here.** They have no early run, so
they format, run `start` (Codex in the foreground), then `wait`.

**rustfmt runs before every start** (ADR 0270). `make lint` only checks it,
and `make test` is chained after it, so one rewrapped line otherwise skips the
whole engine suite. A `style: rustfmt` commit does not send you back to Phase
1: rustfmt changes no behaviour, so there is nothing to re-review.

**The Codex driver tests run alone, after the Codex review is joined.** Both
driver modules spawn a `/bin/sh` stub and share no state with a Codex review.
But their 30 s and 60 s timeouts fail under load. On 2026-08-10 a run that
overlapped the two took 539s against the usual 82s. Twelve driver tests
failed, and every one passed alone moments later.

So the early run skips both modules, and `wait` runs them by themselves. If
Phase 3 abandoned the Codex review, re-issue
`./scripts/harden-suites.sh wait --codex-abandoned`. Never treat a driver-test
failure as a finding until that module has run alone on a quiet host.

**Suite selection lives in code.** `hs_select_suites` maps the branch's
changed paths to suites, and `hs_suite_command` holds each suite's command.
Both are in `scripts/lib/harden_suites.sh`, and its test pins every row.
Change a row there, never in prose. Cargo suites run one after another in one
lane, and every other suite runs in parallel beside them. The paragraphs below
explain the rows a reader would not guess.

**The piped installer needs a row: `make lint` cannot see its one hard
constraint.** macOS `/bin/sh` IS bash 3.2, so `install.sh`'s re-exec guard
deliberately does not fire there. Everything a `curl … | sh` reaches must stay
inside the bash-3.2 posix subset. ShellCheck reads those files as bash, their
shebang rather than their runtime, so it passes the process substitution that
broke the one-liner. `install_test.sh` holds the line instead: it scans for
constructs bash 3.2 cannot run, then parses each file under bash-as-sh. Nothing
else in the local gate runs it.

**The release path needs a row for the same reason: nothing else runs its
suites.** `make lint` parses those scripts and says nothing about what they do.
The two suites are offline and take seconds, driving stubbed `gh` state that no
other gate models: the DMG-gate dispatch and its failure classification, and the
draft wait that decides whether Phase B adopts the rc build's tarballs or
rebuilds all four. Both halves are release-only code, so a regression there is
invisible until someone is mid-release.

**A file a crate compiles in selects the Rust suite, whatever its extension.**
The script lists every compile input at `start`: cargo's dep-info after any
build, plus every `include_str!` and `include_bytes!` literal in source. So an
edit that touches only `CHANGELOG.md`, `VERSION`, a menu JSON, `sdk_iframe.css`,
`LocaleSection.tsx` or `preferences.ts` still runs `make lint && make test`. A
compile input under `crates/lucidos-app/` also runs the app's own tests.

The guard behind such a file often lives on the other side of the diff. For
example, `voice/language.rs` `include_str!`s `LocaleSection.tsx` to check the
dropdown against its ISO-639-1 codes. A language added to the dropdown alone is
a frontend-only diff, and `tsc` and Vitest never compile that guard. Hand-kept
rows for such files missed `CHANGELOG.md` and the skill file this way, which is
why the rule reads the compiler's own list.

**Any file the CLI source includes is a CLI-row trigger, even an ENGINE path.**
`crates/lucidos-cli` owns no engine code, so nothing ran its tests: `make test`
is the engine crate alone, and `make lint` compiles the CLI tests without
running them. Yet CLI tests `include_str!` engine files to pin the CLI against
them: the two menu JSONs for `--reasoning-effort`, and `api/data_api.rs` plus
the frontend's `dataPathPrefixes.ts` for the data prefixes. An edit to one of those
is a diff outside `crates/lucidos-cli/`. So `start` records the CLI's own
includes in `cli-inputs`, and a listed path selects the CLI row. A hand-kept menu-JSON row once let the guard miss the change it
watches.

**A `system-knowhow/**` edit is not a docs-only skip.** Its frontmatter `name`
and `description` are spliced into the chat agent's routing list, which is
billed on every request of every thread. Two Rust tests own that cost:
`system_knowhow_descriptions_stay_routing_sized` caps each description, and
`always_loaded_context_stays_under_budget` caps the total against the
`ALWAYS_LOADED_BUDGET_CHARS` ratchet. Neither runs when the suites are skipped.

Until 2026-08-17 this table skipped them, so a knowhow file added or reworded by
a docs-only diff paid nothing at review time. The ceiling drifted 794 chars over
that way, and the next branch to touch a `.rs` file inherited a red suite it did
not cause. The filtered run costs one compile of an already-warm crate, against a
budget breach nobody sees until it lands on somebody else.

**CSS is not a skip.** Until 2026-08-05 this table said "CSS-only → skip", and
nothing else in the gate parses CSS: `tsc` ignores it, Vitest never built it.
So a syntax error passed every phase and landed on `main`, where it kills the
checkout-shared build-watch's `vite build`. The watch keeps serving the previous
`dist/` and republishes nothing, so the next frontend-only Apply times out in
`engine::frontend_refresh` and the user gets "Frontend change applied but not
served yet", naming the build-watch instead of the CSS file that broke it, for a
change that may not touch CSS at all. `npx vite build` is sub-second and is the
exact command the build-watch runs, so it fails on precisely what the watch will
fail on. The script runs it beside the other suites.

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

**`/harden` finishes in one turn.** Apply sends "Run /harden now", waits for the next idle, and then refuses a branch with no marker. So do NOT hand the suites to `lucidos background-task run` and end your turn here, as the general rule suggests for long work: that idle would read as a finished `/harden`. Claude Code has no blocking wait tool. So `start` runs behind a shell `&`, and each suite writes an exit file under `.lucidos/harden-suites/` when it ends. `wait` joins on those files in the foreground.

Each exit file holds the suite's real exit code: redirecting is not piping. `wait` and `verdict` print a few lines, so the logs never flood your context. Read the detail from the log a FAIL line names, with `tail -40` or `grep -nE "^error|test result:"`.

**A jsdom test that times out at exactly the vitest default is contention, not
a finding.** The engine suite saturates every core for minutes, and a Vitest
case awaiting async work starves under it. The tell is a round 5000ms against
a file the diff never touched. Re-run those files alone before believing them,
exactly as the Codex note above says for `runtime::codex::driver_tests`. Two
settings and trigger cases failed that way on 2026-09-15 and passed instantly
on their own.

**Never pipe the test command through `| tail` / `| head` / `| grep` to trim output.** Under zsh / bash a pipeline reports the *last* command's exit code, not cargo's. So `cargo test ... | tail` exits 0 even when a Rust test failed, and Phase 4.5 reports a false PASSED on a red run. This has shipped a failing nightly.

Run each suite un-piped: the exit-file pattern above already preserves the real exit. If you must trim, redirect to a log and capture `$?` first (`make test > /tmp/t.log 2>&1; echo "EXIT: $?"`), then read the log. A "tests pass" claim needs the real exit code AND the `test result: ok.` / `0 failed` line. See `/clean-build`'s "Reading exit codes honestly" section for the full mechanism.

If the verdict is PASS, proceed to Phase 5.

If it is FAIL: fix the failures (or the code that caused them), commit the fixes, and **return to Phase 1**. Fixes are new code that hasn't been reviewed by `/code-review` or the hardening agents, so re-run the cycle on the updated diff. Iterate until tests pass on a fully-hardened diff.

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

The marker also stops every background task this thread still has running
(`lucidos background-task run`), since this hardening supersedes it. Their
completions will not re-open the thread (ADR 0369). Then it names every wait
still live on the thread. Read what it printed:

- **Stopped background tasks**: name them in your summary as superseded.
- **Still waiting on**: the thread will re-open when that arrives, and Apply
  stays withheld until it does. Either say so plainly, or stand the wait down
  with `lucidos event-waits cancel` if you no longer need it. Never call the
  session finished while it holds one.

Then inform the user: "Hardening complete. Session can finish."
