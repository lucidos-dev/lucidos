---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

<!-- domain-event: BuildSlotAcquired: The CLI emits this domain event when a contended host build acquires a slot. -->
<!-- domain-event: BuildSlotWaiting: The CLI emits this domain event when a host build must wait for a slot. -->
<!-- domain-event: BuildSlotReleased: The CLI emits this domain event when a host build slot is released. -->
<!-- domain-event: BuildSlotStepCompleted: The CLI emits this domain event after a wrapped command exits under an inherited slot. -->

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos background-task run [--poll] [--description <T>] [--timeout-secs <N>] -- <command>` / `wait <task_id> [--max-secs <N>]` / `output <task_id>` / `stop <task_id>`

Run work longer than one foreground call. The engine runs the command as a
*background task* in this thread's worktree. Pick the wait by how long the work
should take:

- **About 3 minutes or less:** no background task. Run it as one foreground
  call, with an explicit `timeout: 240000` in Claude Code.
- **Longer, the usual case:** `run --poll`, then `wait <task_id>` in the
  foreground. Each `wait` blocks until the task finishes or 230 s pass, then
  prints the new output. Re-issue it until it reports `finished`. No event
  wait is armed while your turn runs.

  If your turn ends first, the engine arms
  one, and re-opens this thread when the task finishes. The same holds for a
  task that finished before you read its result. A user Stop arms nothing.
- **An open-ended wait**, such as a run queued behind a held lock: `run`
  without `--poll`, then **end your turn**. The
  engine arms an event wait on the task's `BackgroundBashCompleted` before
  `run` returns. When the task finishes, it re-opens this thread with the exit
  status and the tail of its output.

The 230 s cap is the point of polling. A Claude Code prompt cache lives 5
minutes from the last request. After a longer gap, the next request pays to
re-read the whole context. A poll keeps every gap inside that window. Either
way the engine kills a task still running after its `--timeout-secs`: an hour
unless you ask for more, 4 h at most.

A command you background yourself (`&`, `nohup`) dies when your turn ends,
because the engine stops every process you started. Claude Code's own
background mode is off in a Lucidos session, so a foreground call that outruns
its timeout is killed.

- **One argument after `--` runs as written**, so quote a pipeline or a
  redirect as a single string. Several arguments run as exactly those words.
- **Always pass `--description`**: what the task is, as a short noun phrase
  in the user's language (`"the nightly e2e sweep"`). The thread's waiting row
  reads `Waiting for <description> to finish`, which tells the user what the
  thread waits on. Without it the row shows the command line.
- `--timeout-secs` kills a task still running after that long
  (default 3600, maximum 14400). Pass it for a run that may outlast the hour,
  such as a long unattended job.
- `run` prints a `status`. `polled` means call `wait` now, in this turn. `watched` means end
  your turn now. `unwatched` means the thread hit a subscription limit and
  nothing will wake it: stop the task and poll it instead. `finished` means it
  was quick, and the output is already in the response.
- `wait` prints what `output` prints. While the task runs, that is the new
  output plus a `message` saying to wait again. Once it has finished, it is the
  final record, with `exit_code` and `status`. A `wait` that runs out of time
  never stops the task. `--max-secs` returns sooner, and the cap is 230.
- **A child's report ends a `wait` early.** The answer then carries the report
  under `child_reports`, and its `message` says to read it and wait again. See
  [`lucidos threads child-reports`](subcommands-3.md#lucidos-threads-child-reports).
- The re-open message carries each output stream's last 4000 bytes. `output
  <task_id>` prints more: what arrived since your last read while the task
  runs, and its final record once it has finished.
- Only this thread's own agent can start, read or stop its tasks. The engine
  refuses a command on the catastrophic deny-list.
- The task gets the same environment as your own shell, so no `CRED_*` or
  `OAUTH_*` secrets. Reach an external API through `lucidos proxy`.
- Discarding or archiving the thread kills its running tasks. Stopping a task
  records its completion as killed, and that still re-opens the thread.
- **The exit status is the command's own.** Do not end a command with
  `; echo $? > file`: the `echo` succeeds, so the task reports exit 0 even when
  the work failed. To keep a copy in a file as well, pass the status on:
  `…; rc=$?; echo $rc > .lucidos/run.exit; exit $rc`.

**Stopping your own task does not re-open your thread.** `stop` stands down
this thread's waits on the task before it signals it, so the killed completion
wakes nobody here (ADR 0369). If one of your own waits watched more than this
task, `stop` ends it whole and names it, so you can re-arm the rest.

**A stop, a timeout or a Discard ends the task's whole process group.** The
engine sends SIGTERM, waits 3 s so a trap can clean up, then sends SIGKILL. A
process that detached into its own session is out of reach. A task that exits
on its own signals nothing, so a `nohup … &` it started keeps running. The chat
agent's `run_bash_background` behaves the same way.

```bash
# The usual case: start the suite, then wait in the foreground.
$ lucidos background-task run --poll --description "the engine test suite" \
    -- 'lucidos build-slot -- cargo test > .lucidos/test.log 2>&1'
