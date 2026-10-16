# 0100: run_bash waits for the shell to exit, not for its pipes to close, and reports a surviving detached process instead of a timeout

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

An agent launched a diagnostic script in the background and was told
`command timed out after 60s`. The launch had in fact succeeded, and the script
wrote its output to disk while the agent went looking for a bug elsewhere.

`execute_bash_tool` waited on `child.wait_with_output()`. That returns when the
stdout and stderr pipes reach EOF, which is not the same as the shell exiting.
`&` binds looser than `&&` in a POSIX shell, so a command of the shape
`cd x && run_thing > log 2>&1 & echo launched` backgrounds the whole chain as
one subshell. That subshell inherits the tool's pipe write ends and holds them
until the inner process exits. The inner process having redirected its own
output to a file changes nothing.

Measured on the shape that failed: the backgrounded chain blocks the reader
5.02s, and the same job brace-isolated returns in 0.02s.

Two things went wrong. The tool reported failure for work that had succeeded.
And `kill_on_drop` SIGKILLs the direct shell only, so the detached process
survived, unowned and invisible to the engine.

## Decision

`wait_for_shell` waits on `child.wait()` with both pipes drained concurrently
into shared buffers. Three outcomes replace the old two:

- **Completed**: the shell exited and both pipes closed.
- **Detached**: the shell exited, but the pipes went quiet while still open.
  The tool returns the drained output and the shell's real exit status, plus a
  note naming the detached process and pointing at `run_bash_background`.
- **TimedOut**: the shell itself never exited. Unchanged behavior, including
  the SIGKILL on drop.

Quiet means no bytes and no EOF for 500 ms, capped at 5 s overall, AND a
canary task finishing in that same window. Progress resets the window, so a
reader working through a backlog is never mistaken for one sitting on an idle
pipe. The canary covers the other direction: a starved reader reads as an idle
pipe, and acting on that discards a successful command's output.

The readers are never aborted. Once the caller has its snapshot they keep
reading and discard, which is what holds the read ends open.

We do **not** kill the process group.

## Rationale

- The three states are physically distinct and the old return type could not
  express the middle one. Making it a variant is what stops the two collapsing
  again.
- Reporting a success as a failure is the expensive direction. An agent told
  its command failed retries it, or hunts a bug that is not there, and that is
  what this incident cost.
- Concurrent draining is not tidiness. `child.wait()` on its own deadlocks
  against a child that fills a pipe buffer, which is the trap
  `wait_with_output()` had been covering for us.
- The buffers are shared rather than owned by the reader tasks. Aborting a task
  that owns its buffer throws away every byte already collected, which is the
  output the caller most needs in the detached case.
- **Not aborting the readers is a correctness requirement, not thrift.** The
  reader owns the pipe handle, so dropping the task closes the read end. The
  detached process would take `EPIPE` or `SIGPIPE` on its next write, so the
  tool would kill the survivor it had just reported. Reading on and discarding
  is what makes the "we do not kill it" decision true.
- **The window measures quiet, not elapsed time.** A flat deadline
  misclassifies a reader still draining a backlog. A first attempt at 250 ms
  flat did exactly that under a parallel test run.
- **Silence is only evidence when the readers ran.** Under a saturated runtime
  a starved reader produces no bytes and looks exactly like an idle pipe. Since
  the detached path snapshots and then discards, believing that would throw
  away output the OS had already buffered for a command that succeeded. A
  canary task spawned alongside the window separates the two cases for the cost
  of one empty future.

## Consequences

- A detached launch now returns in milliseconds with its output, instead of
  costing the full timeout and returning nothing.
- Output written by a detached process after the grace is lost. It was lost
  before too, and the note says so rather than implying the window was
  complete.
- `run_bash` grows a third result shape that callers read as prose. No schema
  or event change, so nothing downstream needs to learn it.
- The engine still spawns processes it does not reap. That is unchanged, and
  now at least it says so.
- A reader task outlives the call when a detached process holds its pipe. It
  ends at EOF, so its lifetime is that process's, and it costs one 8 KB buffer.
- A runtime saturated for the whole 5 s cap still snapshots early and discards
  the rest. The cap has to exist, so this residue is the price of bounding the
  wait. The canary removes every shorter hiccup.
- **Detached can false-positive under a concurrent-spawn race.** Two spawns can
  cross-inherit a pipe write end through the parent's `dup2` window. An
  unrelated process then delays EOF and an ordinary command picks up the note.
  The output and exit status stay exact. Before this change the same race
  produced a spurious 60-second timeout that lost the output, so it is strictly
  improved, not introduced. The two tests covering ordinary commands therefore
  assert bytes and status rather than the variant.

## Alternatives considered

- **Kill the process group on timeout.** Rejected: detaching is sometimes
  deliberate, and a group kill reaches processes the caller meant to keep
  running. Reporting the survivor gives the caller the information without
  taking the decision away.
- **Keep `wait_with_output()` and only reword the timeout message.** Rejected:
  the message cannot tell the truth, because that call never learns whether the
  shell exited. The distinction has to be measured, not guessed.
- **Return an error for the detached case.** Rejected: the shell succeeded, and
  an error invites the retry this ADR exists to prevent.
- **Refuse a command that ends in `&` or contains `nohup`.** Rejected: a
  syntactic guess about intent, and it would reject legitimate uses. The tool
  descriptions and the anti-pattern list steer instead.
- **Drop the grace and treat any still-open pipe as detached.** Rejected: it
  races the ordinary path, where the reader needs a moment to observe EOF after
  the shell exits. Every normal command would then carry the note.
- **Abort the reader tasks once the snapshot is taken.** Rejected, and it was
  the first implementation. It closes the read ends and kills the detached
  process by the back door, contradicting the decision above.
- **Serialize every spawn behind a mutex to close the inheritance race.**
  Rejected as out of scope. It is a pre-existing engine-wide property and it
  would cost concurrency on every subprocess. The race now degrades to a
  cosmetic note rather than a lost turn.
