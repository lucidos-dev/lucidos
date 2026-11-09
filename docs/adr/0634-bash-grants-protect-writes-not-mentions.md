# 0634: A Bash grant covers a command that only mentions .claude/ or .git/; only a write there asks

- **Status**: Accepted. Widened by ADR 0638, which reads a printing `sed`, a
  read-only substitution, and loop keywords.
- **Date**: 2026-11-09

## Context

The workspace floor change kept every `Bash` and `Bash(...)` grant out of
`claude --allowedTools`, so Claude Code can no longer match one without the
floor. Since then the engine decides every Bash call through
`persisted_allow_covers_at`.

That path carried an older rule: a Bash command whose text contained `.claude/`
or `.git/` got no persisted pattern. The rule existed so a card would not store
a line Claude Code ignores. It was a raw substring match, which cost little
while Claude Code matched `Bash` itself and asked the engine only for the
writes it protects.

Once the engine matched alone, every mention carded: a handoff note naming a
rule file, a grep pattern, a `find -not -path '*/.git/*'`. None of those cards
offered "Always allow", so a user with a bare `Bash` grant could never stop
them. In one workspace, Bash calls naming `.claude/` carded about 1% of the
time before the change, and 8% (plain) to 50% (heredoc) after it.

## Decision

A persisted Bash grant is refused only for a command that may WRITE under
`.claude/` or `.git/` (`command_guard::command_may_write_under`). A command that
only mentions or reads those paths is covered like any other.

"Only reads" is a property of the whole command, not of the segment that
names the path. A command naming either directory is covered only when every
segment has a read-only head. A read can hand the path on to a writer: a pipe
into `xargs` or `sh`, a variable, a heredoc fed to an interpreter, or a script
file a later segment runs.

## Rationale

The original reason for the rule is gone: the engine answers every Bash call,
so a stored line always means what the engine says. One reason to protect the
two directories remains. A write under `.claude/` can widen the agent's own
permissions or add hooks. A write under `.git/` can plant a hook or config that
runs on every later git call, including the engine's own on Apply. Neither
shows in the reviewed diff.

A mention carries neither risk, and a bare `Bash` grant already covers any
command that does not touch them.

The check errs towards a write:

- Any segment without a read-only head counts, whatever it names.
- A substitution counts.
- A shell `-c` layer is read through only as a bare wrapper. A preamble or an
  operand after the script keeps the shell head.
- An unclosed quote or a `#` comment falls back to the plain split. That split
  still starts a segment at every real head.
- A `(` or `{` word counts, since it may define a function under a read-only
  name.

The check reads text, so it misses a write the text does not name. A path
built from pieces in a variable is one. A script written in an earlier call is
another. The old substring rule missed those too. The rule catches the
ordinary commands an agent writes, not an agent working against it.

## Consequences

- Bare `Bash` users stop seeing cards for mentions and reads.
- A write under either directory still cards, with no persisted button, as
  before.
- `sed` and other heads off the read-only list still card when they name a
  protected path, since they may write. That matches what Claude Code asked
  for before the floor change.
- So does a command that mixes a read with any other head. A handoff note
  written in a heredoc and passed to `lucidos successor start` in the same
  call cards. Written in its own call, the note is covered.

## Alternatives considered

- **Put Bash grants back into `--allowedTools`.** Restores Claude Code's own
  matching, but Claude Code then matches with no workspace floor and no
  secret-read check. It also matches in unattended runs, where ADR 0125 says a
  clicked grant must never decide.
- **Hold the floor in the Bash PreToolUse hook, and match natively.** Needs an
  engine round trip on every Bash call that fails closed, from a CLI hook that
  fails open by design.
- **Drop the protected-path rule entirely.** Simpler, but then a `Bash` grant
  lets an agent rewrite its own permissions and git hooks with no card.
