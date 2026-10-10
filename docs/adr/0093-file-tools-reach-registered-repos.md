# 0093: Chat file tools reach a registered repository: reads scoped, one uncommitted edit

- **Status**: Accepted
- **Date**: 2026-08-19

## Context

The chat file tools resolve every path through `normalize_data_path`, which
refuses an absolute path outright. `glob_files` and `grep_files` walk
`BROWSEABLE_DATA_SUBDIRS`, four directories under `data/`. A code repository is
outside both, so no file tool could reach one.

The agent works on code anyway. Measured over the nightly pipeline thread in the
dev workspace, 265 tool calls:

| tool | calls |
|---|---|
| `run_bash` | 115 |
| `run_python` | 39 |
| `edit_file` | 13 |
| `bash_output` | 10 |
| `read_file` | 5 |
| `grep_files` | 4 |

It reached for the native tool first. One `read_file` named an absolute path
inside the Lucidos checkout, and came back `Error: Path traversal not allowed`.
Every later repo read went through bash. 89 of the 115 bash calls open with
`cd`, and 80 of those target a path already registered as the repository
`Lucidos`. The bodies are `sed -n '175,265p'`, `grep -rn --include=*.rs` and
`awk 'NR>=382 && NR<=660'`.

That is `read_file` and `grep_files` spelled by hand. What it loses: 50 KB
chunking, `start_line` slicing, the 300-char line cap, the ~50 KB match cap, the
binary sniff and content sanitation.

Separately, `edit_file` text mode answered
`[ACTION COMPLETED] UPDATED: <path> (commit: <sha>)`. Without `replace_all` it
calls `replacen(old, new, 1)`. That message was identical whether the file held
one occurrence or nine. The same thread rewrote a knowhow file with
`str.replace` in `run_python` and printed the count itself.

## Decision

`read_file`, `glob_files`, `grep_files` and `edit_file` take an optional `repo`,
naming a registered repository by name or id. With it set, `path` and `pattern`
are repo-root-relative. With it absent, every tool behaves exactly as before.

Reads see the working tree. Enumeration runs
`git ls-files --cached --others --exclude-standard`.

`edit_file` with `repo` requires `commit: false`. It writes the working tree and
commits nothing. `write_file`, `delete_file` and `copy_file` take no `repo`.

`edit_file` text mode reports how many occurrences it replaced, of how many it
found, and names `replace_all` when the two differ.

## Rationale

**Why reads at all.** This is ADR 0051's scratch argument one tree over. The
engine already hands the agent these repositories: `manage_repositories`
registers them, and `run_coding_agent` spawns sessions into them. Refusing to
read one sends the model to `run_bash cat`, whose output arrives raw and
untruncated. The engine was naming paths it would not serve.

**Why registered repositories only.** No file tool takes a filesystem path, so
the reachable set is exactly what the user registered under Settings. This
grants no reach the agent lacks, since `run_bash` already reads any path on the
host. What it buys is a consented, named surface, and a schema with no path
argument to abuse.

**Why the working tree rather than a ref.** The agent needs to see uncommitted
work, which is what bash gave it. `GET /api/v1/repositories/:id/file?ref=`
already serves ref reads for the UI's file explorer.

**Why `git ls-files`.** A grep over a Rust repo must not descend `target/`, and
a walk of `node_modules` is worse. `.gitignore` is the only correct answer, and
git is the only thing that reads it properly. `--others --exclude-standard`
keeps a file written five minutes ago visible. `--cached` entries are filtered
against disk, so a tracked path deleted from the tree never reaches a caller.

**Why one write.** A one-line fix in a repository otherwise costs a whole
coding-agent spawn: a worktree, a branch, a session. That is the right shape for
work the user should review, and heavy for a typo. What the tool adds over
`run_python` is a verified occurrence count and a containment guard.

**Why the write never commits.** Every other file-tool write git-commits, which
is the whole reason they exist beside `run_python`. That contract is right for
`data/`, the artifact store. Pointed at someone's checkout the same contract
commits onto whatever branch is out, unreviewed and unasked. So the repo edit
drops the commit rather than aiming it somewhere.

**Answering ADR 0051 directly.** That ADR rejected a commitless file-tool write
for the scratch tree, and this allows one for a repository. The reversal is
deliberate, and the difference is the alternative. For scratch, `run_python`
gave an identical result, so a second result shape bought nothing. For a
repository it does not: Python cannot report what a replace matched, or prove
the path stayed inside the repo. In the steps UI it reads as opaque code, not as
an edit to a named file.

**Why `commit: false` is required, not defaulted.** Defaulting it would be less
typing and strictly worse. The argument's job is to put the consequence in the
recorded call. A user reading the step then sees an uncommitted write as such,
rather than inferring it from an argument that is absent.

**Why not `write_file` or `delete_file`.** Creating and removing files is
structural work, and the result wants review as a whole. `run_coding_agent`
lands that as a change with a branch and an Apply, and remains the route.

## Consequences

- The `data/` boundary stays exactly where ADR 0051 left it. `commit: false`
  without `repo` is refused, so no path under `data/` can be written
  uncommitted.
- A repo edit is invisible to the timeline. It emits no event, moves no ref and
  writes no index entry, so the working tree is the only record. The tool result
  says so and points at `git diff`.
- A coding agent spawned into a repository the chat agent has edited branches
  off a dirty tree. That was already true of `run_python`, and a visible tool
  result makes it likelier to be noticed.
- Gitignored files stay unreachable through `repo`. Reading a build log inside a
  coding-agent worktree is still bash work.
- A search never follows a symlink. `read_file` may, because
  `resolve_in_repo` proves containment per call; a walk cannot, because
  `grep_files` reads its entries directly. So `repo_entries` drops every
  symlink, matching `list_searchable_data_files` one tree over. The target is
  listed under its own real path anyway.
- `glob_files` and `grep_files` now take their file list as a parameter, so
  `data/` and repo walks share the matching, the caps and the brace expansion.
- The always-loaded context grows by 1,893 characters, billed on every request
  of every thread: 1,362 in tool schemas and 531 in the prompt.
  `ALWAYS_LOADED_BUDGET_CHARS` moves with its reason recorded on the constant,
  and `edit_file` takes a per-tool ceiling exception.

## Alternatives considered

**A `repo:<name>:<path>` prefix on `path`.** Matches the UI deep-link form
already used for repo files. Rejected: it collides with a real path containing a
colon, and hides the target switch inside a string the resolver must guess at.
An argument the model either passes or does not is unambiguous.

**A separate `read_repo_file` tool.** Keeps each tool single-purpose. Rejected
on cost and on behavior: three more schemas billed on every request, and the
model demonstrably reaches for `read_file` first. That habit is worth serving
rather than retraining.

**Expose the repository read-only, with no write at all.** The plan this change
started from. Rejected by the user on the cost. Every one-line fix would need a
coding-agent spawn, when `run_python` can already write the same bytes with no
safety and no record.

**Let the repo edit commit.** Symmetric with `data/`, and it leaves a real
record. Rejected: it commits to whatever branch is checked out, with no review
and no Apply, which is what the change-proposal flow exists to prevent.

**Another documentation pass.** The chat system prompt could simply tell the
agent to prefer `grep_files`. It already does, in its SEARCHING FILES section,
and the tools could not reach the repo, so the guidance was unfollowable. ADR
0051 ran this experiment and measured it failing.