$ lucidos background-task wait 5f0c2e1a-…   # re-issue until it reports finished

# An open-ended wait: start it watched, then end your turn.
$ lucidos background-task run --description "the nightly e2e sweep" \
    -- './scripts/e2e.sh > .lucidos/e2e.log 2>&1'

# Look in on it, or give up on it.
$ lucidos background-task output 5f0c2e1a-…
$ lucidos background-task stop 5f0c2e1a-…
```

### `lucidos confinement-tests run [--hold-secs <N>]`

Ask the engine to run the *confinement tests* outside the agent sandbox. A
seatbelt never nests on macOS, so a sandboxed coding-agent session cannot run
them in place. `./scripts/confinement-tests.sh` calls this for you when its
seatbelt trial fails. See `system-knowhow/agent-sandbox.md` § Confinement tests.

The engine runs a fixed script in this thread's worktree, as a background
task. The command takes no command text. Only this thread's own agent can call
it, and an engine that itself runs in a sandbox refuses.

The run needs the owner's `run-confinement-tests` permission. With none, the
engine raises a card: *Allow once*, *Always allow* or *Don't allow*. This call then
holds its tool call open until the owner answers. **Run it in the foreground**
with `timeout: 240000`: any other step you take meanwhile closes the card.

- It prints the task id first, then follows the run and prints its output.
  The exit status is the tests' own.
- `--hold-secs` sets how long to hold the card open (default 200). `0` raises
  no card, and exits 75 when no permission and no passing run covers it.
  `/harden` uses `0`.
- **Exit 75 means nothing ran**: no answer in time, the card closed, a Don't allow,
  or no permission under `--hold-secs 0`. It is never a pass. The script also
  exits 75 when the sandbox cannot apply on the host at all.
- A thread with no live coding-agent session is refused with no card.
- **A passing run certifies a retry.** `--hold-secs 0` exits 0 with state
  `passed` when this thread's latest run passed at the clean tree's commit.
  It checks that before any grant. Commit before you run, since a
  dirty-tree run cannot certify (the start message says so). A clean run
  fails if the tree leaves its commit before the run ends, so do not edit or
  commit while it runs.
- If the call ends before the run does, follow it with
  `lucidos background-task wait <task_id>`.
- *Allow once* covers one run, and the pass above covers its commit's retry.
  *Always allow* covers every later run in this workspace until the owner
  revokes it (`lucidos permissions list`). Only the owner can pick it.

```bash
$ lucidos confinement-tests run     # in the foreground, timeout: 240000
```

### `lucidos build-slot [--label <T>] [--max-wait <SECS>] -- <command>` / `--status` / `--set-capacity <N>`

Run a heavy build under a *build slot*, so parallel *worktrees* cannot pile N
full compiles onto one host, OOM it, or bury its cores.

**Wrap anything heavy** that a coding-agent session runs: `cargo build`,
`cargo test`, a Gradle or Xcode build, a large bundler run. The slot is taken
before the command starts and freed when it exits, or when this process dies.
Do NOT wrap cheap work (a type-check, a unit-test run of a small package):
it would sit in a slot for minutes to save seconds.

A background task excludes time spent queued for a build slot from its run budget.
Its wait shows the queue until a slot becomes available, then shows the run deadline.
The wait is bounded by 24 hours.

On Linux, the default slot count uses the smaller of host RAM and the visible
cgroup v2 memory limits. A capacity override takes precedence.

**A granted slot also shapes the build.** It runs at `nice +10`, which the
whole compile tree inherits, and gets a share of the cores as
`CARGO_BUILD_JOBS`. The share is the host's cores divided by the slots held
right now. It never falls below the fixed `cores / capacity` share, and never
below 1. So a solo build keeps the machine, and contention divides it.

```bash
# The normal shape. Blocks until a slot frees, then runs the build.
$ lucidos build-slot -- cargo test --release
lucidos build-slot: slot 0 of 3, nice +10, 18 cores

