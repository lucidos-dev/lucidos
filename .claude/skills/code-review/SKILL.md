---
name: code-review
description: Review the current branch diff for correctness bugs plus reuse / simplification / efficiency / altitude cleanups, scaled by an effort arg (low/medium → precision, fewer high-confidence findings; high → recall-biased; xhigh/max → broader + a gap sweep). Reports findings through a structured channel (the ReportFindings tool, or an in-band array handoff for agents without it) — never pasted into chat. Repo-owned so it runs identically under Claude Code and Codex — Phase 1 of /harden drives it. Pass an effort word as the arg (default medium).
---

# code-review (Lucidos, repo-owned)

Diff reviewer used by `/harden` Phase 1. This is the **repo's own copy** of the
review procedure, deliberately vendored into `.claude/skills/` so every coding
agent can run it — not just Claude Code.

**Why this exists.** Claude Code ships a built-in `code-review` skill (the
renamed `/simplify`). Codex has no access to Claude Code's built-in or plugin
skill registry — it only sees skills on disk under `.claude/skills/` (and its
own `~/.codex/skills/`). When `/harden` ran under a Codex session it could not
find `code-review`, so it fell back to an ad-hoc manual review. Vendoring the
procedure here removes that gap: both backends run the same phases.

**How to run it (any backend):**
- **Claude Code** — invoke via the Skill tool: `Skill skill: "code-review" args: "medium"`. A project skill of this name overrides the built-in, so this resolves here.
- **Codex / any agent without a Skill tool** — read this file and follow the phases below directly against the branch diff.

**Subagents are optional.** The phases below say "run angle X". If your runtime
has an `Agent`/subagent tool, run the finder angles and verifiers as parallel
subagents (faster, independent perspectives). If it does not (e.g. Codex),
perform each angle inline as a focused sequential pass yourself — the procedure
and output contract are identical either way.

## Effort scaling

The arg picks the recipe. Default to **medium** when no arg is passed.

| effort | bias | finder angles | candidates/angle | verify | Phase 3 sweep | max findings |
|---|---|---|---|---|---|---|
| low / medium | **precision** — every finding is one a maintainer would act on | 3 correctness + 3 cleanup + 1 altitude (7) | 6 | 1-vote, 3-state | no | 8 |
| high | **recall-biased** | 3 correctness + 4 cleanup-class + altitude (7) | 6 | 1-vote (recall-biased) | no | 10 |
| xhigh / max | **recall** — a missed bug ships; err toward surfacing | 5 correctness + 3 cleanup + 1 altitude (9) | 8 | 1-vote | yes | 15 |

`/harden` calls this at **medium** (precision): fewer findings, very low
false-positive rate, complementary to harden Phase 2's broader bug-detection
agent.

## Phase 0 — Gather the diff

