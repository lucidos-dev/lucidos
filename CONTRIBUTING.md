# Contributing to Lucidos

This guide covers the dev environment, the branch and PR flow, commit
conventions, and the sign-off every contribution needs.

> **Pre-1.0, expect breakage.** The newest `v*` tag is the current version.
> Until 1.0, the public surfaces can change without notice: events, the HTTP
> API, the JS SDK, the database schema, and the on-disk layout. Pin a commit if
> you need stability.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
[GOVERNANCE.md](GOVERNANCE.md) describes how the project is run and how to
become a maintainer.

## How this repository works

This repository is a **published mirror**. Lucidos is developed in a private
source repository, and each release is exported here as a single commit.

- **`main` gains one commit per release.** Each release commit is tagged
  `v<x.y.z>` and has the previous release's commit as its single parent. The
  history is linear: `git log`, `git describe` and `git diff` between two
  release tags all work, and `git pull` fast-forwards. Each commit is a
  *snapshot* of the source repo. Internal-only paths (planning docs, the
  release tooling) are stripped from the published tree.
- **Your PR is imported.** A maintainer squashes it onto the previous release
  tag and ships it in the next release. The release commit carries a
  `Co-authored-by:` trailer naming your GitHub account. The change is also
  ported back into the source repo, so it stays in every later release.
- **Your PR is then closed with a link to the release containing it.** GitHub
  shows it as "Closed". If the closing comment links a `v<x.y.z>` release,
  your change shipped.

> **A clone from before `v0.21.0` needs a one-time reset.** Chaining the
> earlier releases into one history changed all of their SHAs. If you cloned
> or forked before `v0.21.0`, `git pull` will conflict or produce a nonsense
> merge. Adopt the rebuilt history once, and move in-flight work onto it:
>
> ```bash
> # origin = your fork, upstream = this repository
> git fetch upstream
> git checkout main && git reset --hard upstream/main   # adopt the history
> git checkout my-branch
> git rebase --onto main <commit-you-branched-from>     # replay your work
> ```
>
> If the branch has already shipped, you can re-fork instead.

## Speak the project's language

Lucidos uses a precise vocabulary: **workspace**, **app**, **intent**,
**knowhow**, **trigger**, **event**, **artifact**, **thread**. Each term has a
specific meaning. Use the canonical term in issues, PRs, commit messages, and
code. The two glossaries are the source of truth:

- [`system-knowhow/glossary.md`](system-knowhow/glossary.md): user-facing terms.
- [`docs/glossary.md`](docs/glossary.md): dev-only terms (extends the above).

## Set up the dev environment

