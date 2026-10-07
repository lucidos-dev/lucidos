# 0358: Claude Code sessions run with their own background tasks switched off

- **Status**: Accepted
- **Date**: 2026-10-04

## Context

The engine ends a coding-agent session's whole process group when its turn
ends. A job Claude Code runs in the background dies with it, and nothing wakes
the session when the job would have finished.

Claude Code says the opposite. Its `Bash` tool offers `run_in_background` with
"you'll be notified when it finishes". A foreground call that outruns its
timeout is also moved to the background, and the result says "You will be
notified when it completes". That tool result is the newest, most concrete text
in context, and it beats a rule far back in the system prompt.

The engine prompt forbade both. A Sonnet session still ended its turn on a dead
job three times in one thread. Twice the user had to point out that nothing was
waiting.

## Decision

Every Claude Code spawn carries `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, set
after the workspace env so a user variable cannot clear it. Instructions never
tell a Claude Code session to pass `run_in_background`.

## Rationale

The switch removes the false promise at its source, for every model:

- `run_in_background` leaves the `Bash` and `Agent` schemas. Passing it, even as
  `false`, fails validation.
- A timed-out command fails as `Command timed out after Ns`, exit 143, instead
  of moving to the background.
- An `Agent` call always blocks until its report returns.

The waits that work stay: a foreground call with the maximum timeout, and
`lucidos background-task run`, which the engine owns and which re-opens the
thread.

## Consequences

- `/harden` keeps its overlap by starting its early suites and its Codex review
  with a shell `&` and a redirect. Those jobs still die at turn end, which is
  correct, because `/harden` joins them in the same turn.
- A message the user sends while a long command runs waits until that command
  ends. Claude Code can no longer move the command aside to deliver it.
- `lucidos cc-agent-guard` keeps its background arm for a session where the
  switch is somehow off. It reads the switch and stops requiring the flag when
  the switch is on.

## Alternatives considered

- **A `cc-bash-guard` arm refusing `run_in_background`.** It cannot stop the
  move-on-timeout path, which caused two of the three failures. With the switch
  on, Claude Code rejects the parameter before any hook runs, so the arm would
  be dead code.
- **Stronger prompt wording.** The rule was already explicit and repeated. A
  tool result contradicting it wins, so more wording would not fix it.
- **A stronger model.** It would slip less often, but the harness would still
  tell every model something false.
