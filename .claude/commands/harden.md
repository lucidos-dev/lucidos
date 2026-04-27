Harden the code: review changed code for reuse, quality, and efficiency, then fix any issues found. Check for bugs and CLAUDE.md compliance. Run this before finishing your session.

## Phase 0: Check if Already Hardened

`lucidos hardened query` prints `FRESH`, `STALE`, or `MISSING` for the branch
in `$PWD`. `FRESH` means HEAD still matches the SHA recorded by the last
`/harden`; `STALE` means CC has committed since then and a re-run is needed.

```bash
[ "$(lucidos hardened query 2>/dev/null)" = "FRESH" ] && echo "ALREADY_HARDENED" || echo "NOT_HARDENED"
```

If the output is `ALREADY_HARDENED`, inform the user: "Already hardened — skipping." and stop. Do NOT re-run hardening.

## Phase 1: Run /simplify

Invoke the `/simplify` skill using the Skill tool. This reviews the branch diff for code reuse, quality, and efficiency issues and auto-fixes them.

Wait for simplify to complete. If it made changes, commit them before proceeding.

## Phase 2: Launch Three Hardening Agents in Parallel

Run `git diff main...HEAD` to get the current diff (including any simplify fixes). Also run `git diff main...HEAD --name-only` to get the list of changed files. Launch three agents in parallel:

### Agent 1: Bug Detection

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

## Phase 5: Create Marker

After all phases complete (and any bug fixes are applied), record this branch's
HEAD as hardened in the parent workspace's DB:

```bash
.claude/hooks/mark-harden.sh
```

State lives in the `hardened_branches` DB table (keyed by repo root + branch),
not on disk — do not look for or manage any marker files.

Inform the user: "Hardening complete. Session can finish."
