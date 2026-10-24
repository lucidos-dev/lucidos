# Develop Lucidos

Point a Lucidos workspace at a Lucidos source checkout and Lucidos can work on
its own code. You describe the change you want. A coding agent works in an
isolated git worktree, commits there, and proposes the result as a pending
**change**. You read the diff, then click **Apply** to merge that branch into
`main`. The UI is the same in a phone browser, so you can review and Apply from
a phone.

Only a checkout includes the platform source, so only a checkout can change it.
To run Lucidos, use the [desktop app or the one-line installer](quickstart.md).

Lucidos is a Rust engine, a TypeScript frontend, and PostgreSQL with pgvector as
the event store. You pick Claude Code or Codex as the coding agent per thread.
Lucidos is MIT licensed.

## What happens when you Apply

Apply merges the branch. What happens next depends on what the change touched.

- **A frontend-only change**: the engine picks up the newly built client in
  place, and the page offers a **Refresh** to load it.
- **An engine change** (Rust source, a migration, the SDK bundle) needs a new
  binary. Lucidos rebuilds in the background while the running engine keeps
  serving. When the new binary is ready, Lucidos offers **Switch to new
  version**. You choose when to switch, and in-flight threads resume after it.

!!! note "Review and hardening"
    A coding-agent thread commits only inside its own throwaway worktree, so you
    always get a diff to read before anything merges. A change to Lucidos's own
    source must also pass `/harden`. It reviews the diff against the project's
    rules and runs the test suites for the layers it touched. If a session
    skipped it, Apply runs it synchronously and you wait.

## Two ways in

**Clone and run it.** Clone the repository, then start a workspace with
`./scripts/web-dev.sh`. Pass `-b` the first time so it builds the engine. The
script starts PostgreSQL with pgvector in Docker and runs the Rust engine
natively. It also starts the shared dev gateway and serves the frontend behind a
build watcher. [Prerequisites and dev setup](#prerequisites-and-dev-setup) below
has the prerequisites and exact commands.

**Or bootstrap it in one command.** On a clean machine, the installer's `--dev`
flag does the whole setup:

{%
   include-markdown "../../README.md"
   start="<!--devbootstrap-start-->"
   end="<!--devbootstrap-end-->"
%}

The build is the slowest step. Use the debug flag if you only want to get
running.

## Prerequisites and dev setup

{%
   include-markdown "../../README.md"
   start="<!--devsetup-start-->"
   end="<!--devsetup-end-->"
   heading-offset=1
%}

## Where your workspace lives

A **workspace** is a directory you choose and pass with `-w` (for example
`~/workspaces/dev`), separate from the checkout. It holds your artifacts, apps,
triggers and knowhow under `data/`, plus a rebuildable `.lucidos/` runtime
directory. One checkout can serve several workspaces at once. Each gets its own
engine port and its own database in the shared PostgreSQL cluster.

## What a coding-agent thread can work on

When you start a thread, the **Coding agent on…** group in the compose
destination picker offers three kinds of target. The target you pick is the
agent's default write scope. Its worktree is a throwaway checkout of that target
only. Writing outside it needs your approval (see
[Permission cards](#permission-cards)).

| Target | What the worktree is | How the work lands |
|---|---|---|
| **Lucidos source** | A full worktree of the Lucidos repository. Offered only when the engine was launched from a source checkout. | Proposed as a *change*; **Apply** merges it into `main`. `/harden` runs. An engine-affecting change then offers *Switch to new version*. |
| **An installed app** (`data/apps/<id>/`) | A worktree of your workspace's git, sparse-checked-out to that one app folder. | Proposed as a *change* with the same Apply flow, against the workspace git. No engine restart, and Lucidos's `/harden` does not run (apps own their own hardening). |
| **A registered external repository** | A full worktree of a git repository you registered. | You review the diff in the external-repo diff viewer, then push or open a pull request yourself. |

The engine refuses any other target, with a message naming what to use instead:

- A path under `data/` other than one whole app folder (knowhow, triggers,
  artifacts, scripts) belongs to the chat path and its file tools.
- An unregistered folder must be registered as a repository first.
- The engine's own `.lucidos/` state directory is never a target.
- System roots are refused, unless you registered a repository that lives under
  one.

### Permission cards

Inside its worktree, the agent writes without asking. Three things raise a
permission card instead:

- a shell command that the agent's own gate flags
- a write outside the worktree, anywhere else on your machine
- a write into the worktree's `.git` directory, whose contents do not appear in
  the diff you review

The thread waits until you answer. A card offers four answers:

- **Allow once**
- **Allow for this thread**: remembered for that thread's lifetime.
- **Always allow**: remembered for every future thread, and editable under
  **Settings → Permissions**.
- **Deny**

## Contributing

The GitHub repository is a published mirror of the development repo. You fork
and open a pull request as usual. A maintainer then imports your PR into the next
release, credits you as co-author, and closes the PR with a link to that release.

- **[CONTRIBUTING.md](https://github.com/lucidos-dev/lucidos/blob/main/CONTRIBUTING.md)**
  is the guide: branch and PR flow, commit conventions, which test suites to run
  for what you touched, and the [DCO](https://developercertificate.org/) sign-off
  (`git commit -s`) required on every commit. CI does not run on pull requests,
  so run the relevant suites locally and say in the PR what you ran.
- **[Code of Conduct](https://github.com/lucidos-dev/lucidos/blob/main/CODE_OF_CONDUCT.md)**
  applies to everyone taking part.
- **[SECURITY.md](https://github.com/lucidos-dev/lucidos/blob/main/SECURITY.md)**
  has the private disclosure process. Do not open a public issue for a
  vulnerability.
- **[GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions)** is
  the place for questions, ideas, and anything open-ended.

Lucidos is pre-1.0. The public surfaces (events, the HTTP API, the JS SDK, the
database schema, the on-disk layout) can change without notice, and `main`
changes often.
