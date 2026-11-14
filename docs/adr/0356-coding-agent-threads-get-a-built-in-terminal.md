# 0356: A coding-agent thread gets a built-in terminal

- **Status**: Proposed
- **Date**: 2026-10-04
- **Plan**: [`docs/plans/2026-10-04-built-in-terminal.md`](../plans/2026-10-04-built-in-terminal.md), with diagrams in [`2026-10-04-built-in-terminal.viz.html`](../plans/2026-10-04-built-in-terminal.viz.html)

## Context

A tester on the desktop app asked for a quick way into a terminal from a
coding-agent thread. They wanted to try the agent's work by hand before
applying it. Another product does this with a launcher that opens an outside
app, such as iTerm2, in the context of the thread.

Lucidos has no terminal. A user can read a worktree in the Files panel and the
Diff view, and can ask the agent to run a command. Neither is interactive: no
REPL, no Ctrl-C, no full-screen program, no dev server to watch. On a phone
there is no way in at all.

The maintainer settled three things before this record was written:

1. **Direction.** A built-in terminal, not a launcher to an outside app.
2. **Shape.** An engine-side pseudo-terminal (PTY) whose working directory is
   the thread's worktree. It streams into a pane inside the thread, on every
   client: the desktop app, a browser, and the phone PWA through the gateway.
3. **Access.** Any paired device that can open the thread can open its
   terminal. The trust model is the rest of the client's.

The maintainer then answered the plan's open questions, on 2026-10-04:

4. **Off by default.** A user turns the terminal on in Settings → Developer.
   The engine refuses it while it is off, not only the UI.
5. **Ship, isolate next.** Build the terminal now. A standalone app tab can
   reach it, and that is an accepted residual risk. Serving standalone app tabs
   from their own origin is the follow-up that closes the gap for every route.
6. **A reclaimed worktree refuses to open.** The terminal never recreates one.
7. **Idle limits.** 30 minutes detached at a prompt, 12 hours detached in any
   state.

This ADR records those answers and the design choices that follow from them. It
is Proposed because the design as a whole is awaiting review.

## Decision

**Once the workspace turns the terminal on, a coding-agent thread can open one
terminal.** The engine runs the user's
login shell on a PTY, with the thread's worktree as its working directory. A
client attaches over a WebSocket, through the gateway's existing upgrade
splice. Several clients can attach to the same terminal at once.

1. **Access is the client's access.** Any paired device that can open the
   thread can open its terminal, attach to it, type into it and end it, while
   the switch is on (decision 9). There is no second credential, no per-device
   grant and no confirmation.
2. **The implication, stated as part of the decision.** The terminal is a raw
   shell, running as the OS user who runs Lucidos. It sits outside the command
   guard (ADR 0002) and outside the coding-agent permission cards. Nothing
   classifies, cards, checkpoints or records what is typed into it. Pairing a
   device now hands it an interactive shell on the host, directly.
3. **The engine owns the shell, never the socket.** A registry keyed by thread
   owns each PTY. A socket is an attachment that comes and goes. A shell with
   no attachment keeps running until it exits or times out.
4. **The terminal records who and when, never what.** `TerminalOpened` and
   `TerminalExited` are system events naming the thread, the device and the
   cause of the end. No event, row or file holds a byte of input or output.
   Scrollback lives only in engine memory, in a bounded ring.
5. **The shell starts from an allowlisted environment.** It is the user's
   login shell, so their profile runs, as in any terminal app. The engine adds
   what Lucidos tools need: `PATH` entries for `lucidos` and the bundled
   Postgres client, the `PG*` variables, `LUCIDOS_WORKSPACE` and
   `LUCIDOS_THREAD_ID`. The plan names the full allowlist, which also covers
   login basics, the ssh agent and a Linux desktop's GUI session. Nothing
   outside it crosses from the engine's own environment.
6. **Every explicit end reaches the shell's whole session.** Archive, Discard,
   delete, the idle timeout and a Kill from a client hang up the PTY. After
   `GROUP_TEARDOWN_GRACE`, every process still in the shell's session gets
   SIGKILL. The engine's teardown skips the grace, as ADR 0263 does, because it
   must finish inside `REAP_WAIT`. A shell that exits by itself signals nothing.
7. **A live terminal holds its worktree, and never removes one.** The cleanup
   worker counts it as live work. The terminal refuses to open on a thread
   with no worktree on disk, and never creates one.
8. **Revoking a device ends its open sockets.** The gateway tracks each
   upgrade it splices, by device, and closes them when that device is revoked.
   This covers voice as well.
