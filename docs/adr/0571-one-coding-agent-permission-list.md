# 0571: One coding-agent permission list, one meaning per line

- **Status**: Accepted
- **Date**: 2026-10-24
- **Supersedes**: [0564](0564-codex-grants-share-bash-lines-and-trusted-repo-scripts.md) § 3, and the "never granted" half of § 1

## Context

Settings shows one list, *Coding agent permissions* (`cc-allowed-tools`). A
line in it meant different things per agent:

- A bare `Bash` line allowed every command in Claude Code and nothing in Codex
  (ADR 0564 § 3).
- A Codex card on `/bin/zsh -lc ./scripts/harden-gates.sh` offered "Always
  allow `harden-gates.sh` commands". The click stored `Bash(harden-gates.sh:*)`,
  and the engine refuses every path head, so the line covered nothing, for
  either agent.
- The card chose its buttons through a TypeScript copy of the engine's grant
  derivation. Nothing tied a button to what the engine would honour.

## Decision

1. **Every line means the same whichever agent asks.** Bare `Bash` means "any
   command" for Codex too, and a Codex command card offers the bare "Always
   allow". The exceptions are Claude Code's own: a secret read, a catastrophic
   command, and a command touching `.claude/` or `.git/` (ADR 0125) still card.
2. **A trusted repo script is grantable by its exact path.** The card stores
   the worktree-relative path, `Bash(./scripts/harden-gates.sh:*)`. A line like
   it covers a request only when all three hold at request time:
   - the head is a trusted repo script (ADR 0564 § 1 and § 2);
   - it resolves to that same tracked path;
   - no segment runs before it.

   A later segment still needs its own grant, or must settle Safe on its own.
3. **Path lines never reach Claude Code's `--allowedTools`.** Claude Code
   prefix-matches a line with no trust check, so the engine's gate answers
   those requests instead.
4. **The engine decides the buttons.** It stamps the offer (`broad`, `narrow`,
   `session`) on the permission request event. It offers a pattern only when
   storing it would cover that same request. The consent endpoint stores
   exactly the stamped pattern, and session hydration reads it back from the
   event.

## Rationale

The user reads one list, so one line must have one meaning. Two meanings in
one file is the bug, whatever each half intended.

ADR 0564 rejected both halves of this. Its reasons, answered:

- **"Too wide for commands that escaped a sandbox."** The user picks the bare
  "Always allow" deliberately, and Claude Code has offered the same button
  since the start. The exceptions above still card. The trade is now the
  user's to make.
- **"An earlier segment can rewrite the script."** A path grant covers nothing
  that runs before its head, so `echo x > s.sh && ./s.sh` still cards.
- **"A symlink can swap the target."** Trust follows symlinks and compares
  bytes with the default branch at request time. A link leaving the worktree,
  or one pointing at changed bytes, is untrusted.
- **The basename was the defect, not the grant.** `harden-gates.sh` names any
  file anywhere. `./scripts/harden-gates.sh` names one tracked file.

A button the engine does not stamp cannot lie. The TypeScript copy drifted
from the engine once already, and the stored line was dead on arrival.

## Consequences

- A user who clicks the bare "Always allow" on a Codex card stops every Codex
  command carding, except the three exceptions. It also stops Claude Code
  carding, as it always did.
- A path grant stops carding while the script matches the default branch. Once
  an agent edits it, it cards again until the edit lands.
- A relative script head from Claude Code stays ungrantable. Its `Bash`
  request carries no `cwd`, so trust cannot resolve, and the card offers no
  path button.
- A command whose segments need two grants offers no narrow button. One line
  could not cover it, so the button would be dead.
- An existing basename line such as `Bash(harden-gates.sh:*)` stays inert. The
  user deletes it in Settings.
- The unattended lane is unchanged. Grants sit below it, and it never reads
  trust.

## Alternatives considered

- **Two lists, or per-agent sections in Settings.** Rejected by the user: the
  list is one concept, "what my coding agents may do here".
- **Keep Codex out of bare `Bash`, and document the difference.** Rejected: a
  line that reads the same and means two things is the defect itself.
- **Grant a script by basename.** Rejected again, for ADR 0564's reasons.
- **Keep the card's own derivation and test it against the engine's.**
  Rejected: a parity test catches drift after the fact. Stamping removes it.
