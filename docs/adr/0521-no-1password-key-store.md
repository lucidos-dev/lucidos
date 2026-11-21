# 0521: 1Password is not a KeyStore for the secrets key

- **Status**: Accepted
- **Date**: 2026-10-18

## Context

ADR 0509 seals stored secrets with a secrets key, held behind the `KeyStore`
trait: the macOS Keychain for the signed `.app`, a 0600 key file elsewhere. A
user who keeps their secrets in 1Password asked whether 1Password could hold
that key instead.

Plan: `docs/plans/2026-10-17-public-key-backups-and-1password-references.md`,
decision 2.

## Decision

No 1Password `KeyStore`. The secrets key stays in the Keychain or the key file.
What the user wants from 1Password is met by *secret references* instead (ADR
0522): a credential points at a secret they already keep there.

## Rationale

Each thing a 1Password store could add is either already there or not
delivered:

| Goal | Verdict |
|---|---|
| An off-machine copy of the key | Already exists. Every archive carries the secrets key, and only the user's backup key opens it (ADR 0520). |
| Protection on the machine | None. A service account token sits on the machine and fetches the key, so a same-user process that reads one reads the other. |
| Availability | Worse. Boot would need the network and 1Password. Caching the key locally to stay offline undoes the point. |
| Transport | Awkward. Service accounts have no REST API, only the `op` tool and the SDKs. |
| A hosted Lucidos | Wrong store. A cloud KMS belongs behind the same trait. |

## Consequences

- The `KeyStore` trait keeps its two backends. A cloud KMS remains the planned
  third, for hosted.
- Lucidos boots offline, with no dependency on any password manager.

## Alternatives considered

**A 1Password store with a local cache.** Rejected. The cache is a key file
with extra steps, and it adds a network dependency for no protection.

**Unlock through the 1Password desktop app (Touch ID).** Rejected. An
unattended engine cannot answer the prompt, so a restart at night would stall.
