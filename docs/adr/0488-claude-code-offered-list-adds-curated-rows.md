# 0488: The Claude Code offered list adds the curated rows Claude Code did not list

- **Status**: Accepted
- **Date**: 2026-10-15

## Context

ADR 0325 made the Claude Code picker exactly the list that Claude Code's
`initialize` reply names, and accepted one cost: a model Claude Code does not
list is not offered, even one it would accept.

That cost became a bug. Claude Code 2.1.300 listed seven rows and omitted
`claude-opus-5-5[1m]`, which it accepts through `--model`. The `run_coding_agent`
tool refused that id, so a caller could not pick a model the user could run.

The same list gated a second surface. `lucidos spawn-thread` reaches the
`chat_submit` route, which never checked `cc_model`, so the CLI ran any id the
tool would refuse.

## Decision

The offered list is Claude Code's own rows first, then each offered curated row
that Claude Code did not list. A discovered row wins on its value, so its label
and its `resolvedModel` stay Claude Code's.

A curated row with `offered: false` is overlay only. It keeps its
`context_window` for threads already on it, and is never listed or accepted for
a new spawn. It also drops a discovered row with the same value, so a withdrawn
model stays withdrawn whatever Claude Code lists. Opus 5 and the Fable rows
carry it.

The `chat_submit` route checks `cc_model` for a new coding-agent thread with the
same function the tool uses. Both refuse with the same text.

## Rationale

- **One list.** The tool, the route and the picker all read
  `runtime::coding_agent_model_options`. No surface keeps a list of its own.
- **Claude Code's rows stay authoritative.** The merge only adds ids Claude Code
  left out. It never changes a row Claude Code reported.
- **An alias still resolves from Claude Code.** A curated row carries no
  `resolvedModel`, so `default`, `opus` and `sonnet` show "runs X" only from a
  discovered row. No resolution is written in this repository.
- **The tool and the CLI agree.** One check, in one place, gives one refusal.

## Consequences

- The picker shows the curated rows that Claude Code did not list, beside its own.
  The hand-kept file is again a source for new spawns, not only a fallback.
  A release that Claude Code omits is offered once its row is added here.
- Claude Code omits a model for two reasons: it lags a release, or the account
  or provider cannot run it. The merge cannot tell them apart. A curated row the
  provider cannot run is offered, and a spawn that picks it fails on its first
  turn, loudly (ADR 0487). Setting `offered: false` removes such a row.
- A model dropped from both lists is refused for a new spawn. A thread already
  on it keeps running, because a follow-up is not checked.
- A cross-workspace sender still passes a Claude Code model through unchecked.
  The target's route now checks it against the target's list and answers 400.
- `lucidos spawn-thread` prints the engine's refusal. The CLI holds no model list.

## Alternatives considered

- **Keep discovered-only (0325).** It refuses a model Claude Code accepts, which
  is the incident.
- **Accept any id and let Claude Code fail.** The spawn then succeeds and the
  thread dies on its first turn, with a link and no reason.
- **Check the model in the CLI.** The CLI cannot see a workspace's discovered
  list, so it would hold a second list that drifts.
- **A tool-schema enum for `model`.** It bills every row on every chat request
  and cannot track a per-workspace list.
