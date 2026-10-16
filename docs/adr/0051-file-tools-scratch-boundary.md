# 0051: Chat file tools reach .lucidos/tmp/ read-only, never write outside data/

- **Status**: Accepted
- **Date**: 2026-08-06

## Context

`.lucidos/tmp/` is the workspace's ephemeral scratch tree: gitignored, not
indexed, safe to delete. Several engine tools write into it and then name the
path back to the LLM. `http_request(temp_path)` answers
`[SAVED] .lucidos/tmp/<f>`; `git_clone`'s tmp route answers `CLONED TO TMP: …`
and instructs the agent to extract from it with `copy_file`; the chat system
prompt documents the prefix and `system-knowhow/best-practices.md` rule 8 makes
it the default landing place for clone-and-inspect work.

The file tools could not address any of it. `normalize_data_path` routed every
path without a typed prefix under `artifacts/`, so `.lucidos/tmp/x` became
`artifacts/.lucidos/tmp/x`. Measured over the dev workspace event log, 169
`ToolResult` events named such a path, split two ways:

- **75 loud.** `read_file` and `copy_file`-source failed with
  `file not found: artifacts/.lucidos/…`, naming a path nobody had asked for.
  The model recovered by shelling out to `run_bash cat`, bypassing `read_file`'s
  chunking, line slicing, image handling and content sanitation.
- **94 silent, and worse.** `write_file` and `copy_file`-dest *succeeded at the
  wrong place*: they created and git-committed `data/artifacts/.lucidos/tmp/…`
  into the user's tracked artifacts repo and reported
  `[ACTION COMPLETED] CREATED`. 14 such commits sit in the dev workspace's
  artifact history.

A documentation fix was tried first and failed. Rule 10 of `best-practices.md`
("`read_file` is `data/`-only") was added on 2026-07-27 to teach the model
around the hole; seven more occurrences followed in the next ten days. Per
`.claude/rules/temporary-measures.md`, guidance had already failed, which is the
condition for fixing it in code.

## Decision

`.lucidos/tmp/` resolves against the workspace root as a first-class prefix in
`normalize_data_path`, **readable but not writable** by the file tools. Reads
(`read_file`, `copy_file`-source) land there. Mutations (`write_file`,
`edit_file`, `delete_file`, `copy_file`-dest) are refused at the existing
`read_only_reason` chokepoint with a message naming `run_python`.

Every other path under `.lucidos/` is refused in both directions.

## Rationale

**Why readable.** The engine creates the file, prints the path, and tells the
agent to act on it. Refusing to read that path is the engine contradicting
itself one tool call later, and the agent's only recourse is a shell command
that returns raw untruncated bytes. The `git_clone` case is sharper still: the
tool result explicitly says "extract via `copy_file`", and `copy_file` could not
resolve the source the same tool had just named.

**Why not writable.** Every file tool git-commits what it writes; that is the
whole reason they exist alongside `run_python`. A gitignored tree has nothing to
commit, so a write would either commit nothing while reporting a commit sha (the
existing bug, wearing a different mask) or need a second commitless result
shape threaded through `commit_file_change`. `run_python` runs with cwd at the
workspace root and already does this correctly, and the system prompt already
points at it.

**Why only `tmp/`.** `.lucidos/worktrees/` holds complete Lucidos source
checkouts, one per coding-agent thread. `exhaust/` is engine runtime scratch and
the system prompt already marks it "do not reference". `engine.pid`, `ports` and
`cc-commands.json` are process state. None of that is "a file in the workspace"
in the sense `read_file` advertises, and handing the chat model an entire source
tree through a workspace-file tool is not a thing to do by accident.

**Why a symlink guard.** `is_path_traversal` is a string check. `git_clone`'s
tmp route clones arbitrary repositories into this tree, and a repository may
carry a symlink pointing anywhere. String validation cannot bound where the read
lands, so the resolver canonicalizes the tmp root and the target and refuses a
target outside. It refuses only on a *proven* escape: a path that does not exist
fails to canonicalize and falls through to the ordinary "file not found", so a
missing file never masquerades as a security error.

**Precedent.** `normalize_data_path` already refuses `data/<untyped>` on exactly
this reasoning, in nearly these words: silently routing `data/.env` to
`artifacts/.env` "would commit a secret into the tracked artifacts repo". The
scratch case is that hazard one prefix over. The difference is that this one was
live rather than hypothetical.

## Consequences

- The `read_file` contract widens from "under `data/`" to "under `data/`, under
  `.lucidos/tmp/`, or under `system-knowhow/`". Rule 10 of `best-practices.md`
  is rewritten accordingly; its old claim that `system-knowhow/` is "the only
  prefix that reaches outside" is now false.
- A previously-"successful" `write_file(".lucidos/tmp/…")` now returns an error.
  That is the point: it was succeeding at the wrong place. The refusal names
  `run_python`, so the model has a route rather than a wall.
- `crate::core::TMP_DIR` is the single spelling of the prefix. A tool that
  writes scratch and prints the path must use it, or the string the resolver
  matches and the string the LLM is handed can drift apart, which is how this
  bug was possible at all.
- Existing junk under `data/artifacts/.lucidos/` is *not* cleaned up. It is the
  user's data in the user's workspaces; this change stops new instances only.
- `list_files`, `glob_files` and `grep_files` still walk `data/` alone. A clone
  under `.lucidos/tmp/` must not appear in artifact listings, which are a
  performance axis (rule 8).

## Alternatives considered

**Refuse `.lucidos/` in both directions, with a clear message.** Strictly
smaller, and it fixes the silent-commit half completely. Rejected because it
leaves the loud half: `read_file` still cannot read the file `http_request` just
wrote, so the model keeps falling back to `run_bash cat`, and `git_clone`'s
documented extract-with-`copy_file` workflow stays broken. The engine would
still be naming paths it refuses to serve, just more politely.

**Improve only the error message.** The cheapest option, and the one closest to
what was already tried. Rejected: it does nothing about the 94 silent
mis-commits, which are the more serious half and are invisible from the UI.

**Allow writes to `.lucidos/tmp/` with the commit skipped.** Most convenient for
the model, and superficially symmetric. Rejected because it punches a hole in
the one contract that distinguishes the file tools from `run_python`: everything
they write is committed and recoverable. It would need a commitless branch in
`commit_file_change` and a second result shape with no sha, to duplicate a
capability `run_python` already has.

**Expose all of `.lucidos/` read-only.** Simpler to state and to implement.
Rejected on scope: it makes every coding-agent worktree, and therefore the
entire Lucidos source tree, readable through a tool described to the model as
reading "files in the workspace".

**Another documentation pass.** Already run, on 2026-07-27, and measurably
insufficient. Repeating a failed intervention is not an alternative.
