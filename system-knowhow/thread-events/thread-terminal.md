---
name: ThreadEvent Reference, Thread terminal
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Thread terminal

The lifecycle of a thread's terminal, and the suggestions an agent posts to it
(ADR 0820). A thread has at most one terminal. A coding-agent thread's shell
starts in its worktree, a chat thread's in the workspace `data` folder.

The terminal is **off by default**. None of these lifecycle events can occur
until the user turns on the `terminal_enabled` preference.

**No event holds a byte of terminal input or output.** These rows are metadata
only. No agent reads the screen, and no agent sends keys.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `TerminalOpened` | A shell started for this thread. Carries `terminal_id: Uuid`. The `actor` is the device that opened it. A second open of the same thread joins the live shell and writes no row. | lifecycle (rare per thread) | yes | yes |
| `TerminalExited` | The shell ended. Carries the same `terminal_id`, `cause`, an optional `exit_code` and `duration_secs`. Causes: `exited` (the shell ended by itself), `killed` (a device ended it), `archived`, `discarded`, `deleted` (the thread went away), `idle-timeout` (no device watched it for too long), `disabled` (the switch went off), `engine-stopped` (the engine stopped, or the boot sweep settled an open whose engine died). | lifecycle (rare per thread) | yes | yes |
| `TerminalCommandSuggested` | An agent posted a command for the user to run. Carries `suggestion_id: Uuid`, `command: String` (one line of printable text) and an optional `label`. A tap types the command at the prompt. It never presses Enter, so the user reads it and runs it. Posting one needs no live terminal. The `actor` names the agent that posted it. | a few per thread | yes | yes |
