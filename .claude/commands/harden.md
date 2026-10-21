Harden the code: review changed code for reuse, quality, and efficiency, then fix any issues found. Check for bugs and CLAUDE.md compliance, then run the test suites for what was touched (iterating from Phase 1 if anything fails). Run this before finishing your session.

**No postpone option.** Never tell the user you're "postponing", "deferring", or "skipping" `/harden`. There is no such mode, so saying it misleads them. Apply runs hardening synchronously when the marker is `MISSING`, and the user waits at that point. Either Phase 0 reports `ALREADY_HARDENED` or `PLAN_ONLY` (say so and stop) or you run the full skill. The only honest answers are "already hardened, skipping", "plans only, no hardening needed" or "running hardening now".

## Phase 0: Check if Already Hardened

`lucidos hardened query` prints `FRESH`, `STALE`, `MISSING` or `NOT_NEEDED`
for the branch in `$PWD`. `FRESH` means HEAD still matches the SHA recorded by
the last `/harden`; `STALE` means CC has committed since then and a re-run is
needed. `NOT_NEEDED` means the branch lands implementation plans only: the
engine decides that with the same predicate Apply reads, and Apply never
hardens such a branch.

```bash
case "$(lucidos hardened query 2>/dev/null)" in
  FRESH) echo "ALREADY_HARDENED" ;;
  NOT_NEEDED) echo "PLAN_ONLY" ;;
  *) echo "NOT_HARDENED" ;;
esac
```

If the output is `ALREADY_HARDENED`, inform the user: "Already hardened, skipping." and stop. Do NOT re-run hardening.

If the output is `PLAN_ONLY`, inform the user: "This branch carries only implementation plans, which need no hardening." and stop. Do NOT mark it hardened: the hardening runs on the branch that implements the plan.

On `NOT_HARDENED`, clear the review head an earlier run left, since only this
run's iterations may read it (§ Later iterations review only the fix):

```bash
rm -f .lucidos/harden-review-head
```

Then run the handoff checkpoint. It is Lucidos's copy of the default hardening
flow (`system-knowhow/coding-agent-flows/hardening`), and
`./scripts/check-default-flows.sh` keeps the two identical:

<!-- default-flow: hardening -->
#### Handoff checkpoint

At this flow's first entry, before any review, run
`lucidos successor offer --at pre-harden` and follow what it prints. Only the
first entry runs it: a later pass returns to the review, never here. A
successor this checkpoint started skips it too, and starts at the review.
<!-- /default-flow: hardening -->

It is automatic. It asks the owner at most once, and only when a handoff pays
but needs their approval. On "carry on", or when the command fails, continue
below. On a handoff, end your turn: the successor runs `/harden` from the start, but
skips this checkpoint.
On an ask, follow what it prints: Allow once hands off, Don't allow carries on.
A later iteration returns to Phase 1, so a handoff never lands in the middle of
the loop.

Then read the tier this branch runs (ADR 0465):

```bash
lucidos hardened expected-tier 2>/dev/null || echo full
```

`fast` means an *integrated sub-thread*: its parent merges its branch and runs
every suite once over the merged work. The fast tier changes four phases:
Phase 0.3 is skipped, Phase 4 merges no `main`, Phase 4.5's script selects the
fast suites itself, and Phase 5 marks `--tier fast`. Review runs as usual.
`full` is every other branch, and the rest of this file as written.

## Phase 0.3: Catch Up With main

Merge `main` into the branch before you review or test anything. Then this
one run certifies the work and the merge together. Otherwise a conflict at
Apply spawns a resolver that hardens everything a second time (ADR 0345).

Run it once per `/harden`, and only after Phase 0 printed `NOT_HARDENED`. A
merge into a `FRESH` branch would make its marker stale.

**Skip it on the fast tier.** An integrated sub-thread never merges `main`:
its parent merges it, once, over all its children's work.

1. If `git rev-parse -q --verify MERGE_HEAD` succeeds, a merge is already in
   progress. Go to step 4 and finish it, since any commit now concludes it.
2. Commit your own uncommitted work, as its own commit. It must not ride
   inside the merge commit.
3. Register the merge drivers, then merge:

   ```bash
   ./scripts/merge-drivers.sh
   git merge main --no-edit
   ```

   The registration is idempotent. If it refuses (an old `lucidos` CLI), the
   merge still runs, as plain text. A clean merge commits itself, and
   "Already up to date" needs nothing. Either way, go on to Phase 0.4.