# Name it for the listing, when the program name is not the useful label.
$ lucidos build-slot --label "integration suite" -- ./gradlew test

# Who is building right now, and where the count came from.
$ lucidos build-slot --status
build slots: 1/3 held, capacity from host RAM
pool: /Users/me/.lucidos/build-slots
  slot 0  HELD  cargo  pid 41231  2m14s  /path/to/worktree
  slot 1  free
  slot 2  free

# Set the count for this machine. Not per workspace: the pool spans them.
$ lucidos build-slot --set-capacity 2
```

**The default label is the program name only**, such as `cargo` or
`deploy.sh`, never its arguments. The label lands in the slot listing and on
the `BuildSlot*` events, so a token on the command line stays out of both. Do
not put a secret in `--label` either.

**In the Lucidos repo you do not need this.** `make lint`, `make test` and the
build scripts already take a slot, so type the ordinary command. Use the
wrapper in any OTHER repo.

**It waits, it does not fail.** The second build is wanted, just not at the
same time, so it blocks with no deadline and prints progress.
`--max-wait <secs>` opts into a deadline and exits **75** when it passes. If
you set one and hit it, do not retry on a timer: subscribe and end your turn.

```bash
$ lucidos await-event --on BuildSlotReleased --timeout-secs 3600 \
    --reason "a build slot before running the test suite"
