# 0339: A handshake script gets only well-formed CRED_* and OAUTH_* names, never a credential's custom alias

- **Status**: Accepted
- **Date**: 2026-10-01

## Context

A `script_handshake` layer runs an approved Python script with a cleared
environment (ADR 0144). The runner restores a fixed runtime allowlist, then
adds the env vars its pipeline layer supplies. Those come from
`credential_env_vars_for` and `oauth::account_env_vars`.

A credential can carry a custom env var name. The credential then injects
its secret twice: as `CRED_<NAME>` and under that alias. The write path checks
the alias for shape and refuses engine-owned names, nothing more. So an alias
could be `PYTHONPATH`, and a planted module would then run inside an approved
script with the handshake's credentials. That bypasses the approval gate.

The first fix dropped a blocklist of loader-hook names at the runner:
`PYTHON*`, `OPENSSL_*`, the `command_guard` code-injecting names and the
runtime names. A blocklist stops only the names someone listed. `HTTPS_PROXY`
and `REQUESTS_CA_BUNDLE` were not listed, and either one steers where the
script sends the credential it just received.

## Decision

The runner passes a pipeline-supplied name only when it is `CRED_` or
`OAUTH_` followed by one or more of `A-Z`, `0-9` and `_`. Everything else is
dropped and logged. The runtime allowlist is unchanged.

## Rationale

**An allowlist matches what the pipeline actually produces.** Every canonical
name is `CRED_` or `OAUTH_` plus an `env_var_segment` tail, which is exactly
that shape. Nothing legitimate falls outside it.

**A custom alias never passes, by construction.** The credential write path
refuses the `CRED_` and `OAUTH_` prefixes for an alias. So the rule needs no
separate "drop the alias" case, and no new loader hook can reopen the hole.

**A handshake has no need for the alias.** The alias exists for a third-party
CLI or SDK that expects an exact variable name. A handshake script is written
for Lucidos and reads `CRED_<NAME>`, which always arrives.

## Consequences

- A handshake script that read a custom alias stops seeing it and must read
  `CRED_<NAME>`. No shipped `apis.json` or knowhow example reads an alias.
- `run_bash`, `run_python` and scheduled scripts still get the alias. Only the
  handshake runner narrows.
- `command_guard::is_code_injecting_env_name` lost its second caller and is
  private again.

## Alternatives considered

- **Keep the blocklist and extend it.** Rejected: each new proxy, CA or loader
  variable is another bypass until someone lists it.
- **Stop `credential_env_vars_for` emitting the alias for a handshake.** That
  moves the rule away from the spawn. The runner is the boundary for untrusted
  code, so it checks every name it receives, whoever built the list.
- **Reserve the hook names globally on the write path.** Rejected: a user may
  legitimately set `LD_LIBRARY_PATH` or `HTTPS_PROXY` for their own scripts.