9. **The terminal is off by default, and the engine enforces the switch.** The
   switch is `terminal_enabled`, one workspace-wide preference, default off,
   set in Settings → Developer. While it is off, the engine refuses the open
   route and the socket handshake before it spawns a PTY or upgrades a socket.
   Turning it off ends every live terminal. The Lucidos Agent cannot set it,
   as with the command guard. Hiding the UI follows the switch but is not the
   gate.
10. **Turning the terminal on is announced.** The engine raises a notification
    on every device, naming the device that turned it on.
11. **The standalone app tab is an accepted residual risk.** An app opened in
    its own browser tab is a same-origin document carrying the device's
    identity (ADR 0231). It can open a terminal, and it can turn the switch on
    first.

    The off default limits the exposure to workspaces that turn the terminal
    on, and decision 10 makes a silent flip visible. Turning the switch on and
    opening a terminal also refuse a request whose `Referer` names an app
    document, ADR 0117's rule. None of these is a barrier against a hostile app
    tab, and a short plain-language warning beside the toggle says so. Serving standalone app tabs from their own origin is the
    planned follow-up. It closes this gap for every route at once.

## Rationale

**The launcher is the proposal the philosophy lens judges, and it loses.** It
is an integration with somebody else's product: the scrollback, the history and
the session live in iTerm2, and the workspace hands the user off. It also fails
on mechanics. A phone has no iTerm2, a browser cannot launch an app on the
host, and only the macOS app could try. The built-in terminal points at our
own client, so the lens settles nothing more about it. What remains is how it
is built (Rule 3) and what it costs.

**It is built from open standards.** The PTY and the escape sequences are the
terminal's own standard (ECMA-48 and the xterm conventions). The wire is a
WebSocket carrying raw bytes plus a few JSON control messages. The emulator is
a JavaScript library behind a narrow interface: write bytes, receive keys,
resize. That makes it a library we could replace, not a runtime we live in.

**The engine is the only place a shell can serve every client.** Engines bind
loopback (ADR 0096), and the gateway is the only network path into a workspace
(ADR 0094). The gateway already splices WebSocket upgrades at the same auth
boundary as HTTP (ADR 0151). It answers the handshake's origin question itself
(ADR 0163), which keeps app frames out, since they run at an opaque origin (ADR
0227). The terminal reuses all of it, and the gateway still parses no frames.

**WebSocket, not SSE plus POST.** The argument ADR 0151 made for voice holds:
a duplex binary channel, where latency is the product. Keystroke echo through a
new POST per key would be a terminal rebuilt badly. The SSE stream also has a
different job. It carries events, one stream per workspace (ADR 0146).

**The client's access is the honest boundary, because the client already
reaches a shell.** A paired device can ask the Lucidos Agent to run bash, and
the command guard ships off. It can ask a coding agent to run whatever that
agent's permissions allow. The terminal removes the agent from between the user
and the shell, and takes the gates and the transcript with it. A second lock
would stop nobody holding a paired device, which passes every other check too.

**Recording bytes would be a liability with no reader.** Output carries
secrets: an `env` dump, a printed token, a `cat` of a key file. Input carries
passwords typed at a prompt. Thread events feed context builders, recaps and
memory extraction, so a recorded stream would need an exclusion in every reader
(the argument ADR 0318 made). The lifecycle events are the audit the access
decision needs: which device opened a shell, when, and how it ended. They are
system events, not thread events, so no context builder reads them.

**A person's shell gets a person's environment.** The agent spawn path inherits
the engine's whole environment, `DATABASE_URL` and service-level provider keys
included, which suits a process Lucidos drives. A terminal is a person's shell
on a screen, perhaps a phone screen in a café. Starting clean and letting the
login profile run gives it what the user's own terminal app would hold, plus
Lucidos's tools. ADR 0339 made the same allowlist choice for handshake scripts.
ADR 0326 is why the profile matters: a service starts without version managers.

**Job control is why the kill reaches the session, not the group.** An
interactive shell gives each job its own process group, so ADR 0263's group
kill would miss every job. Hanging up the PTY is how a terminal has always
ended a session. The kernel signals the foreground job and the shell, and the
shell passes SIGHUP to its jobs. The sweep by session id then catches whatever
ignored it. Graceful first, for ADR 0263's reasons: traps run and a test runner
closes its browsers.

**One terminal per thread keeps "open" idempotent across devices.** The desktop
and the phone open the same shell, which is what carrying on from the phone
means. Several terminals per thread can come later. Every socket already names
its terminal, so only the open route would change.