```

- **Nesting is safe.** A wrapped command that wraps again runs straight
  through, so a script you call cannot deadlock against the slot you hold. It
  changes nothing either: the outer slot's priority and core share stand, and
  are not applied a second time.
- **A `CARGO_BUILD_JOBS` you set yourself always wins**, and nothing is
  exported over it.
- **`LUCIDOS_BUILD_SLOT_NICE` overrides the increment**, and `0` opts out
  entirely, which suits a foreground build you are waiting on. A non-root
  process cannot lower a nice increment again, so choose before the build
  starts.
- **The Apply build goes first.** The engine's own background rebuild waits
  as a *priority waiter*: while it waits, your build leaves a freed slot to
  it, and `--status` says so. It never takes a slot beyond the count, and it
  never stops a running build. Automatic main repairs yield to chosen work as background builds, including commands started through `lucidos background-task run`. Do not set `LUCIDOS_BUILD_SLOT_CLASS`
  yourself: it exists for the build the user is watching.
- **It never blocks a build it cannot govern.** With no `lucidos` binary, no
  writable pool, or no engine to announce to, the command just runs. A host
  that will not report its core count gets no share, and the build runs at
  cargo's own default.
- **The exit code is the command's**, and a signalled command reports
  `128 + signal`, so a killed build never reads as a pass.
- **Every release attempts a best-effort post.** `BuildSlotReleased` is
  posted after the slot frees, including when a session has already given up on
  `--max-wait` and subscribed. `BuildSlotWaiting` and `BuildSlotAcquired` are
  posted only under contention. The three events share a `request_id` and a
  `class`: the slot class the acquisition resolved (`priority`, `ordinary` or
  `background`). Each release includes `wait_ms`, `hold_ms`, `outcome` and
  `exit_code`.
- **Nested steps have their own timing.** `BuildSlotStepCompleted` carries
  `duration_ms`, `exit_code`, and the inherited `request_id` and `class` if
  available. The class is the one the enclosing hold was granted, so a step
  proves its class without joining another event.
  A command that cannot start still posts one, with a `null` exit code and
  outcome `unknown`. This post happens after the command exits. Each best-effort post has a
  five-second HTTP timeout and preserves the command's exit code.

### Hidden: `lucidos build-slot --lease`

The launch build wrapper uses this mode to hold one slot across its full body.
It accepts `--label` and `--max-wait`. After acquiring a slot, it prints
`granted <index> <jobs> <nice>` to stdout, with `-` when the grant supplies no
job count. Launch wrappers that set `LUCIDOS_BUILD_SLOT_LEASE_RECORDS` also
receive the request ID and the granted class as fifth and sixth fields.
They pass these to nested steps through `LUCIDOS_BUILD_SLOT_REQUEST_ID` and
`LUCIDOS_BUILD_SLOT_GRANTED_CLASS`. The launch wrapper sends
`release <exit_code>` to report its body's exit status. EOF or owner death
leaves the status unknown.

The helper holds until it reads a line or EOF on stdin, or detects
that its parent shell died. The wrapper passes the original shell PID in
`LUCIDOS_BUILD_SLOT_LEASE_OWNER` so an orphaned helper exits before acquiring.
Without a usable pool or with an inherited slot, it prints `unleased`.
A wait timeout prints `error` and exits 75.

### Hidden: `lucidos cli-surface --out <DIR>`

Write the CLI's command tree to `cli-surface.json` in the output directory.
Codegen uses the built CLI when its dep-info passes validation. The ignored
CLI writer test provides the same JSON when the built CLI is unavailable.

### `lucidos ps [options]`

A `ps` for macOS inside the *agent sandbox*, where `/bin/ps` is setuid and
refused. The engine puts a shim that runs this command first on a sandboxed
agent's `PATH`, so type plain `ps`. It needs no workspace and reads the
kernel's process table directly.

```bash
$ lucidos ps aux
$ lucidos ps -o pid,ppid,pgid,uid,lstart,comm,command -p 4021,4022
$ lucidos ps -U me -o pid,%cpu,rss,command   # a `top` substitute
```

- **Options:** `aux`, `ax`, `-A -a -e -x`, `-p <pids>`, `-U`/`-u <users>`, `-f`,
  `-j`, `-w`, `-o <field>[=<header>]`. Selection follows `/bin/ps`: `-u`
  matches the effective user and `-U` the real one.
- **No `-E`.** Another process's environment can hold credentials, so the
  stand-in refuses to print it.
- **Fields:** `pid ppid pgid uid user tty tt lstart start stime stat comm
  command args rss vsz etime time %cpu %mem nice pri c sess jobc`. For the
  fields scripts use, the output matches `/bin/ps` byte for byte.
- **It never guesses.** The kernel withholds some fields from an unprivileged
  reader. Another user's or a setuid process's command prints as `(comm)`.
  `rss`, `vsz` and CPU time print as `0`, and the first `stat` letter as `?`.
  `%cpu` is the average over the process lifetime.
- **Anything else exits 2**, including an unknown option or field.
- **Not macOS:** exits with a refusal. Use the system `ps`.

### `lucidos event-waits pending-cards --wait-id <ID> --after <CURSOR>`

```sh
lucidos await-event --on SubThreadCardOpened \
  --condition "{\"ancestor_thread_id\":\"$LUCIDOS_THREAD_ID\"}" \
  --timeout-secs 3600 --reason "a question or permission request in a sub-thread"
