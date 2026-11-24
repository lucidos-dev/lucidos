# 0790: The static command guard hard blocks only universal catastrophes and never settles ambiguous commands as Safe

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0002](0002-lucidos-agent-command-safety.md), [0644](0644-command-guard-on-by-default.md), [0728](0728-command-guard-one-owner-per-harm.md)

## Context

The command guard in `command_guard.rs` classifies each shell or Python
command. The LLM judge is the primary classifier for the ambiguous middle. It
widens coverage and cuts false positives.

When the judge is off or unavailable, static code must still decide. This
record states how that static code is shaped and which gaps it accepts.

This record restates the design comments of
`crates/lucidos-engine/src/engine/command_guard.rs`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

**Hard-block lane**

- `catastrophic_reason` is deliberately narrow. It holds only universally
  destructive patterns that no legitimate task needs.
- Out-of-workspace deletion of a specific path, such as `rm -rf /etc`, is not
  caught there. It belongs to the ask and judge lane.
- The whole-command-line form of `catastrophic_reason`, wrapper included, is
  the one guard lane a coding agent's background task takes.
- The hard-block is the floor that no grant can lift.

**Static fallback**

`fallback_classify` handles the ambiguous middle in this priority order:

1. An obvious side-effect shape gives `IrreversibleDanger` with that category.
2. A destruction shape aimed outside the workspace gives `IrreversibleDanger`
   with `SideEffectCategory::OutOfWorkspaceDestruction`.
3. A destruction shape confined to the workspace gives `ReversibleDanger`,
   which means checkpoint and run, unless the command also holds an unexplained
   refusal.
4. Any other fast-path refusal gives `IrreversibleDanger` with
   `SideEffectCategory::Other`.
5. Everything else is an omission and settles `Safe`.

**Scan rules**

- The `IrreversibleDanger` list is a supporting detector, not the primary one.
  It stays conservative.
- Heads that overwrite their destination are the last operand or a
  target-directory value. `cp` and `install` only read their sources.
- A path counts as out-of-workspace evidence only for destruction.
  Out-of-workspace reads are a wanted feature.
- Python destruction scope pairs a destruction call with any string literal.
  One literal outside the workspace makes it out-of-workspace.
- A literal the guard cannot place asks. A destruction call on a pure variable
  path stays in-workspace.
- A short option's glued value is the text after the option character. A scan
  for the first `/` is not used.

**Head resolution**

- The three danger scans resolve heads through one shared resolver, so the
  trigger grant check cannot skip a head and auto-allow.
- A flag-shaped head branches into both readings, and every reading is scanned.
- An exec wrapper's own options are stepped over to the first non-flag token.
  That adds at most one candidate and cannot chain.
- The exec-wrapper list is not a completeness claim. A miss is repaired by
  adding a name, not by debate.
- `segment_safety` does not use this resolver.

**Allowlist and quoting**

- The read-only allowlist refuses substitution, code-injecting preambles,
  path-qualified heads, and out-of-workspace or git-metadata paths under
  write-capable heads.
- For git output flags, only long spellings count. A bare `-o` is not matched.
- Quote tracking uses three runs: single quotes, ANSI-C `$'...'`, and double
  quotes.

## Rationale

The judge is the primary classifier, so the static path only needs to cover
headline shapes. Every static match costs a permission prompt or a trigger
block on the fallback path, so the list stays conservative.

Resolution that can only add a danger verdict is safe. Resolution that settles
a command as `Safe` is not. That is why `segment_safety` does not use the
branching resolver. Doing so would newly settle decorated commands as Safe.

Enumerating wrapper flag arities fails open on every flag not listed. Branching
into both readings avoids that. Scanning to the first non-flag avoids the chain
that would turn `xargs -0 grep -l gcloud` into a cloud-CLI call.

The three quote runs differ on whether `$(` expands and whether backslash
escapes. Collapsing any pair loses a real command. Tracking single quotes alone
reads `echo "'$(rm -rf /)'"` as quoted.

Git `-o` is rejected on the subcommands that can write. Matching it would cost
a judge call on a routine read and buy no safety.

## Consequences

The static path is coarser than the judge by construction. Known residuals:

- Destruction behind a variable path, and in-place editing flags, are caught
  only by the judge.
- A plain file write is not flagged. Flagging every write would tax routine
  data output, and an out-of-workspace one is an implausible screwup.
- A wrapper flag whose value is a separate token hides the real command. `xargs
  -I {} rm -rf /` resolves `{}`. Glued and `--flag=value` forms resolve.
- An unlisted exec wrapper fails open. The cost is a judge call that did not
  happen.
- The write-capable read-only entry is coarse. `sort /etc/passwd` and `tree -I
  .git` are refused even though they only read. Telling read from write needs
  per-head flag arity.
- A stray outside path next to an unrelated delete errs toward ask.

## Alternatives considered

- Enumerating each wrapper's flag arity was rejected. It fails open on every
  flag not listed.
- A naive head walk was rejected. It stops on a `-`-prefixed token and resolves
  the head to the flag, so the line settles `Safe`.
- Hard-blocking out-of-workspace deletion of a specific path was rejected for
  the deterministic lane. It goes to ask or judge.
- Matching a bare `-o` as a git output flag was rejected. It buys no safety.
- Treating `$'...'` as a plain single-quoted run was rejected. It inverts the
  quote state after the first `\'`.