**A shell must not lose its directory.** ADR 0035 gave reclamation one owner
after a teardown deleted a tree under a live process. A terminal is a live
process in that tree, so the owner must see it.

**A socket is authenticated once.** The handshake is the only check, and the
splice then runs until a side closes. Revoking a lost phone has to end its
shell now, not when the phone next drops its connection. The gateway is the
only hop that knows which device a splice belongs to.

**Off by default, because most users never asked for a shell.** Pairing a phone
to read threads should not also hand it a shell on the host. The off default
makes that exposure a choice somebody made in this workspace.

**The engine enforces the switch, and the gateway keeps no copy.** The switch
is a preference in the workspace's database, and the engine owns every terminal
route. The gateway is the only network path (ADR 0094), so an engine refusal
binds every network caller. The gateway forwards a refused handshake verbatim
and splices nothing. A gateway copy would be a second source of truth for one
setting, free to drift from the first. When the switch turns off, the engine
ends each terminal, which closes each socket, and each splice ends with it.

**The switch cannot stop the callers that matter most, so turning it on is
announced.** Anyone holding the device's identity can flip it: a lost phone, or
an app in its own tab. A notification on every device turns a silent flip into
one the user sees.

**The app-tab residual is accepted because the tab already reaches further.**
ADR 0231 records that a standalone app tab can call any route as the device. It
can already apply a pending change or start a coding agent. The terminal makes
its reach to a shell direct, not new. A fix for the
terminal alone would leave every other route open. The fix belongs to the
app-tab origin, which closes all of them.

## Builds on, reopens or bends

| ADR | Relationship |
|---|---|
| 0002 command guard | Untouched. The guard gates the Lucidos Agent's tools and never claimed to gate a person. The terminal is a new command path, deliberately outside it. |
| 0026 a session is never owned by a request future | Applied to a new resource: the socket handler never owns the PTY. |
| 0035 one owner of worktree reclamation | Builds on it. The terminal joins the worker's live-work check and removes nothing. |
| 0094, 0096 gateway is the only network path | Builds on it. No new port, no direct engine path. |
| 0100, 0263 process-group kills | **Bends** them. A terminal end reaches the session, not just the group, because job control splits jobs into groups. ADR 0263's "explicit end reaches the tree, natural exit signals nothing" carries over unchanged. |
| 0146 one SSE stream per workspace | Untouched. Terminal bytes never ride SSE. Each open terminal view holds its own socket. |
| 0151 the gateway splices upgrades | **Bends** "the gateway holds no session state". It now keeps a map of live splices by device, so revocation can end them. It still parses no frames. |
| 0163 the gateway answers the handshake's origin | Builds on it. It is what keeps app frames and other local pages out. |
| 0169 every caller identifies itself | Builds on it, and closes a gap. The mutating gate skips GET, so it never sees a WebSocket handshake. The terminal routes require an identified device themselves. |
| 0231 app request hatch | Inherits its stated limit, as an accepted residual risk (decision 11). A standalone app tab is a same-origin top-level document, so no route classification binds it. Here that limit now reaches a shell directly. Serving app tabs from their own origin is the follow-up that closes it. |
| 0318 side questions never recorded | Reuses its argument for not recording bytes. ADR 0320 later recorded side questions, for reasons that do not apply to terminal output. |
| 0326 login-shell hydration, 0339 allowlisted env | Builds on both. |
| 0341 agents run below the engine | Applies it to the shell. It runs at the engine's nice + 5, set in the child before exec. |
| `runtime/spawn_env.rs` posix_spawn-only spawns | **Bends** the convention. A PTY needs `setsid`, a controlling terminal and reset signals in the child, which forces one `pre_exec` path. ADR 0075 explains the cost: the fork path loses `posix_spawn`. That spawn lives in one module. |
| `docs/philosophy.md` Principle 4 | **Bends** a sentence. It lists the frontend's whole runtime dependency set, and the emulator library joins it. The doc changes in the same commit as the library. |
| Engine statelessness (`CLAUDE.md`) | Untouched. A shell is a process handle, which may die on restart. Its lifecycle is in events, so a restart reads as an ended terminal. |

## Consequences

What we gain:

- A user can try an agent's work by hand, from any device, without leaving the
  thread. The shell's directory is the worktree, and its branch is the thread's.
- A phone gets a terminal into the host for the first time.
- Revoking a device now ends its voice call too, not only its terminal.
- The WebSocket identity gap (no identity check on a GET upgrade) closes for
  the terminal routes.

