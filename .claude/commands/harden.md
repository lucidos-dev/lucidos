Harden the code: review changed code for reuse, quality, and efficiency, then fix any issues found. Check for bugs and CLAUDE.md compliance, then run the test suites for what was touched (iterating from Phase 1 if anything fails). Run this before finishing your session.

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

- Skip Phase 1 (`/simplify` looks for code-shaped issues that don't apply to prose).
- Skip Phase 2 Agent 1 (no code logic to bug-check).
- Phase 2 Agents 2 and 3 (compliance, regression), Phase 3, Phase 4, Phase 5 still run.
- Phase 4.5 already auto-skips for docs-only via its test-selection table.

Do NOT extend this fast path to "string-only" or "comment-only" `.rs` edits. Strings can carry format args, escape sequences, regexes, or be parsed at runtime — any `.rs` change keeps the full cycle.

## Phase 1: Run /simplify

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this phase entirely and proceed to Phase 2.

Invoke the `/simplify` skill using the Skill tool. This reviews the branch diff for code reuse, quality, and efficiency issues and auto-fixes them.

Wait for simplify to complete. If it made changes, commit them before proceeding.

## Phase 2: Launch Three Hardening Agents in Parallel

Run `git diff main...HEAD` to get the current diff (including any simplify fixes). Also run `git diff main...HEAD --name-only` to get the list of changed files. Launch three agents in parallel:

### Agent 1: Bug Detection

**Docs-only fast path:** if Phase 0.5 flagged this diff as docs-only, skip this agent (Agents 2 and 3 still run).

Scan the diff for bugs and incorrect logic. Tag each finding with a severity:

- 🔴 **Bug** — will break production, must fix before merging
- 🟡 **Nit** — worth fixing but not blocking

Do NOT use a "pre-existing" category. If a bug exists in a file touched by this branch, it must be classified as 🔴 or 🟡 based on severity — "it was already broken" is not a valid excuse to skip it.

Focus on:
- Code that will fail to compile or parse (syntax errors, type errors, missing imports, unresolved references)
- Code that will definitely produce wrong results regardless of inputs (clear logic errors)
- Security vulnerabilities in the changed code (injection, auth bypass, data exposure)

**HIGH SIGNAL ONLY.** Do NOT flag:
- Code style or quality concerns (already handled by simplify)
- Potential issues that depend on specific inputs or state
- Subjective suggestions or improvements

### Agent 2: CLAUDE.md Compliance

Check the changes against all applicable CLAUDE.md files (root and any in directories containing modified files). Flag only clear, unambiguous violations where you can quote the exact rule being broken.

Do NOT flag:
- General best practices not mentioned in CLAUDE.md
- Issues silenced by lint ignore comments
- Pedantic nitpicks

### Agent 3: Regression Check

For each modified file, run `git log --oneline -10 <file>` to see recent history. Check if the current changes revert, contradict, or undermine recent fixes or intentional refactors. Only flag clear regressions where you can point to a specific prior commit that the new change undoes.

## Phase 3: Validate Findings

Wait for all three agents to complete. For each issue found, launch a parallel validation subagent that reads the actual code and verifies the issue is real. The validator should:
- Read the relevant source files (not just the diff)
- Confirm the issue actually exists in the code
- Discard findings that are false positives or depend on assumptions about runtime state

Only issues confirmed by validation proceed to the report.

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
- Mixed → run both

If everything passes, proceed to Phase 5.

If anything fails: fix the failures (or the code that caused them), commit the fixes, and **return to Phase 1**. Fixes are new code that hasn't been reviewed by `/simplify` or the hardening agents — re-run the cycle on the updated diff. Iterate until tests pass on a fully-hardened diff.

## Phase 5: Create Marker

After all phases complete (and any bug fixes are applied), record this branch's
HEAD as hardened in the parent workspace's DB:

```bash
.claude/hooks/mark-harden.sh
```

State lives in the `hardened_branches` DB table (keyed by repo root + branch),
not on disk — do not look for or manage any marker files.

Inform the user: "Hardening complete. Session can finish."