4. On a conflict, resolve every file. Read both sides and keep what each one
   meant, then `git add` the file. Ask the user when a conflict is ambiguous.
   A conflict left in a registry `.gitattributes` names (ADR 0460) is a real
   one: both sides edited the same entry.
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
HARDEN_REVIEWED_HEADS="$(lucidos hardened integrated-heads 2>/dev/null)" \
  ./scripts/harden-scope.sh "$(lucidos hardened sha 2>/dev/null)"
```

`HARDEN_REVIEWED_HEADS` names the heads this branch's integrated children
hardened. Every commit under one counts as reviewed, as a commit on `main`
does. So a merge of such a child adds only its resolution to the review.

The first line is the answer:

- `MERGE_ONLY`: every commit since the hardened SHA that is not on `main` is
  a merge of `main`.
- `INCREMENTAL`: the branch also has commits of its own since then, such as a
  fix made during a merge session.
- `FULL <reason>`: go on to Phase 0.5 as usual.

The lines after it name what to review: `commit <sha>` (own commits, oldest
first), `merge <sha>`, `overlap <path>`, `codex-base <sha>`, and
`fork-point <sha>`. The review covers these inputs, and nothing else:

- **The new lines:** one file holding each commit's patch and each merge's
  resolution. `git show --remerge-diff` shows what the resolver wrote against
  git's own conflicted result, including edits to files that merged clean.

  ```bash
  mkdir -p .lucidos && {
    for c in <each commit sha>; do git show --format='commit %H %s' "$c"; done
    for m in <each merge sha>; do git show --remerge-diff --format= "$m"; done
  } > .lucidos/review-target.diff
  ```
- **The overlap:** each `overlap` path is a file both sides of a merge
  changed. Read the branch's change there against the other side's. A
  semantic conflict can merge clean.

The phases then run as follows:

- Phases 0.5 and 0.6 are skipped.
- Phase 1 kickoff starts the early suite run unchanged. With a `codex-base`
  line, the Codex review runs against that base. `INCREMENTAL` without one
  runs it against `main`, which after Phase 0.3 is the branch's own change. A
  hardened SHA as the base would pull in all of main's new code. `MERGE_ONLY`
  skips it.
- Phase 1 runs `code-review` **inline**, with `.lucidos/review-target.diff`
  as its target, and also reads each overlap path. An empty file means a
  clean merge with nothing resolved: note "Phase 1: clean merge" and move on.
- Phase 2.5 gates on the review target.
- **Phases 4, 4.5 and 5 run unchanged.** Every suite `harden-suites.sh`
  selects for the branch still runs, because a merge that mixes sides needs
  the full suite (`CLAUDE.md`).

**A `fork-point` line means the branch has no marker of its own.** Its
children's runs reviewed what they cover, and the listed commits are the
branch's own since it forked. That may be a lot of new work, so Phases 0.5
and 0.6 run as usual, and Phase 1 runs the way they pick. Every review input
is still the review target above.

This check runs in the first iteration only. A later iteration has its own (§ Later
iterations review only the fix).

## Phase 0.5: Detect Docs-Only Diff

Run `git diff main...HEAD --name-only`. If every changed file ends in `.md` or `.txt`, the diff is **docs-only**. In docs-only mode:

- Phase 1 skips both kickoff steps and runs `code-review` **inline**, with
  only the compliance, prose, private-data and buried-finding angles. The
  correctness angles look for code-shaped bugs that prose does not have.
- Phase 3, Phase 4 and Phase 5 still run.
- Phase 2.5 auto-skips for docs-only via its own packaged-runtime gate.
- Phase 4.5 still runs `scripts/harden-suites.sh`, which picks the suites a docs-only diff needs. A `system-knowhow/**` edit runs the always-loaded budget tests, and a compiled-in file such as `CHANGELOG.md` runs the Rust suite.

Do NOT extend this fast path to "string-only" or "comment-only" `.rs` edits. Strings can carry format args, escape sequences, regexes, or be parsed at runtime — any `.rs` change keeps the full cycle.

## Phase 0.6: Detect a Small Diff

A small diff keeps every angle but runs the Phase 1 review inline, even on
Claude Code. A subagent re-reading a one-token diff costs more than the review
itself. The check ignores `.md` and `.txt` files, so a plan or glossary edit
alongside a small fix does not push it over.

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

- Phase 1 runs `code-review` **inline**, Codex review included. It is the
  phase that finds real bugs in small CSS diffs, such as a forced-colors
  regression.
- Phase 2.5, Phase 3, Phase 4, Phase 4.5 and Phase 5 run unchanged.

This check runs in the first iteration only. A later iteration sizes its fix itself
(§ Later iterations review only the fix).

A `.rs`, `.sql`, `.sh`, `Cargo.toml`, `Cargo.lock` or `Makefile` change is never
small, whatever its size. The reason is the one Phase 0.5 gives for `.rs`
strings, and it also keeps every Phase 2.5 surface out of this mode.

## Every iteration records its verdict

An iteration runs from Phase 1 to its verdict. Count them from 1. Record each
one just before you act on its verdict: before any "return to Phase 1", and
before Phase 5.

```bash
lucidos events emit HardenRunCompleted --summary "harden iteration <n>: <pass|fail>" \
  --payload "{\"thread_id\": \"$LUCIDOS_THREAD_ID\", \"branch\": \"$(git branch --show-current)\", \"head_sha\": \"$(git rev-parse HEAD)\", \"iteration\": <n>, \"verdict\": \"<pass|fail>\", \"failed_phase\": \"<phase, or empty on pass>\", \"tier\": \"<full|fast>\", \"findings\": {\"code_review\": <n>, \"codex\": <n>, \"fixed\": <n>}, \"suites\": $(./scripts/harden-suites.sh timings)}"
```

- `failed_phase` names the gate that failed, such as `4.5 prose` or
  `4.5 engine tests`.
- `suites` is each suite's start, end and exit code in epoch seconds. It also
  says whether the round was a full run or a retry of what failed. The next
  round of tuning reads per-suite time from it.
- `findings` counts this iteration's findings by source. `code_review` is what
  the review returned, and `codex` what Codex returned (0 when it did not
  run). `fixed` is how many of them led to a fix. A re-measure compares yield
  per source against ADR 0476.
- A domain event carries no thread of its own, so `thread_id` is what ties the
  run to its thread. The auto-compact trial counts these
  (`docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`).

An emit that fails never blocks hardening: note it and go on.

## Later iterations review only the fix

Every "return to Phase 1" in this file starts a later iteration. The iteration before
reviewed every line up to the review head, so a later iteration reviews only what
came after it (ADR 0476). Phases 0 to 0.6 do not run again.

1. Find the new lines. Phase 4 writes the review head at the end of every
   iteration:

   ```bash
   ./scripts/harden-scope.sh "$(cat .lucidos/harden-review-head 2>/dev/null)"
   ```

   A `FULL` answer means the head is missing or unusable: run the iteration as
   the first iteration does. Otherwise build `.lucidos/review-target.diff` from
   the `commit` and `merge` lines exactly as Phase 0.4 does, and read each
   `overlap` path.
2. Size the target:

   ```bash
   grep -E '^[+-]' .lucidos/review-target.diff | grep -cvE '^(\+\+\+|---)( |$)'
   ```

3. **60 lines or fewer:** run Phase 1 with no kickoff Codex review, and the
   review **inline** on the target. **Over 60:** run Phase 1 as the first iteration
   does, on the target. The Codex review takes `CODEX_BASE` from a
   `codex-base` line, else `main`.
4. Phases 2.5 to 5 then run as usual.

## Phase 1: Run /code-review

**Docs-only:** if Phase 0.5 flagged this diff as docs-only, skip both kickoff
steps below and run the review inline, with the angles Phase 0.5 names.

### Phase 1 kickoff: start the early suite run (Claude Code only)

The Phase 4.5 test suites start now and run alongside the review phases
(ADR 0292). The cheap gates run first, in the foreground with
`timeout: 240000`. Each takes seconds, and a failure they catch should not
cost a suite run:

```bash
./scripts/harden-gates.sh
```

On a FAIL, read the log it names, fix, commit, and run it again before the
suites start. Then format, so the start commit is the one the suites test:

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
  in `scripts/lib/harden_suites.sh` names every markdown file under `docs/`
  except the glossary, which tests read, and all of the plans and ADR folders.
  A fix to those docs alone keeps the suites running. Run
  `./scripts/harden-suites.sh stop` before editing anything else. Tests read
  the tree while they run, so an edit mid-run tests a mix of old and new code.
  Each suite records what changed when it exits, so the verdict voids a run
  whose edit is still there. An edit made and undone mid-run leaves no trace,
  which is why the stop is the rule.
- **Commit with explicit paths, never `git commit -a`.** An engine test
  briefly rewrites the two `VERSION` files and restores them. An `-a` commit
  during the run can capture that temporary content.

### Phase 1 kickoff: launch Codex review in parallel (advisory, Claude Code only)

Before running the `code-review` skill, kick off a Codex review of the branch
diff **in the background**. It overlaps Phases 1 to 3 and adds almost no wall
clock.

It is the second reviewer beside `code-review`, and the only one from another
model family. A later iteration launches it only for a fix over 60 lines (§ Later
iterations review only the fix).

This step is **advisory**: its findings feed the same validate→fix pipeline as every other reviewer (joined in Phase 3), but Codex being unavailable, slow, erroring, or timing out NEVER blocks the hardened marker.

- **Claude Code only.** A Codex-backed `/harden` run is already Codex reviewing this diff — skip this step and note "Codex review: skipped (Codex-backend run)".
- **Docs-only:** skipped, since Codex is a code reviewer, not a prose reviewer.
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

Run the **repo-owned** `code-review` skill (`.claude/skills/code-review/SKILL.md`)
at **high** effort. It is the one Claude review of the iteration, beside Codex
(ADR 0476). Its target is `git diff main...HEAD`, or
`.lucidos/review-target.diff` when Phase 0.4 or a later iteration built one.

- **Claude Code, first iteration of a full diff:** run it in ONE `Agent` subagent,
  so its file reads stay out of your long context. Tell it to read the skill
  file and follow it at high effort against the target. Also tell it that it
  runs as a subagent for `/harden` Phase 1, so it reports as the skill's Output
  section says. Under Lucidos the call blocks and returns the report inline
  (ADR 0358). Do not pass `run_in_background`: the parameter does not exist.
- **Inline cases** (small diff, docs-only, merge-only, incremental, or a small
  later-iteration fix): invoke it in your own session, `Skill skill: "code-review"
  args: "high"`. The project skill overrides Claude Code's built-in of the same
  name.
- **Codex / any agent without a Skill or Agent tool:** the skill is NOT in your
  available-skills list. **Read `.claude/skills/code-review/SKILL.md` and
  follow its phases directly**, inline, at high effort. Do not skip Phase 1
  because the Skill tool can't find it. Continue to Phase 3 in the same turn,
  and do not stop until Phase 5 has written the marker.

When `code-review` returns its findings:

- **No findings:** record that and go on to Phase 2.5.
- **Findings:** the skill's own verifier already confirmed each one, so do not
  validate them again. Act on each by its `category`:
  - `correctness`: fix it. If reading the code to fix it shows the finding is
    wrong, drop it and say why.
  - `compliance`: fix it. A rule requires each one, such as a registry row, a
    glossary entry or a placeholder for private data.
  - `cleanup`: list it in the Phase 4 report. Do not fix it and do not ask
    about it. A fix can void the early suite run and start another iteration.
- Run `./scripts/harden-suites.sh stop` before the first edit, per the kickoff
  rules above.
- **Commit any fixes** before Phase 2.5. Name the files: `git add <file>`,
  never `git commit -a`.

Do NOT pass `--comment` (that mode posts to GitHub PRs, which Lucidos does not use).

**Report Phase 1 in prose. The findings are structured data, not a message.**
The skill hands them to *you* through its Output section: a subagent's final
report, the `ReportFindings` tool, or an in-band list with an explicit
`No findings.` in the empty case. Translate the result into one sentence of
prose, such as "Phase 1: no findings" or "Phase 1 flagged N issues: …".

**Never paste a findings array into your reply**: not empty `[]`/`{}`, not populated, not fenced, not inline. The fix
sits at the source (the repo-owned skill), not in a frontend filter. See
`docs/temporary-measures.md` § "code-review findings array leaking into chat".

## Phase 2.5: Packaged-Runtime Dependency & Fail-Fast Check

**Gate:** run this phase only when `git diff main...HEAD` touches a packaged-runtime surface; otherwise note "Phase 2.5: not applicable" and skip. The trigger surfaces are any diff that:

- adds or changes a subprocess spawn (`Command::new(...)`, `tokio::process::Command::new(...)`), an MCP server `command:` entry, a `--permission-prompt-tool` / hook `command`, or any other place that shells out by name;
- reads a file/asset from disk at runtime, or flips an asset between `include_str!`/`include_bytes!` (baked) and disk-read (staged);
- adds or reads a `LUCIDOS_*_DIR` / `*_BIN` env var, or a `current_exe()`-relative path walk;
- edits the packaging / delivery / install contract: `scripts/lib/resource_contract.sh` (`resource_contract_names`, the one `RESOURCE_NAMES` source), `scripts/lib/stage_runtime.sh` (`stage_runtime_assemble`), `scripts/build-dmg.sh`, `scripts/build-headless.sh`, `scripts/lib/service.sh`, `install.sh`, `crates/lucidos-app/src/desktop.rs` (`spawn_gateway` env, the `*_RESOURCE_NAME` constants), or the gateway's engine / embedded-Postgres provisioning.

**Why:** the packaged `.app`, tarball and `install.sh` service run under a minimal launchd PATH and stage only the fixed `RESOURCE_NAMES` set. A dependency that resolves in dev can be absent when packaged, and the failure shows as a cryptic tool error or a boot hang. The worked catalog is `data/artifacts/audits/packaged-app-bundle-and-failfast-audit.md`.

For each runtime dependency the diff adds or changes, confirm BOTH:

- **Pattern A: staged + resolved, never PATH-dependent.** A binary, asset or dir the runtime requires must be staged by EVERY vehicle, from `resource_contract.sh`'s `resource_contract_names` through `stage_runtime_assemble`, `desktop.rs::bundled_resources` and the service env. It must also be resolved by absolute path or a guaranteed-set env var, never by a bare command name on PATH. A genuinely external user-install (`git`, `claude`, `codex`, `node`, `npx`, a non-bundled `psql`) is acceptable on two conditions. It resolves like `resolve_claude_binary`, absolute locations first, and its absence fails fast with an actionable message. Flag any bare `Command::new("name")` / `command: "name"` whose target is not on the launchd minimal PATH and not absolute-resolved, and any disk-read asset no vehicle stages.
- **Pattern B — fail fast, don't degrade.** A missing dep / `None` cli-dir / `Err` spawn / missing-file / unreachable-process must surface as an immediate, descriptive error at spawn or boot ("X not found at <path> — <feature> unavailable"), NOT a `log!`-and-proceed, a silent stub, or an unbounded wait. Flag any resolution that returns `None`/`Err` then proceeds anyway, any health check that reports healthy while a required surface is broken, and any "is the env var set?" check that never verifies the path actually exists.

If the diff edits the resource set or a staging/service/spawn-env path, run `./scripts/lib/resource_contract_test.sh` and both `--check` paths. The contract is one list, checked against the two launchers (ADR 0121). So a resource added to `resource_contract_names` alone is red until `service.sh` and `desktop.rs` resolve it too. Validate flagged items in Phase 3 and fix real ones in Phase 4 like any other finding.

## Phase 3: Validate Findings

### Join the Codex review (if launched in Phase 1)

If you launched a background Codex review in Phase 1, join it now: `.lucidos/codex-review.done` exists once it has ended. The last line of `.lucidos/codex-review.out` is the status line, and everything above it is the review. Read the status line first: it decides which case below applies.

- **`verdict returned`:** fold Codex's findings into the validation set below. Treat each like any other reviewer's finding: confirm it against source, fix a real 🔴 in Phase 4, discard a false positive. Log recurring dismissals to `docs/code-review-priors.md`. Codex frequently returns "no actionable bugs": record that outcome and move on.
- **Still running:** give it one bounded foreground wait on the marker, with `timeout: 240000`: `for i in $(seq 1 230); do [ -f .lucidos/codex-review.done ] && break; sleep 1; done`. The default timeout would kill the loop at 2 minutes, and a longer loop outlives the prompt cache. Cap the wait at ~5 minutes *since it was launched in Phase 1*. Usually it is already done, since Phase 1 ran in parallel with it. Remember the probe loop can hold the task for up to a minute before the review even starts, and a retry runs a second review.
- **`NO VERDICT`:** the reviewer started and failed twice. That is a lost review angle, never a pass and never "unavailable". Say so now, and quote the status line verbatim in the Phase 4 report, upstream cause included. It is still advisory, so proceed.
- **`unavailable`, or abandoned after the wait:** it is advisory. Note "Codex review: unavailable (advisory), proceeding" and continue. NEVER block the marker or stall the turn on Codex. (If a prior iteration's Codex task is still running when a new one launches, you may abandon the stale one.)

**A reviewer that returns nothing is configuration, not weather.** The status line quotes the cause (`docs/harden-notes.md` § A silent Codex review is configuration).

Then validate the findings that no verifier has checked, per the rest of this
phase.

### Validate Codex and Phase 2.5 findings

Validate each Codex finding and each Phase 2.5 finding **inline**, in your own
session. `code-review` findings skip this step, since the skill's verifier
already checked them (ADR 0476). For each one:

- Read the relevant source files, not just the diff.
- Confirm the issue actually exists in the code.
- Discard findings that are false positives or depend on assumptions about
  runtime state.

Only confirmed findings proceed to the report.

When validation dismisses a finding about a **pattern likely to be re-flagged
by future reviews** (a guarded construct that looks unguarded, a documented
deliberate behavior that looks like a bug), add a pattern-based entry to
`docs/code-review-priors.md` in the same change — that ledger is what keeps
the next review round from re-litigating it. One-off misreadings of the diff
don't need an entry; recurring-shaped ones do.

## Phase 4: Report and Fix

- If **no validated issues**: report "No bugs or compliance issues found."
- If **validated issues found**: list each issue grouped by severity (🔴 Bug,
  🟡 Nit) with file, line, and description. Fix 🔴 bugs and rule violations
  directly. List 🟡 nits and Phase 1 `cleanup` findings without fixing them,
  and do not ask the user about them. A fix can void the early suite run and
  start another iteration (ADR 0476).
- **Either way, name what the Codex angle returned.** A `NO VERDICT` status
  line goes in verbatim, so the report never reads as two reviewers when it
  had one.

**A failure that also fails on `main` is main's bug, and another thread may be fixing it.** Run `lucidos main-check status` first. When main is red with a failure that matches, its fix thread owns it (ADR 0464). Do not fix it in this branch: note it in the report and carry on. Otherwise run `git merge main --no-edit`, since main moves every few minutes. Fix it here only if it still fails after that merge.

On the fast tier, merge no `main`. Note the failure for the parent, whose run merges it.

Commit any fixes from this phase before proceeding to Phase 4.5, naming the files. A fix outside `HARDEN_SAFE_PATHS` stops the early suite run first (`./scripts/harden-suites.sh stop`), per the Phase 1 kickoff rules. A fix to a doc on that list, such as a new entry in `docs/code-review-priors.md`, needs no stop.

Then record the review head. Every line up to it has had this iteration's
review, so a later iteration starts from it (§ Later iterations review only the fix):

```bash
mkdir -p .lucidos && git rev-parse HEAD > .lucidos/harden-review-head
```

## Phase 4.5: Verify Tests Pass

### Always first: the cheap gates

```bash
./scripts/harden-gates.sh
```

One command runs every gate below, side by side, and prints a PASS or FAIL
line per gate. It runs for **every** diff, with no fast path. The kickoff ran
it before the suites started; it runs again here because review fixes may
have added lines. On a FAIL, read the gate's section below, fix, commit, and
return to Phase 1. Each section's command runs its gate alone, to check a fix.

### The em-dash gate

```bash
./scripts/check-em-dashes.sh
```

Runs for **every** diff, with no docs-only skip, and is diff-scoped to added lines
(`.claude/rules/em-dashes.md`). A non-zero exit is a hardening failure: fix the
flagged lines, commit, and return to Phase 1. It also covers Codex, which has no
write-time hooks.

### Also always: the prose gate

```bash
./scripts/check-prose.sh
```

Runs for **every** diff, with no docs-only skip, over added lines only. The four
limits (20-line comment block, 25-word sentence, 6-sentence paragraph, no ISO date
in a comment) are in `.claude/rules/prose.md`. A non-zero exit is a hardening
failure: fix, commit, return to Phase 1. The unchecked rules (20-word imperative
step, active voice, 3-word noun clusters) are a `code-review` angle in Phase 1.

### Also always: the whole-tree gates

Each one runs for **every** diff, docs-only included. Each is a few milliseconds,
and a merge can break one with no edit on the branch (`docs/harden-notes.md`
§ Why a gate is unconditional). Fix a failure at its source, never by loosening
the check. Each script header holds the mechanism.

| Command | Fails on | Fix |
|---|---|---|
| `./scripts/check-adrs.sh` | A duplicate ADR number, an ADR with no index line or the reverse, an index out of order, a missing required section (ADR 0460). | `--fix` repairs order only. Never auto-renumber a duplicate. Create ADRs with `./scripts/adr-new.sh`. |
| `./scripts/check-context-budget.sh` | The always-loaded set over `CONTEXT_BUDGET_CEILING`, a resident set that differs from the declared list, a path-scoped rule over its ceiling (`--report` prints the set). | Move content: reference to a skill, a convention to a scoped rule, maintainer prose to `docs/agent-config.md`. Raising a ceiling needs a reason in the commit message. |
| `./scripts/check-prompt-mirror.sh` | The process-safety prohibition (ADR 0025) missing from `CLAUDE.md` or the engine system prompt. | Restore the missing half, never delete the other. A second mirror needs the proof in `scripts/lib/prompt_mirror_scan.sh`. |
| `./scripts/check-build-script-paths.sh` | A `build.rs` that reads `env!("CARGO_MANIFEST_DIR")` and bakes a checkout path (ADR 0079). | Read the variable at run time, never exempt the file. |
| `./scripts/check-e2e-workflow.sh` | `e2e.yml` losing a security limit (ADR 0382): no secret, `contents: read`, triggers only on `e2e/**` and `main`, cache saved on `main` only, one-day artifacts. | Restore the limit. |
| `./scripts/mirror-push-gate.sh` then `./scripts/check-mirror-pushers.sh` | A tracked script, workflow or Rust source that pushes outside the allowlist or names the mirror's git URL (ADR 0462). The first command installs the refusing push URL. | Route the push through `release_tree_push`, never widen the allowlist. |
| `./scripts/check-knowhow-refs.sh` | A `system-knowhow/` path, sibling id, event name or severity word that does not resolve (`.claude/rules/system-knowhow.md`). | Correct the pointer. Never widen arm 3's non-event list to silence an event name. |
| `./scripts/check-default-flows.sh` | A copy of a default coding-agent flow that differs from its knowhow block (ADR 0505). | Copy the block back, or change the default and every copy together. Lucidos's own steps stay outside the markers. |
| `./scripts/check-audit-scans.sh` | A workspace-audit `scan` line that misses a hit row in its `FIXTURES` block. | Fix the pattern. A new scan needs a hit row. |
| `./scripts/lib/hooks_registered_test.sh` | A `.claude/settings.json` hook that is not valid, missing or not executable. | Fix the mode or path. |

### Then the test suites: join, and read the verdict

`scripts/harden-suites.sh` owns the suites: which ones run, their commands,
the early suite run, and whether its result still counts (ADR 0292). Join it
in the foreground with `timeout: 240000`:

```bash
./scripts/harden-suites.sh wait
```

`wait` joins the run. It then starts the two Codex driver modules alone in the
background and joins them too. Last, it prints one line per suite and a
verdict. It returns within 230 s, inside the 5-minute prompt cache, so exit 3
is routine: re-issue it. Act on its exit status:

| Exit | Verdict | Do this |
|---|---|---|
| 0 | PASS | Proceed to Phase 5. |
| 1 | FAIL | Read the named log, fix, commit, return to Phase 1. |
| 2 | RERUN | Format, start a normal run, and `wait` again (below). After a retry round that passed, this run is the full set. |
| 3 | still running | Re-issue `wait`. It says if the Codex review or the driver tests are what it waits on. |

**Join inline, never in a subagent.** Run `wait` yourself and follow the table.
Add `--codex-abandoned` when Phase 3 abandoned the Codex review. Each `wait`
ends inside the prompt cache, so polling costs one cache read per call. A
subagent join blocks you for the whole run, usually past the cache. You then
write your whole context again when it returns (ADR 0517).

Run tests only through `harden-suites.sh`. Never call `cargo test`, `make test`
or `./scripts/test-engine.sh` directly: the script's one cargo lane keeps two
cargo runs from overlapping. On FAIL, fix the root cause, never weaken or skip
a test.

**A FAIL makes the next round a retry of what failed** (ADR 0536). The verdict records the failed
engine tests and browser specs by name in `.lucidos/harden-retry`, and any
other failed suite whole. The next `start`, early or normal, runs only those
and says `a retry of what failed`. Its PASS still reads RERUN: the full set
then runs once more, and only that run can pass the harden. Never delete the
record to skip the retry.

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

**A VOID that says the safe-path allowlist is off names the files that turned
it off.** Read each one. A file that never reads the path goes on
`HS_READ_CHECK_KNOWN` in `scripts/lib/harden_suites.sh`, at the count the
check printed, in this change. Otherwise every later early run voids on it too.

**Codex-backend and docs-only runs start here.** They have no early run, so
they format, run `start` (Codex in the foreground), then `wait`.

**rustfmt runs before every start** (ADR 0270). `make lint` only checks it, and
`make test` is chained after it, so one rewrapped line otherwise skips the whole
engine suite. A `style: rustfmt` commit does not send you back to Phase 1,
because rustfmt changes no behaviour.

**The Codex driver tests run alone, after the Codex review is joined.** The early
run skips both driver modules, and `wait` starts them by themselves. If Phase 3
abandoned the Codex review, re-issue
`./scripts/harden-suites.sh wait --codex-abandoned`. Never treat a driver-test
failure as a finding until that module has run alone on a quiet host
(`docs/harden-notes.md` § Timeouts under load).

**The fast tier is the script's call, not yours.** `start` asks the engine for the
tier and records it, and no variable overrides the answer. It keeps `make lint`
in place of the engine suite, plus tsc, Vitest, `vite build` and the script tests.
It drops every crate's test suite, the installer, the release path, the
first-paint check and the touched browser specs. The integrating parent runs
them all.

**Suite selection lives in code.** `hs_select_suites` maps the branch's changed
paths to suites, and `hs_suite_command` holds each command, both in
`scripts/lib/harden_suites.sh`. Its test pins every row, so change a row there,
never in prose. Cargo suites run one after another in one lane. Every other suite
runs in parallel beside them. `docs/harden-notes.md` § Suite rows explains the
rows a reader would not guess.

These matter when you read a verdict:

- **A file a crate compiles in selects the Rust suite, whatever its extension.**
  `CHANGELOG.md` and `sdk_iframe.css` both do.
- **A `system-knowhow/**` edit is never a docs-only skip.**
- **CSS is never a skip.** The script runs `npx vite build` beside the other suites.
- **A first-paint input runs the growth check** (ADR 0454). On FAIL, move what the
  first frame skips behind a dynamic import. Otherwise add a
  `First-paint-growth:` trailer saying why the first frame needs it.
- **A frontend change runs the browser specs it can break** (ADR 0502).
  `e2e-touched` runs them here on the debug engine in an e2e slot, without
  mobile-webkit. `e2e-touched-webkit` sends the mobile-webkit ones to GitHub,
  pinned to the start commit (ADR 0536). On FAIL, read the log the FAIL line
  names. Either the spec follows the UI change, or the UI change was
  wrong.
- **A busy e2e lock is a `SKIP`, and a `SKIP` passes** (ADR 0541). Every e2e
  slot was held, so the specs did not run here. The main check runs them after
  landing. Never wait for the lock and never rerun on a timer. Name the skipped
  specs in your summary.
- **`make lint` is the per-change lint gate, never `cargo check`.** `/harden` is
  the only stage between a change and `main`. Never restate its flags.

**`/harden` finishes in one turn.** Apply sends "Run /harden now", waits for the next idle, and then refuses a branch with no marker. So do NOT hand the suites to `lucidos background-task run` and end your turn here, as the general rule suggests for long work: that idle would read as a finished `/harden`. Claude Code has no blocking wait tool. So `start` runs behind a shell `&`, and each suite writes an exit file under `.lucidos/harden-suites/` when it ends. `wait` joins on those files in the foreground.

Each exit file holds the suite's real exit code: redirecting is not piping. `wait` and `verdict` print a few lines, so the logs never flood your context. Read the detail from the log a FAIL line names, with `tail -40` or `grep -nE "^error|test result:"`.

**A jsdom test that times out at exactly 5000 ms is contention, not a finding.**
Re-run those files alone before believing them.

**Never pipe the test command through `| tail`, `| head` or `| grep`.** A pipeline
reports the last command's exit code, so a red cargo run reads as PASS
(`docs/harden-notes.md` § Never pipe a test command). Run each suite un-piped. To
trim output, redirect to a log, capture `$?` first, then read the log. A "tests
pass" claim needs the real exit code and the `test result: ok.` line.

Either way, record the iteration first (§ Every iteration records its verdict).

If the verdict is PASS, proceed to Phase 5.

If it is FAIL: fix the failures (or the code that caused them), commit the
fixes, and **return to Phase 1**. Fixes are new code no review has seen, so
the next iteration reviews them (§ Later iterations review only the fix). Iterate
until tests pass on a fully-hardened diff.

## Phase 5: Create Marker

After all phases complete (and any bug fixes are applied), record this branch's
HEAD as hardened in the parent workspace's DB:

```bash
lucidos hardened mark
```

On the fast tier, mark the tier you ran instead:

```bash
lucidos hardened mark --tier fast
```

The engine refuses a fast marker on any branch but an integrated sub-thread's,
since it would not count there. A fast marker hands the child's branch up to
its parent, and Lucidos proposes nothing for it.

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
