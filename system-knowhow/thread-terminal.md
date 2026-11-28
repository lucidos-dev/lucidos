---
name: Thread terminal
description: The thread terminal (a person's shell beside a thread, off by default) and terminal suggestions. Load before offering the user a command to run. Triggers: "run this in a terminal", "open a shell", "terminal_suggestions", "lucidos terminal suggest", "why is the Terminal switch off".
---

# Thread terminal

Every thread can have one **thread terminal**: a real shell for the user,
shown in the content pane beside the thread (ADR 0820). A coding-agent thread's
shell starts in its worktree. A chat thread's shell starts in the workspace
`data` folder.

## What you can and cannot do

- **You cannot see it or type into it.** No agent reads the screen or sends
  keys, and no byte of input or output reaches an event, the transcript or your
  context. Ask the user what a command printed.
- **You can offer a command.** A *terminal suggestion* shows the exact command
  in the thread with a **Run in terminal** button. A tap opens the terminal and
  types the command at the prompt. It never presses Enter: the user reads the
  line and runs it.
- **You cannot turn the terminal on.** The `terminal_enabled` preference is off
  by default, and only the user sets it, in Settings > System > Developer >
  Terminal. Read `terminal_enabled` in the user preferences context for the
  current state. A suggestion links to the setting while the switch is off.

The shell is not sandboxed. It runs as the user who runs Lucidos, with their
login profile, plus `lucidos` on `PATH`, `LUCIDOS_WORKSPACE` and
`LUCIDOS_THREAD_ID`. A coding-agent thread's shell also exports the profile
folders its agent reads, so a login there reaches the agent's next turn.

## Posting a suggestion

The chat agent calls the `terminal_suggestions` tool with `action="suggest"`.
A coding agent runs `lucidos terminal suggest`, documented in
`system-knowhow/lucidos-cli/subcommands-15.md`.


```bash
lucidos terminal suggest --command "git log --oneline -5" --label "Recent commits"
```

- `command` is one line of printable text. A newline, a tab, a control
  character or a character that reorders text is refused.
- `label` is optional, a few words on what the command does.
- The suggestion lands on your own thread, and posts with the switch off. The
  row then tells the user to turn Terminal on, and offers Copy.
- When a program is already running in the terminal, the tap types nothing and
  the row offers Copy instead.

Offer a suggestion when only a person at a shell can do the step: answer an
interactive prompt, sign in to a CLI, or run a command you may not run. Never
use it to push your own work through the user.

## Coding-agent sign-in

A sign-in is a terminal suggestion. A coding agent's CLI can report that an MCP
server or its account needs a sign-in. The engine then posts the CLI's own
login command, by absolute path. A typed `/mcp` or `/login` posts it too. The shell
carries the thread's account profile, so the login lands where the agent reads
it. After the user taps **Run in terminal**, their next message starts the
agent afresh, which picks the login up.

Do not invent a sign-in command of your own: point at the suggestion. With the
switch off, tell the user where it is.

## Events

`TerminalOpened`, `TerminalExited` and `TerminalCommandSuggested` are thread
events with metadata only (`system-knowhow/thread-events/thread-terminal.md`).