Run `git diff @{upstream}...HEAD` (or `git diff main...HEAD` / `git diff HEAD~1`
if there's no upstream) to get the unified diff under review. If there are
uncommitted changes, or the range diff is empty, also run `git diff HEAD` and
include the working-tree changes in scope — the review often runs before the
commit. If a PR number, branch name, or file path was passed as an argument,
review that target instead. Treat this diff as the review scope.

## Phase 1 — Find candidates

Run the finder angles for the chosen effort (see table). Each surfaces **up to
N candidate findings** (N = candidates/angle) with `file`, `line`, a one-line
`summary`, and a concrete `failure_scenario`. Run the angles independently — do
NOT let one angle's conclusions suppress another's; if two angles flag the same
line for different reasons, record both.

### Correctness angles

**Angle A — line-by-line diff scan.** Read every hunk in the diff, line by
line. Then read the enclosing function for each hunk — bugs in unchanged lines
of a touched function are in scope (the change re-exposes or fails to fix them).
For every line ask: what input, state, timing, or platform makes this line
wrong? Look for inverted/wrong conditions, off-by-one, null/undefined deref,
missing `await`, falsy-zero checks, wrong-variable copy-paste, error swallowed
in catch, unescaped regex metachars.

**Angle B — removed-behavior auditor.** For every line the diff DELETES or
replaces, name the invariant or behavior it enforced, then search the new code
for where that invariant is re-established. If you can't find it, that's a
candidate: a removed guard, a dropped error path, a narrowed validation, a
deleted test that was covering a real case.

**Angle C — cross-file tracer.** For each function the diff changes, find its
callers (Grep for the symbol) and check whether the change breaks any call
site: a new precondition, a changed return shape, a new exception, a
timing/ordering dependency. Also check callees: does a parallel change in the
same diff make a call unsafe?

**Angle D — language-pitfall specialist** *(xhigh/max only).* Scan for the
classic pitfalls of the diff's language/framework — JS falsy-zero, `==`
coercion, closure-captured loop var; Python mutable default args, late-binding
closures; Rust panics on byte-index slicing, unwrap on user input; SQL
injection; timezone/DST drift; float equality. Flag any instance the diff
introduces.

**Angle E — wrapper/proxy correctness** *(xhigh/max only).* When the change
adds or modifies a type that wraps another (cache, proxy, decorator, adapter):
check that every method routes to the wrapped instance and not back through a
registry/session/global — e.g. a caching provider holding a `delegate` field
that resolves IDs via `session.get(...)` instead of `delegate.get(...)` will
re-enter the cache or recurse. Also check that the wrapper forwards all the
methods callers actually use.

### Cleanup + altitude angles

The angles above hunt for bugs; these hunt for cleanup in the changed code.

**Reuse.** Flag new code that re-implements something the codebase already has
— Grep shared/utility modules and files adjacent to the change, and name the
existing helper to call instead.

**Simplification.** Flag unnecessary complexity the diff adds: redundant or
derivable state, copy-paste with slight variation, deep nesting, dead code left
behind. Name the simpler form that does the same job.

**Efficiency.** Flag wasted work the diff introduces: redundant computation or
repeated I/O, independent operations run sequentially, blocking work added to
startup or hot paths. Name the cheaper alternative.

**Altitude.** Check that each change is implemented at the right depth, not as a
fragile bandaid. Special cases layered on shared infrastructure are a sign the
fix isn't deep enough — prefer generalizing the underlying mechanism over
adding special cases.

**Prose.** Flag writing in the diff that breaks `.claude/rules/prose.md` in a way
no script can see. `scripts/check-prose.sh` already covers the four measurable
limits, so do **not** re-report a long sentence or an over-long comment block.
This angle owns the rest:

- An imperative step over 20 words (the gate's 25 is the *descriptive* limit).
- Passive voice where the agent is known.
- A noun cluster of more than 3 words.
- Filler: "it is worth noting", "importantly", emphasis that only restates.
- The wrong shape: prose doing a list's job, or a list doing a table's.
- A comment doing another file's job. A rejected alternative belongs in
  `docs/adr/`, and what the code used to do belongs in the commit message.

Report against the rule and name the shorter form, the same way the
private-data angle names the placeholder.

**Private-data leak.** Flag any private/personal/company-internal data the diff
introduces into a **shipping** file (everything except `docs/plans/**` and
`WORKSPACES.md` ships verbatim to the public mirror — test fixtures and comments
included): real names used as fixtures, personal/family data, internal
repo/app/org names, real home paths, named live workspaces (`personal`/`work`).
The definition, carve-out (legitimate attribution), and approved placeholders
live in `.claude/rules/no-private-data.md` — flag against that rule and name the
placeholder to use.

Cleanup and altitude candidates use the same `file`/`line`/`summary` shape; in
`failure_scenario`, state the concrete cost (what is duplicated, wasted, or
harder to maintain) instead of a crash. **Correctness bugs always outrank
cleanup and altitude findings** when the output cap forces a cut.

Pass every candidate with a nameable failure scenario through to Phase 2 —
finders that silently drop half-believed candidates bypass the verify step and
are the dominant cause of misses.

## Phase 2 — Verify (1-vote, 3-state)

Dedup candidates that point at the same line/mechanism, keeping the one with the
most concrete failure scenario. For each remaining candidate, run **one
verifier** (subagent if available, else an inline focused pass): give it the
diff, the relevant file(s), and the candidate, and have it return exactly one
of:

- **CONFIRMED** — can name the inputs/state that trigger it and the wrong output
  or crash. Quote the line.
- **PLAUSIBLE** — mechanism is real, trigger is uncertain (timing, env, config).
  State what would confirm it.
- **REFUTED** — factually wrong (code doesn't say that) or guarded elsewhere.
  Quote the line that proves it.

Keep candidates where the vote is CONFIRMED or PLAUSIBLE. In recall mode
(xhigh/max) a single non-REFUTED vote carries the finding — do NOT drop on
uncertainty.

## Phase 3 — Sweep for gaps *(xhigh / max only)*

Run **one more finder** as a fresh reviewer who has the verified list. Re-read
the diff and enclosing functions looking ONLY for defects not already listed.
Do not re-derive or re-confirm anything already there — the job is gaps. Focus
on what the first pass tends to miss: moved/extracted code that dropped a guard
or anchor; second-tier footguns (default evaluated once, `hash()`
non-determinism, lock-scope shrink, predicate methods with side effects);
setup/teardown asymmetry in tests; config defaults flipped.

Surface up to N additional candidates, each naming a defect not already on the
list, and run them through Phase 2. If nothing new, return an empty sweep — do
not pad.

## Output

Findings are **structured data for the caller** (`/harden` Phase 1), never a
message for the reader. Report them through the structured channel your runtime
offers — and **never print a findings array as prose: not empty `[]`/`{}`, not
populated, not fenced, not inline.** A bare `[]` in the transcript is meaningless
noise; it is the exact recurring bug this contract exists to prevent (see
`docs/temporary-measures.md` § "code-review findings array leaking into chat").
Keep the machine-readable findings path separate from the chat path — that
separation, not a downstream text filter, is the fix.

Each finding is an object, ranked most-severe first, capped at `max findings`:

```json
{
  "file": "path/to/file.ext",
  "line": 123,
  "summary": "one-sentence statement of the bug",
  "failure_scenario": "concrete inputs/state → wrong output/crash"
}
```

Route the findings by what your runtime offers:

- **You have a `ReportFindings` tool (Claude Code):** call it **exactly once**
  with the findings array — ranked most-severe first, and an **empty array when
  nothing survives verification**. It renders the findings structurally instead
  of as text, so the empty case never becomes a stray `[]`. Do NOT also print the
  findings. This is the whole handoff: the tool result stays in your context, so
  you still fix the real findings in `/harden` Phase 1.
- **You have no such tool (Codex / any other agent):** the findings array is your
  own structured working record — `/harden` Phase 1 runs this review inline, so
  *you* are the caller: build the array, then act on it directly (fix the real
  findings in Phase 1). Keep it internal — **report to the reader in prose only,
  never paste the array**. When nothing survives, write `No findings.` and proceed;
  never a bare `[]`.

See `harden.md` Phase 1 for the caller's apply/report rules.