Prerequisites and the full walkthrough live in the [README](README.md#dev-setup).
The short version:

```bash
# Build the engine and start a dev workspace
./scripts/web-dev.sh -w ~/workspaces/dev -b

# Later runs (binary already built)
./scripts/web-dev.sh -w ~/workspaces/dev
```

This starts PostgreSQL + pgvector in Docker, builds and runs the Rust engine
natively, and serves the frontend. Each workspace gets its own ports, so several
can run side by side. The README covers prerequisites (Rust, Docker, Node.js, an
LLM provider), port assignment, and local HTTPS.

The working conventions for the codebase (Rust, events, migrations, frontend,
testing) live in [`CLAUDE.md`](CLAUDE.md) and the rule files under
[`.claude/rules/`](.claude/rules/). They apply to humans and AI coding agents.
Read the ones for the area you change.

## Branch and PR flow

1. **Fork** the repository and clone your fork.
2. **Branch** off `main`. Name the branch after the change, prefixed with its
   type: for example `feat/trigger-group-reorder`, `fix/thread-drawer-spacing`,
   `docs/contributing-guide`.
3. **Make your change**, with tests (see below). Keep to one logical change per
   PR.
4. **Commit** following our message conventions, **signed off** (see DCO below).
5. **Open a PR** against `main`. Fill in the
   [pull request template](.github/PULL_REQUEST_TEMPLATE.md) and link the issue
   it addresses.
6. **A maintainer imports it into a release** and closes the PR with a link to
   that release, crediting you as co-author. See
   [How this repository works](#how-this-repository-works).

> **CI does not run on pull requests.** The mirror's workflows are release
> gates. They run on release candidates, version tags, and published releases.
> Run the relevant suites locally and list what you ran in the PR. Maintainers
> cut releases locally, so contributors do not touch the release flow.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/). The subject
line is `type(scope): summary`, in the imperative mood:

```
feat(threads): sort drawer by last user action
fix(push): skip wake-push when the notification was already read
docs: genericize README for public release
refactor(app): rename coding-agent control surface off the "cc" prefix
```

Common types: **`feat`**, **`fix`**, **`docs`**, **`refactor`**, **`chore`**,
**`test`**. The scope is optional. Use the area you touched (`engine`,
`threads`, `push`, `app`, `models`, …). Add a body explaining the *why* when
the change needs it.

A change to a documented surface updates the matching `system-knowhow/*.md`
**in the same commit**. Documented surfaces include event types, the JS SDK, the
CLI, the plugin manifest, and glossary terms. The engine LLM reads these files.
[`.claude/rules/system-knowhow.md`](.claude/rules/system-knowhow.md) spells out
the rule.

## Sign your work (DCO)

Lucidos requires a [Developer Certificate of Origin](https://developercertificate.org/)
sign-off on every commit. The DCO states that you wrote the patch, or have the
right to submit it under the project's [MIT license](LICENSE). You keep the
copyright to your work.

Sign off by adding `-s` to your commit:

```bash
git commit -s -m "fix(threads): exclude mid-turn threads from needs-attention"
```

This appends a trailer to the commit message:

```
Signed-off-by: Your Name <your.email@example.com>
```

Use your real name and a reachable email. Every commit in a PR must carry the
trailer. If you forget, `git rebase --signoff main` adds it to the whole branch.
A PR with unsigned commits can't be released.

## Tests

Run the suites for the layers you touched.

| You changed… | Run |
|---|---|
| Rust (`.rs`, `Cargo.toml`, `.sql`) | `make test` (engine tests against a disposable Postgres) |
| HTTP API surface | also `./scripts/e2e-api.sh` |
| TypeScript / frontend | `cd crates/lucidos-app && npx tsc --noEmit && npm test` |
| UI behaviour / flows | `./scripts/e2e-browser.sh` |
| Everything, end to end | `./scripts/e2e.sh` (API + browser + WASM + embedder) |
| Docs / CSS only | no tests needed |

> Don't run bare `cargo test -p lucidos-engine`. The integration tests need a
> real Postgres, and `make test` (`./scripts/test-engine.sh`) provisions one.
> Without it, every DB-backed test panics on connect. See
> [`.claude/rules/testing.md`](.claude/rules/testing.md).

A bug fix comes with a failing test that the fix turns green. A refactor that
changes data flow needs integration tests as well as unit tests.

## Reporting bugs and proposing features

Open an issue using the matching template:

- **Bug report**: something is broken.
- **Feature request**: something should exist.
- **Knowhow contribution**: you want to contribute a knowhow doc, app, or
  trigger. Read [`docs/taxonomy.md`](docs/taxonomy.md) and the `building-*.md`
  guides under [`system-knowhow/`](system-knowhow/) first.

A feature request may add a **surface** (a new place the user interacts with
Lucidos) or an **integration** (a new relationship with somebody else's
product). For those, read [`docs/philosophy.md`](docs/philosophy.md) and say in
the issue how your proposal answers it. That page also lists two ideas already
settled as a *no*. It applies only to surfaces and integrations.

For open-ended questions and discussion, use
[GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions).

## Security

Please **do not** open public issues for security vulnerabilities. Follow the
private disclosure process in [SECURITY.md](SECURITY.md).
