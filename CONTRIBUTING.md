# Contributing to Lucidos

Thanks for your interest in Lucidos! This guide covers how to set up the dev
environment, the branch and PR flow, our commit conventions, and the sign-off we
require on every contribution. Keep it open in a tab — it's meant to be concrete,
not exhaustive.

> **Pre-1.0, expect breakage.** Lucidos is currently on the **0.9.x** line.
> Until 1.0 the public surfaces — events, the HTTP API, the JS SDK, the database
> schema, on-disk layout — can change without notice. Pin a commit if you need
> stability, and don't be surprised when `main` moves under you.

By participating you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).
How the project is run, and how you can grow into a maintainer, is described in
[GOVERNANCE.md](GOVERNANCE.md).

## Speak the project's language

Lucidos has a precise vocabulary — **workspace**, **app**, **intent**,
**knowhow**, **trigger**, **event**, **artifact**, **thread**. These aren't
interchangeable synonyms; each has a specific meaning. Use the canonical term in
issues, PRs, commit messages, and code. The two glossaries are the source of
truth:

- [`system-knowhow/glossary.md`](system-knowhow/glossary.md) — user-facing terms.
- [`docs/glossary.md`](docs/glossary.md) — dev-only terms (extends the above).

A quick skim before you write will keep the conversation aligned.

## Set up the dev environment

Prerequisites and the full walkthrough live in the [README](README.md#dev-setup).
The short version:

```bash
# Build the engine and start a dev workspace
./scripts/web-dev.sh -w ~/workspaces/dev -b

# Later runs (binary already built)
./scripts/web-dev.sh -w ~/workspaces/dev
```

This brings up PostgreSQL + pgvector in Docker, builds and runs the Rust engine
natively, and serves the frontend. Each workspace gets its own ports, so several
can run side by side. See the README for prerequisites (Rust, Docker, Node.js, an
LLM provider), port assignment, and local HTTPS.

The deeper working conventions for the codebase — Rust, events, migrations,
frontend, testing — live in [`CLAUDE.md`](CLAUDE.md) and the rule files under
[`.claude/rules/`](.claude/rules/). They apply to humans and AI coding agents
alike; read the ones relevant to what you're touching.

## Branch and PR flow

We develop on GitHub with a fork-and-pull-request model:

1. **Fork** the repository and clone your fork.
2. **Branch** off `main`. Name the branch after the change, prefixed with its
   type — e.g. `feat/trigger-group-reorder`, `fix/thread-drawer-spacing`,
   `docs/contributing-guide`.
3. **Make your change**, with tests (see below). Keep the branch focused —
   one logical change per PR is much easier to review.
4. **Commit** following our message conventions, **signed off** (see DCO below).
5. **Open a PR** against `main`. Fill in the
   [pull request template](.github/PULL_REQUEST_TEMPLATE.md) and link the issue
   it addresses.

> **There is no public CI.** Nothing runs your tests for you on push, and there
> are no status checks to "wait for". Run the relevant suites locally and report
> what you ran in the PR — a maintainer will not be able to tell green from red
> otherwise. Releases are cut locally by maintainers via `scripts/release.sh`;
> contributors never need to touch the release flow.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/). The subject
line is `type(scope): summary`, written in the imperative mood:

```
feat(threads): sort drawer by last user action
fix(push): skip wake-push when the notification was already read
docs: genericize README for public release
refactor(app): rename coding-agent control surface off the "cc" prefix
```

Common types: **`feat`**, **`fix`**, **`docs`**, **`refactor`**, **`chore`**,
**`test`**. The scope is optional but encouraged — use the area you touched
(`engine`, `threads`, `push`, `app`, `models`, …). Add a body explaining the
*why* whenever the change isn't self-evident.

If your change touches a documented surface (an event type, the JS SDK, the CLI,
the plugin manifest, a glossary term, …), update the matching `system-knowhow/*.md`
**in the same commit** — stale knowhow misleads both contributors and the engine
LLM that reads it. This rule is spelled out in
[`.claude/rules/system-knowhow.md`](.claude/rules/system-knowhow.md).

## Sign your work (DCO)

Lucidos requires a [Developer Certificate of Origin](https://developercertificate.org/)
sign-off on every commit. The DCO is a lightweight statement that you wrote the
patch, or otherwise have the right to submit it under the project's
[MIT license](LICENSE). It is **not** a CLA — you keep the copyright to your work.

Sign off by adding `-s` to your commit:

```bash
git commit -s -m "fix(threads): exclude mid-turn threads from needs-attention"
```

This appends a trailer to the commit message:

```
Signed-off-by: Your Name <your.email@example.com>
```

Use your real name and a reachable email. Every commit in a PR must carry the
trailer; if you forget, `git rebase --signoff main` adds it to the whole branch.
PRs with unsigned commits can't be merged.

## Tests

Run the suites for the layers you touched. Because there's no CI, this is on you.

| You changed… | Run |
|---|---|
| Rust (`.rs`, `Cargo.toml`, `.sql`) | `make test` (engine tests against a disposable Postgres) |
| HTTP API surface | also `./scripts/e2e-api.sh` |
| TypeScript / frontend | `cd crates/lucidos-app && npx tsc --noEmit && npm test` |
| UI behaviour / flows | `./scripts/e2e-browser.sh` |
| Everything, end to end | `./scripts/e2e.sh` (API + browser + WASM + embedder) |
| Docs / CSS only | no tests needed |

> Don't run bare `cargo test -p lucidos-engine` — the integration tests need a
> real Postgres, which `make test` (`./scripts/test-engine.sh`) provisions for
> you. Without it, every DB-backed test panics on connect and reports hundreds of
> false failures. See [`.claude/rules/testing.md`](.claude/rules/testing.md).

Bug fixes should come with a failing test that the fix turns green. Refactors that
change data flow need integration coverage, not just unit tests.

## Reporting bugs and proposing features

Open an issue using the matching template:

- **Bug report** — something is broken.
- **Feature request** — something should exist.
- **Knowhow contribution** — you want to contribute a knowhow doc, app, or
  trigger. See [`docs/taxonomy.md`](docs/taxonomy.md) and the `building-*.md`
  guides under [`system-knowhow/`](system-knowhow/) first.

For open-ended questions and discussion, use
[GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions) rather
than the issue tracker.

## Security

Please **do not** open public issues for security vulnerabilities. Follow the
private disclosure process in [SECURITY.md](SECURITY.md).
