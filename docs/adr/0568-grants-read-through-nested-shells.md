# 0568: A grant reads through every shell layer, and a shell or exec wrapper is never a grant

- **Status**: Accepted
- **Date**: 2026-10-24

Narrows [ADR 0564](0564-codex-grants-share-bash-lines-and-trusted-repo-scripts.md).

## Context

A Codex card carried `/bin/zsh -lc "bash -c '…'"`. The grant lane unwrapped
one shell layer, so the head read as `bash`, and "Always allow" wrote
`Bash(bash:*)`. That line covered almost any later `bash -c '<anything>'` on
the engine's gate. Claude Code reads the same file as `--allowedTools` and
prefix-matches it, so it skipped its own card too. Exec wrappers (`timeout`,
`xargs`) were already refused by name. Shells were not.

## Decision

1. **The grant lane reads through every shell `-c` layer**, up to four deep,
   on both the derive side and the match side. A double-wrapped `git push`
   derives `Bash(git:*)`, and that grant covers it. A fast-path refusal in any
   layer still refuses.
2. **A head that runs its arguments grants nothing by name**: a shell, an
   exec wrapper, a prefix word (`sudo`, `env`, `exec`) or `eval`, `source`
   and `.`. `GRANTS_NOTHING_HEADS` names the set, matched after
   normalizing (`\bash`, `/bin/bash`). No card derives such a line, the
   engine's gate never matches one, and `--allowedTools` leaves one out with a
   log line. The card reads the generated copy of the set.
3. **The preamble an unwrap drops still counts.** A redirect before a wrapped
   shell refuses the grant. A privilege word in any layer refuses the attended
   gate's auto-allow. A grant still covers `sudo ls`, as before.
4. **An attended coding-agent command that cards logs why**: the static
   verdict, a refused shape, the judge's lane and summary, or a judge that is
   off or failed.

## Rationale

A grant names a head, so it can only stand for a command whose head is what
runs. A shell's head is never what runs: its arguments are. Reading through
the layers keeps the grant useful for the command the user saw. The user
requires that Codex not re-ask about routine work on a technicality.

## Consequences

- An existing `Bash(bash:*)` line becomes inert for both agents. Settings
  says so.
- A line deeper than four layers, or one whose unwrap is declined (a
  code-injecting `VAR=` before the shell), keeps a shell head and cards.
- The static classifier keeps its single unwrap. A nested shell still reaches
  the judge as an unknown head.
- Repo scripts stay ungrantable (ADR 0564). The logged reasons decide whether
  to revisit that.

## Alternatives considered

- **Refuse shell heads only, without reading through.** Rejected: a
  double-wrapped `git push` would then offer only "Allow once", and the user
  ruled out re-asking about routine work on a technicality.
- **Unwrap without a bound.** Rejected: a bound costs nothing real and keeps a
  hostile line from spinning the parser.
- **Rewrite the user's grant file.** Rejected: the spawn filter makes the line
  inert, and the file stays the user's to edit.
