# 0257: Coding agents wait on long work through engine-owned background tasks and event waits, never in-turn

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

A coding agent often has to wait for work longer than one foreground call can
hold: a test suite, an e2e run, a release build. The engine tears down the
agent's whole process group when its turn ends, so a job the agent backgrounds
itself dies with the turn.

The agents used to wait inside the turn with Claude Code's blocking `TaskOutput`
tool. Claude Code 2.1.278 removed it. The replacement notifies the agent when a
background job exits. That never reaches a Lucidos agent, because the job and
the notifier die with the turn. The engine prompt kept naming `TaskOutput`, so
agents hit "No such tool available" and improvised `kill -0` / `sleep` loops.

Waiting inside the turn has a cost of its own. Every call that returns during
the wait re-reads the agent's whole context, and after five minutes that read
is uncached.

## Decision

A coding agent hands long work to the engine with `lucidos background-task run
-- <command>`, over `/api/v1/threads/:thread_id/background-tasks`. The engine
runs it in the thread's worktree, through the registry behind the chat agent's
`run_bash_background`. It arms an *event wait* on the task's
`BackgroundBashCompleted` before the call returns. The agent then ends its
turn, and the wait re-opens the thread with the result.

Short work stays a foreground call with its timeout set to the 10-minute
maximum. `/harden` is the one exception to ending the turn. Apply reads its
next idle as a finished `/harden`, so it joins its background suites on exit
files inside the turn.

## Rationale

- **The engine is the only owner that outlives the turn.** A task it owns
  survives the agent's teardown and is killed on Discard and Archive. If the
  engine itself goes away, the boot sweep records the task as abandoned.
- **One delivery path.** The event wait was already how a chat thread hears
  about a finished task. It has a one-shot gate, a deadline, caps, a visible
  waiting indicator and a boot rebuild. The coding-agent `msg_tx` wake push it
  replaces was unreachable, since no coding agent could start a task. Keeping
  both would deliver one completion twice.
- **The wait is armed at spawn, not at the turn tail.** A quick task can finish
  before the agent ends its turn, and arming later would find nothing running.
  A task that finished first comes back inline instead.
- **Nothing waits, so nothing re-reads the context.** The agent's next request
  is the re-open itself.

## Consequences

- The idle keep-alive for background tasks (`KeepAliveForBgBash`) is gone. A
  running task no longer holds an idle agent process in memory for up to an
  hour. The re-open resumes a terminated session like any other follow-up.
- The re-open prompt cuts every long string in the delivered payload to its
  last 4000 bytes, for the chat agent as well. The event log keeps the full
  output.
- The task gets the environment the agent's own shell has, so no `CRED_*` or
  `OAUTH_*` secrets. The chat tool's environment carries them, and reusing it
  would hand a coding agent secrets it is otherwise denied.
- The route applies only the catastrophic deny-list, because the agent's own
  permission check already saw the full command line. A broad
  `Bash(lucidos:*)` grant was already a universal shell grant through
  `lucidos build-slot -- <cmd>`, so this adds no new bypass.
- A stop or a timeout kills the task's shell, not a pipeline behind it. Every
  engine exec path shares that limit, and `runtime/python.rs` records why the
  fix belongs to all four at once. **Superseded by ADR 0263**: a background
  task's stop, timeout and Discard now end its whole process group.
- A thread that hits its subscription caps gets an `unwatched` task. It is told
  to stop the task and run the command in the foreground.

## Alternatives considered

- **A `setsid` wrapper plus `lucidos events emit` and `lucidos await-event`.**
  This needs no engine change. But macOS has no `setsid` binary, and a crash
  or SIGKILL emits nothing, so the thread waits to its deadline. Discard cannot
  reach the job, and every agent has to get the recipe right. The engine
  owning the job fixes all four.
- **Keeping the agent process alive while its background jobs run**, so Claude
  Code's own notification reaches it. That ties a large idle process to every
  long job. It depends on a notifier whose shape Claude Code has already changed
  once, and it leaves Codex without an answer.
- **An in-turn wait on a saved process id** (`while kill -0 $PID; do sleep …`).
  It works, and `/harden` uses its exit-file form because it must finish in one
  turn. As the general answer it pays a full context read per call and caps
  each call at 10 minutes.