What we give up, knowingly:

- **A paired device is an interactive shell as the host's OS user**, once the
  workspace turns the terminal on. Losing an unlocked phone with a live pairing
  hands out a shell until the device is revoked. That was true through the
  agents before. It is now direct and unrecorded. The phone can also turn the
  switch on itself, which decision 10 announces.
- **No transcript.** What was typed and printed is gone once it leaves the
  ring. The audit is who, when and how it ended, nothing more.
- **Not shared with the agent.** The agent never sees terminal output. It sees
  outcomes: changed files and commits. A user who wants the agent to read
  something sends it to the composer.
- **The same worktree, two drivers, no lock.** A user and an agent turn can
  edit one file, contend for `.git/index.lock` or run two heavy builds. Cargo's
  build-directory lock and the host memory watch (ADR 0264) are the nets.
- **Commits made in the terminal belong to the thread's change.** Apply carries
  them like the agent's own.
- **A restart ends every terminal.** Switching to a new version, a crash or a
  stop kills the shells. The client shows the end and offers a new shell. It
  never opens one silently.
- **The standalone app tab residual (ADR 0231) now reaches a shell.** An app
  opened in its own tab can turn the switch on and open a terminal. It stays
  open until standalone app tabs get their own origin. Until then the off
  default, the notification and the warning beside the toggle are the whole
  answer.
- **One more step before first use.** A user turns the terminal on in Settings
  → Developer before any thread offers it.
- **Bundle cost.** The emulator ships as its own lazy chunk, loaded when a
  terminal first opens. The entry chunk and its budget (ADR 0353) do not move.
- **Unix only.** There is no Windows build, so the PTY layer is unix. A
  Windows build would need a ConPTY backend behind the same interface.

## Alternatives considered

**A launcher to an outside terminal app** (iTerm2, Terminal.app). The request
as asked. Rejected: it exports the user to somebody else's product. It also
works only in the macOS app, and cannot reach a phone or a remote browser.

**Ask the agent, or add a one-shot command mode to the composer.** A `!`
prefix could run a command in the worktree, through the guard, with the output
recorded in the thread. It is cheaper and auditable. Rejected as the answer to
this request: it is not interactive, so no REPL, no Ctrl-C, no editor and no
dev server. It stays compatible as a later addition.

**A second credential to open a terminal.** A per-device grant, a PIN, or a
confirmation on another device. Rejected by the maintainer's access decision. A
holder of the paired device passes it, and the shell is already reachable
through the agents.

**On by default.** The plan's first draft. Rejected by the maintainer: every
paired device would get a shell on the host before anybody chose that.

**Wait until standalone app tabs get their own origin.** Closes the residual
before it exists. Rejected by the maintainer: the origin change serves every
route, not just this one, so it should not hold the terminal back.

**Turn the terminal on only from the host.** The `lucidos` CLI or the desktop
app would carry the machine-local token, which an app tab never holds. That
makes the switch a real barrier against an app tab. Rejected: a user on a phone
or a remote browser could never turn it on, and decision 10 makes a flip
visible instead.

**A copy of the switch in the gateway.** Rejected: a second source of truth for
one setting. The engine's refusal already binds every network caller.

**Record output as thread events, filtered out of context.** Gives a transcript
on every device. Rejected: secrets in output, passwords in input, megabytes per
build, and a filter every context reader must remember.

**Keep shells alive across engine restarts**, inside `tmux` or `dtach` on the
host. A shell would survive a version switch. Rejected for now: it needs a
bundled or installed helper, a re-adoption path at boot and a second kill
model. The engine's statelessness rule already expects active processes to die.

**A server-side terminal emulator** (for example the `vt100` crate) for exact
redraws on reattach. Deferred. A raw byte ring plus a resize nudge redraws
nearly every program. The plan measures the gap before adding a second
emulator.

**`portable-pty`**, the PTY layer from WezTerm. Mature and cross-platform,
ConPTY included. Not chosen as the default. The engine already depends on
`libc`, and the spawn needs our own child steps anyway: renice, signal reset,
environment. There is no Windows build for ConPTY to serve. The plan's first
phase confirms the choice against a macOS spike.

**Terminate the WebSocket in the gateway and relay messages.** Rejected for the
reasons in ADR 0151: a frame parser in the network-facing process.

**SSE downstream plus POST upstream.** Rejected for the reasons in ADR 0151,
and because keystroke echo is the latency that matters.

**A terminal for chat threads, in the workspace root.** A different feature.
The root holds the user's live data and no thread owns a branch there. Out of
scope.
