# 0126: A git clone credential comes from the credential store, never an environment variable

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Every engine clone was anonymous, so a private or internal repo failed as a
plugin marketplace with `remote authentication required but no callback set`.
Four sites clone: the marketplace scan, the plugin install fetch, and both
`git_clone` tool routes.

The first implementation read a token from the process environment
(`GITHUB_TOKEN`, `GH_TOKEN`, `GIT_TOKEN`), with a `GH_HOST` allowlist deciding
which hosts counted as GitHub. It was justified by a claim that turned out to be
false: that the credential store is unreachable from a clone, because the store
is `async` over a `PgPool` while git2 types are not `Send`.

## Decision

An HTTPS clone secret comes from the Lucidos credential store and nowhere else.
The caller resolves a `GitCredentials` in async code, then hands it into the
blocking clone. No engine clone reads a token from the environment.

## Rationale

The environment-variable store is **non-secret by design**. Its value rides in
the `EnvironmentVariableSet` event to every device and shows in plain text in
Settings. A personal access token stored there is a secret in a place built for
things that are not secrets.

The `Send` argument never bound. A clone body is sync, but every caller of one
is async and already holds a pool. So the resolve happens before
`spawn_blocking`, and nothing crosses an `.await` holding a git2 type.

Per-credential scoping falls out for free. A credential carries its own
`base_url`, and `credential_base_url_matches` compares scheme, host, port, then
a path prefix. So the `GH_HOST` allowlist has nothing left to do: a GitHub
Enterprise install is just another row, and no host has to be guessed.

## Consequences

- Setup is one row in Settings, Credentials: Base URL `https://github.com`, Auth
  Type Bearer Token. It takes effect on the next scan, with no engine restart.
- Base URL is the whole scoping rule, and it must be the **clone** host. A
  credential for `https://api.github.com` never matches a clone of
  `https://github.com/...`, so the REST API and the clone are two rows.
- A narrower Base URL wins, since matches sort by `base_url` length. An
  org-scoped token overrides a host-wide one.
- The callback re-matches on the URL libgit2 passes each time, never the URL the
  clone started with. So a redirect cannot carry a secret to another host.
- ssh-agent and the `git credential` helper are still offered. Neither carries a
  secret through Lucidos, so neither has the problem this ADR is about.
- A credential holding nothing a remote can present (`oauth_client`) is skipped.

## Alternatives considered

**A token in Settings, Environment Variables.** Shipped first, then rejected by
the maintainer. It works, and it needs no database read on the clone path. It
loses because the store broadcasts its values and displays them, so it turns a
token into shared, visible state. Keeping it as a *fallback* was offered and
also declined: two credential homes for one job is the drift this avoids.

**A `GH_HOST` allowlist for Enterprise hosts.** Necessary only while a token was
host-classified rather than host-scoped. A name like `github.<company>.<tld>` is
one registration away from being free, so the allowlist existed to stop a token
reaching an impostor. Per-credential `base_url` matching answers the same
question exactly, with no list to maintain.

**Reading the credential inside the clone callback.** Rejected on mechanics. The
callback is sync and non-`Send`, so it would need a runtime handle and a
blocking DB call per credential round, inside libgit2's retry loop.
