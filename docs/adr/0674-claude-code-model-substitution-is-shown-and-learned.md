# 0674: A Claude Code model substitution shows as a step, and the refused curated row leaves the picker

- **Status**: Accepted
- **Date**: 2026-11-13

## Context

[ADR 0488](0488-claude-code-offered-list-adds-curated-rows.md) offers each curated row that Claude Code did not list. It expected a
row the account cannot run to fail its first turn, loudly. Claude Code does not
fail. Asked for a model the account may not use, it runs another one:

```text
{"type":"system","subtype":"init","model":"claude-opus-5-5", ...}
{"type":"system","subtype":"informational","level":"warning",
 "content":"Model \"claude-fable-5-2\" is restricted by your organization's settings. Using claude-opus-5-5 instead."}
```

Lucidos dropped the warning and took the init model as the thread's model. The
pick was ignored in silence. Claude Code's `initialize` reply omits such a model
but gives no reason, so the probe cannot tell a lagging release from a refusal.
Only a session's init line can.

## Decision

A *model substitution* is a Claude Code turn that asked for a pinned model id
and whose first init line reports another model. The engine records it as a
`CodingAgentModelSubstituted { requested, served, coding_agent }` thread event,
which the thread shows as one step. The engine learns the refusal, and the
offered list drops a curated row Claude Code refused.

## Rationale

- **The init model is the contract.** The warning's wording is not, so the
  engine never parses it.
- **"Pinned" means the id carries a version.** The plan keyed it on a row with
  no `resolvedModel`. Claude Code's discovered rows carry a `resolvedModel` equal
  to their value for pinned ids too, so that rule could not tell them apart.
  A version-free alias (`default`, `opus`) is never a substitution: Claude Code
  resolves it by design.
- **Normalised ids compare equal.** `[1m]`, an `@` pin, and a dated echo (one id
  is the other plus `-` and 8 digits) are the same model.
- **Once per refused pick.** Only the first init of a process reports, and a
  follow-up asks for the served model, which is now the thread's model.
- **Claude Code's rows stay authoritative (ADR 0488).** A refusal never drops a
  discovered row. If Claude Code lists the model, the account may run it.

## Consequences

- ADR 0488's "fails on its first turn, loudly" is replaced: the first turn runs
  on the served model, and the thread says so. The model button shows the
  served model, which is the truth.
- Refusals live in `.lucidos/cc-refused-models.json`, apart from the probe
  cache. They survive an engine restart and a re-probe at the same Claude Code
  version. They also apply to the curated fallback, so a workspace whose probe
  fails learns too.
- **A refusal is dated by the refusing session's version**, never the cache's.
  A session on a just-updated Claude Code refuses before that version's probe
  lands, and the probe keeps it.
- **Only a session on the picker's account learns.** The probe runs under the
  workspace's live Claude Code account. A thread pinned to another account
  still shows the step, but leaves the picker alone. The refusals record their
  account, and a session on a new live account drops them. The probe cache
  records its account too, so a new live account also re-probes.
- **A refused model comes back on its own** in two cases: Claude Code lists it
  (discovered rows are always offered), or a probe finds another Claude Code
  version than the one that refused. A version either side could not read never
  counts as the same one, so the next probe drops that refusal.
- **Known gap:** access granted while Claude Code neither lists the model nor
  updates. The row stays hidden until the next version change. Deleting the
  refusals file and restarting the engine clears it by hand.
- Codex is out of scope. The event carries `coding_agent` so it can reuse it.

## Alternatives considered

- **Parse the `informational` warning.** Its text is not a contract, and an
  account that substitutes without the warning would go unseen.
- **Fail the turn on a substitution.** The user gets no answer, though Claude
  Code produced one on a capable model.
- **Remove the curated row from the file.** Other accounts can run it.
- **Probe every curated row up front.** One cold session per model is slow and
  bills a turn per row.
- **Show the step but keep offering the row.** The user would pick it again and
  meet the same step every new thread.
