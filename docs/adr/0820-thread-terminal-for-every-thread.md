# 0820: Every thread gets a terminal; agents hand commands to it as terminal suggestions, and coding-agent sign-in becomes one

- **Status**: Accepted
- **Date**: 2026-11-26
- **Amends**: [0356](0356-coding-agent-threads-get-a-built-in-terminal.md) (built-in terminal) and [0818](0818-coding-agent-sign-in-cards.md) (sign-in cards)
- **Design**: [`docs/plans/2026-11-26-thread-terminal-design.md`](../plans/2026-11-26-thread-terminal-design.md)
- **Plan**: [`docs/plans/2026-11-26-thread-terminal.md`](../plans/2026-11-26-thread-terminal.md)

## Context

Coding-agent sign-in ran the CLI's login in a hidden PTY and waited for one
known line. Claude Code instead showed a full-screen dialog asking to approve
an organization's managed settings. Nobody could see it, so the wait timed out
with a misleading message. Any new interactive screen hits the same dead end.

The first answer was a relay card: a sandboxed fixed command streamed into the
transcript. With no agent reading it, that card is a terminal limited to one
command. ADR 0356 already designed a full terminal, never built.

## Decision

1. **Every thread gets one thread terminal**, not only coding-agent threads.
   A coding-agent thread's shell starts in its worktree, a chat thread's in the
   workspace's `data/` folder. It opens in the content pane.
2. **The shell is not sandboxed**, and no agent reads its screen or sends keys.
3. **A coding-agent thread's shell exports the agent's profile location**
   (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), resolved by the same code as the agent
   spawn. Folder paths only, never a credential.
4. **`TerminalOpened` and `TerminalExited` are thread events** with metadata
   only. This replaces 0356 decision 4's system events.
5. **Any agent may post a terminal suggestion**: a one-line command with a
   **Run in terminal** button. The engine types it into the shell and never
   presses Enter. It refuses a newline, control characters and a busy shell.
6. **Coding-agent sign-in is a terminal suggestion.** The hidden helper, its
   line matching, the paste field and the sign-in form request retire. While
   the Terminal switch is off, the suggestion says to turn it on.

Everything else in 0356 stands: the off-by-default switch, access for any
paired device, the WebSocket splice, the environment allowlist, the idle
limits and the app-tab residual.

## Rationale

**One generic surface beats a parser per CLI screen.** A real terminal shows
whatever the CLI shows, so a new dialog costs nothing.

**The owner uses a terminal to leave the sandbox.** A sandboxed relay would
fail at that, and add a second spawn path for a narrower job.

**No agent reads it, so no screen text reaches a model or the transcript.**
Terminal output can hold tokens and codes a CLI echoes.

**Thread events let the agent carry on.** The context sees that a terminal
opened and how it ended, never what it showed.

**The profile export is what makes sign-in land.** On Linux the sandboxed agent
keeps Claude Code's config under `CLAUDE_CONFIG_DIR`, and a thread can pin its
own. A login in a plain shell would land where the agent never looks.

**Enter stays with the user.** Any agent may propose any command, a
prompt-injected one included. A typed line with no newline cannot run until a
person reads it and presses Enter.

## Consequences

- A sign-in, a first-run dialog or any other CLI screen is answerable from any
  device, the phone included, once Terminal is on.
- A workspace with Terminal off cannot sign a coding agent in from Lucidos.
  Agents explain how to turn it on.
- Chat threads get a host shell too, so the app-tab residual reaches them.
- Recorded `AgentSignInRequested` events stay readable as ended rows.

## Alternatives considered

**A relay card in the transcript running a fixed sandboxed command.** Rejected:
a narrower terminal, a second spawn path and a sandbox the owner wants out of.

**An agent that reads the screen and sends keys.** Rejected by the owner:
screen text would enter model context and the transcript.

**Keep the hidden sign-in flow while Terminal is off.** Rejected by the owner:
one path, no special sign-in card.

**Suggestions only for Lucidos's own commands.** Rejected by the owner: any
agent may suggest, and the missing Enter is the guard.

**SSE down and POST up.** Rejected for 0356's reasons: echo latency, and SSE
carries events.
