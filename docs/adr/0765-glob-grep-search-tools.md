# 0765: The glob and grep tools share one matching core that takes a file list

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0090](0090-brace-patterns-expand-rather-than-error.md), [0093](0093-file-tools-reach-registered-repos.md)

## Context

`search.rs` holds the pure helpers behind the `glob_files` and `grep_files`
tools.

The tools walk the workspace `data/` directory, scoped to the subdirectories
`list_files` walks: artifacts, apps, knowhow and triggers. Patterns are
relative to `data/`, so the model can reuse the paths it sees in `list_files`
output, such as `apps/**/index.html`.

A registered repository must be searched too. Its entries come from
`engine::tools::repo_files`.

This record restates the design comments of
`crates/lucidos-agents-api/src/engine/tools/search.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- Each tool comes in two forms. `glob_files` and `grep_files` walk `data/`.
  `glob_entries` and `grep_entries` take a file list instead.
- A repository search calls the `_entries` form with the list that
  `engine::tools::repo_files` supplies.
- Brace expansion, the match caps, the binary sniff and the context window are
  shared by both forms.
- Only the source of the paths differs between them.
- Both tools compile a `data/`-relative pattern through one function. It yields
  one matcher per brace alternative, and a path matches when any of them
  matches.

## Rationale

The caps, the binary sniff and the context window are the valuable part of the
search tools. They must not be reimplemented for each file source.

`glob_entries` reuses the matching, the brace expansion and the limit rather
than reimplementing them for a walk of something other than `data/`.

Compiling every pattern in one place means a pattern cannot come to mean one
thing to `glob_files` and another to the `path_glob` of `grep_files`.

## Consequences

- A new file source supplies a list of paths and gets the same matching and
  limits as `data/`.
- A change to brace handling, the caps, the binary sniff or the context window
  applies to every source at once.
- The helpers stay pure: they take paths and patterns, and the walk of the
  source lives with the caller.

## Alternatives considered

- Reimplementing matching, brace expansion and the limit for each file source
  was rejected.
- Reimplementing the caps, the binary sniff and the context window for each
  file source was rejected.
- Letting each tool compile its own patterns was rejected, because one pattern
  could then mean two things.
