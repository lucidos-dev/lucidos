# 0272: A coding-agent spawn always carries input: recovery sends the continuation, and empty input is refused

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

ADR 0004 gave `SpawnArgs` a `continuation` flag. It rested on one claim:
`claude --print --resume` injects "Continue from where you left off." before it
reads stdin, so a resume with no input picks up on its own. Codex needed the
same prompt synthesized in its drivers.

That claim is false for current Claude Code. Print mode queues the continuation
only when `CLAUDE_CODE_RESUME_INTERRUPTED_TURN` is set. Claude Code's own
supervisors set that internal variable, and the engine never did. A session
killed mid-tool, then resumed with no input, emits nothing: not even
`system/init`. It waits on stdin.

No recovery path was hit. Every auto-resume goes through `ContinuationRequested`,
and the spawn consumer already sends `CONTINUE_RESUME_USER_MESSAGE` as a real
input. The no-input branch was reachable only by an empty chat message to a
coding-agent thread with no live session. That parked the agent, and the thread
read as working until the 10-minute watchdog.

## Decision

A coding-agent spawn always carries text or an image. `run_direct_agent` refuses
anything else before it touches the database, a worktree or a process. Recovery
sends the continuation as an ordinary input. The no-input concept is gone:
`SpawnArgs::continuation`, the Codex drivers' synthetic prompt, the silent-resume
classification, and the warm-up handling.

## Rationale

- **The explicit prompt works on every transcript state.** An interrupted tool
  call, a cut stream and a completed API-error turn (ADR 0199) all resume from a
  plain user turn. A built-in auto-continue covers only the first two.
- **One input model for both agents.** The continuation is a forwarded input like
  any other. The ADR 0268 ledger records it, and the agent's read report settles
  it. No runtime needs a special path.
- **A refusal is loud.** The chat path turns the error into `ResponseFailed`. A
  no-input spawn failed silently, for ten minutes, and then ran a continuation
  nobody asked for.

## Consequences

- An empty message with no image to a coding-agent thread fails its turn at once.
- A stale-resume heuristic no longer needs to ask whether a message was sent. One
  always was.
- A future path that wants the agent to "just continue" must send the
  continuation text. It cannot spawn with nothing.

## Alternatives considered

- **Set `CLAUDE_CODE_RESUME_INTERRUPTED_TURN`.** It is an undocumented internal
  switch that already changed once. It also skips a completed turn, so the
  ADR 0199 resume would still stall.
- **Inject the prompt in the Claude Code driver, as the Codex drivers did.** It
  keeps a no-input spawn alive as a concept, and the driver's prompt is invisible
  to the ledger. The engine already sends the text from one place.
- **Reject an empty message at `chat_submit`.** It would also refuse chat
  threads, whose contract is not at stake. The invariant belongs to the
  coding-agent spawn.