```

Read `pending_cards.cards` in the registration response. Those openings
already exist and are reported without spending the wait. If the page has
`next_after`, pass that value and its `wait_id` to continue:

```sh
lucidos event-waits pending-cards --wait-id <wait-id> --after <next-after>
```

Repeat with the returned `next_after` until it is absent. The saved conditions
and watermark also apply after the wait has delivered. Cards that resolve
between pages drop out. Follow `source_thread_id` and `source_event_id` to
the original card.

### `lucidos event-waits list` / `lucidos event-waits cancel [--wait-id <ID>] [--on <EVENT_TYPE>] [--all]`

Read and stop the **calling thread's** own subscriptions, the ones
`lucidos await-event` armed. They are the coding-agent counterparts of the chat
agent's `list_event_waits` and `cancel_event_wait` tools, on the same code, so
both get the same report and refusals. Like `await-event`, both take the thread
from `$LUCIDOS_THREAD_ID` and have no thread flag, so neither can reach another
thread's subscriptions.

**`list` is the only way to answer "am I still watching for that?"** Nothing
tells you when a subscription ends. A delivery re-opens the thread, but a
timeout or a user pressing **Stop waiting** lands while your session is not
running. A subscription is *spent* the moment it fires. Answering from memory
is a guess. Run `list` before saying you are still watching, before
re-subscribing (a duplicate is refused), and to get the id `cancel` takes.

Each entry carries the subscription's id, the events and conditions it watches,
its `--reason`, when it was armed, and when it times out. Both ages are
spelled out:

```bash
$ lucidos event-waits list
{"count":1,"event_waits":[{"wait_id":"3f2b…","subscription":"ChangeProposed",
  "reason":"the refactor to propose its change",
  "armed_at":"2026-08-07T09:14:22Z","armed_ago":"7m",
  "expires_at":"2026-08-07T09:44:22Z","expires_in":"22m"}]}
```

**`cancel` is how you stop watching.** A live subscription re-opens this thread
later whatever you told the user. So when they say stop, drop it, or never
mind, run this rather than promising. Use it too when the thing already
happened, or when a new subscription supersedes an old one.

Pass exactly one of `--wait-id <ID>` (from `list`), `--on <EVENT_TYPE>`, or
`--all`. None is the default: a bare call would have to guess between one and
all, and both guesses are wrong. Stopping is silent: the subscription ends, the
user sees it leave the waiting indicator, and the transcript records the stop.

**Reach for `--on` when the answer arrived some other way.** It is the safe
middle: it needs no id from `list`, and it leaves every other watch on this
thread standing, which `--all` does not. It ends every subscription watching
that event type, whatever `--condition` each one carries.

One sharp edge: a subscription can watch several event types (repeated `--on`
at `await-event`). Naming ONE of them ends that whole subscription, the other
names included. A wait is a single rendezvous, spent by the first match, so no
leg is left once you stop watching one. The result names every type it ended,
so read it. To keep watching the rest, arm a new subscription for them.

```bash
# The user changed their mind about one of several watches.
$ lucidos event-waits cancel --wait-id 3f2b1c04-...
{"status":"stopped","message":"Stopped watching for ChangeProposed. It will not re-open this thread."}

# The release build finished while you were doing something else.
$ lucidos event-waits cancel --on ReleasePublished
{"status":"stopped","message":"Stopped watching for ReleasePublished. Nothing on this thread watches ReleasePublished any more. 1 other subscription(s) on this thread is still live."}

# Stand everything down.
$ lucidos event-waits cancel --all
```

Refusals arrive as a `400` with the reason:

- more than one flag, or none;
- a `--wait-id` not live on this thread (fired, timed out, stopped, or another
  thread's: all look the same and mean "not yours to stop");
- an `--on` nothing on this thread watches. Read that one: a watch you thought
  was armed is not.

Scripts use this too. In `scripts/lib/e2e_lock.sh`, a run that takes the
machine-wide e2e lock runs `lucidos event-waits cancel --on E2ELockReleased`,
since holding the lock answers any watch for its release. The call is best
effort and its refusal is discarded, since most runs never subscribed.

