# Develop Lucidos

Point a Lucidos workspace at a Lucidos source checkout and Lucidos can work on
its own code. You describe the change you want; a coding agent picks it up in
an isolated git worktree, commits there, and proposes the result as a pending
**change**. Nothing has touched your running system yet. You read the diff, and
only when you click **Apply** does that branch merge into `main`. Lucidos is
built this way, and because the UI is the same in a phone browser, so is reading
a diff and applying it.

That loop is the reason to run from source, and it is the *only* reason. Running
from source is not a third way to install Lucidos: the [desktop app and the
one-line installer](quickstart.md) are better at that. Neither of them ships the
platform's own source, though, so neither can change it. A checkout can.

Under the hood it is a Rust engine, a TypeScript frontend, and PostgreSQL with
pgvector as the event store. The coding agent is Claude Code or Codex, picked per
thread. MIT licensed.

## What happens when you Apply

Apply merges the branch. It never restarts anything on click, and what happens
next depends on what the change touched.

- **A frontend-only change** needs no new binary. The engine picks up the freshly
  built client in place, and the page offers a **Refresh** to load it.
- **An engine change** (Rust source, a migration, the SDK bundle) does need a new
  binary. Lucidos rebuilds in the background while the running engine keeps
  serving, then offers **Switch to new version** once the new binary is actually
  ready. Taking the switch is a separate, deliberate act; in-flight threads
  resume across it.

!!! note "The agent proposes, you dispose"
    A coding-agent thread only ever commits inside its own throwaway worktree, so
    there is always a diff to read before anything merges. A change to Lucidos's
    own source also has to pass a hardening gate first: `/harden` reviews the diff
    against the project's rules and runs the test suites for the layers it
    touched. If a session skipped it, Apply runs it synchronously and you wait.

## Two ways in

**Clone and run it.** The ordinary dev setup: clone the repository, then bring up
a workspace with `./scripts/web-dev.sh` (pass `-b` the first time so it builds the
engine). That starts PostgreSQL with pgvector in Docker, runs the Rust engine
natively, starts the shared dev gateway, and serves the frontend behind a build
watcher. Prerequisites and the exact commands are in
[Prerequisites and dev setup](#prerequisites-and-dev-setup) below.

**Or bootstrap it in one command.** On a clean machine the installer's `--dev`
flag does the whole setup for you:

{%
   include-markdown "../../README.md"
   start="<!--devbootstrap-start-->"
   end="<!--devbootstrap-end-->"
%}

Budget for that build: it is the slow step of the whole setup, and the reason to
reach for the debug flag while you are only trying to get running.

## Prerequisites and dev setup

{%
   include-markdown "../../README.md"
   start="<!--devsetup-start-->"
   end="<!--devsetup-end-->"
   heading-offset=1
%}

## Where your workspace lives

A **workspace** is not the checkout. It is a directory you choose and pass with
`-w` (say `~/workspaces/dev`), holding your artifacts, apps, triggers and knowhow
under `data/`, plus a rebuildable `.lucidos/` runtime directory. One checkout can
serve several workspaces at once, and each gets its own engine port and its own
database inside the shared PostgreSQL cluster, so a scratch workspace and a real
one never collide.

## What a coding-agent thread can work on

Starting a thread, the compose destination picker's **Coding agent on…** group
offers three kinds of target. Whichever one you pick becomes the agent's default
write scope: its worktree is a throwaway checkout of that target and nothing
else, and reaching outside it takes your explicit approval (see the permission
cards below).

| Target | What the worktree is | How the work lands |
|---|---|---|
| **Lucidos source** | A full worktree of the Lucidos repository. Offered only when the engine was launched from a source checkout, and hidden on a packaged install, which has no platform source to edit. | Proposed as a *change*; **Apply** merges it into `main`. `/harden` runs. An engine-affecting change then offers *Switch to new version*. |
| **An installed app** (`data/apps/<id>/`) | A worktree of your workspace's git, sparse-checked-out to that one app folder. | Proposed as a *change* with the same Apply flow, against the workspace git. No engine restart, and Lucidos's `/harden` does not run (apps own their own hardening). |
| **A registered external repository** | A full worktree of a git repository you registered. | No Apply / Discard surface at all. You review the diff in the external-repo diff viewer and push or open a pull request yourself. |

Anything else is refused, with a message naming what to use instead. A path under
`data/` that is not exactly one whole app folder (knowhow, triggers, artifacts,
scripts) belongs to the chat path and its file tools. An unregistered folder has
to be registered as a repository first. The engine's own `.lucidos/` state
directory is never a target, and system roots are refused unless you deliberately
registered a repository that lives under one.

### Permission cards

Inside its worktree the agent writes without asking, because that worktree is
disposable and you read the diff before Apply. Three things escalate to a
permission card instead: a shell command the agent's own gate flags, a write
*outside* the worktree (anywhere else on your machine), and a write into the
worktree's `.git` directory, the one in-tree place whose contents never show up
in the diff you review. The thread waits on you until you answer.

So the worktree is where the agent works unsupervised, not a boundary it cannot
cross: a card offers **Allow once**, **Allow for this thread** (remembered for
that thread's lifetime) and **Always allow** (remembered for every future thread,
editable under **Settings → Permissions**) alongside **Deny**. Approving one is
how the agent reaches outside, and it takes a decision from you each time until
you say otherwise.

## Contributing

The GitHub repository is a published mirror rather than the development repo, so
only the landing step differs from an ordinary fork-and-pull-request flow: a
maintainer imports your PR into the next release and closes it with a link to
that release, crediting you as co-author. Everything before that is the flow you
already know.

- **[CONTRIBUTING.md](https://github.com/lucidos-dev/lucidos/blob/main/CONTRIBUTING.md)**
  is the guide: branch and PR flow, commit conventions, which test suites to run
  for what you touched, and the [DCO](https://developercertificate.org/) sign-off
  (`git commit -s`) required on every commit. Note that no CI runs on pull
  requests, so run the relevant suites locally and say in the PR what you ran.
- **[Code of Conduct](https://github.com/lucidos-dev/lucidos/blob/main/CODE_OF_CONDUCT.md)**
  applies to everyone taking part.
- **[SECURITY.md](https://github.com/lucidos-dev/lucidos/blob/main/SECURITY.md)**
  has the private disclosure process. Please do not open a public issue for a
  vulnerability.
- **[GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions)** is
  the place for questions, ideas, and anything open-ended.

Lucidos is pre-1.0. The public surfaces (events, the HTTP API, the JS SDK, the
database schema, the on-disk layout) can change without notice, and `main` moves
under you.
