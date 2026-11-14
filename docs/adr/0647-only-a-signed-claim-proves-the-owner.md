# 0647: Only a signed claim proves the owner; every other caller is an unattested caller whose owner-only acts wait for the owner

- **Status**: Accepted
- **Date**: 2026-11-10
- **Amends**: [0169](0169-every-caller-identifies-itself.md),
  [0192](0192-thread-delete-is-the-one-sanctioned-removal.md),
  [0590](0590-gateway-pairing-is-a-deliberate-grant.md) and
  [0641](0641-coding-agents-trust-by-provenance.md)
- **Amended by**: [0688: Every Allow once card is a permission request](0688-every-allow-once-card-is-a-permission-request.md) (an unattested caller's card is a once-only permission request)

## Context

ADR 0641 put coding-agent trust at spawn. The spawn gate walks a thread's
provenance, and a run fed by a trigger starts only with that trigger's grant.
The walk read the caller's `MessageOrigin`, and three origins could be claimed
by any local process:

1. a `caller_workspace` field in a `/api/v1/chat/stream` body;
2. the machine-local token, which read as the owner;
3. a typed `x-lucidos-device-id`, since an agent can read device ids from the
   database. ADR 0192 recorded this one as a residual.

Every subprocess Lucidos starts runs as the owner's OS user, with no sandbox.
So any of them could read the local token, type any header, and mint itself a
device pairing code. Cross-workspace calls skipped the gateway, so the target
could not verify the caller's origin token either.

## Decision

1. **Only a claim signed with a key held in memory proves the owner.** Two
   exist: the origin token (ADR 0641) and the *gateway stamp*.
2. **The gateway stamps what it authenticated.** It hands each engine it starts
   a fresh *stamp key* over the engine's stdin, and signs the paired device on
   every proxied request. A restart re-keys each engine a gateway spawned,
   which the engine reports in its health body (`started_by_gateway`). An
   engine another starter launched stays keyless, and a keyless engine reads
   every claim as unattested.
3. **Every other caller is an *unattested caller*.** It may read, chat and do
   ordinary writes, recorded as `MessageOrigin::UnattestedCaller`. What it
   claimed rides along as a display hint, never as authority.
4. **An owner-only door asks the owner.** An unattested caller's act parks on
   an owner approval card, and Allow once from an attested device runs it one
   time (ADR 0387). The doors are the coding-agent start, a trigger's
   coding-agent grant, Apply, discard and revert, human-only settings, and
   every `require_owner_device` route.
5. **Some doors refuse instead.** A reveal or ticket whose replay answers
   nobody refuses. So does a body holding a secret, and an answer to any card:
   asking the owner to approve an answer to their own card is circular.
6. **Cross-workspace calls go through the gateway.** The calling engine checks
   its origin token, runs its own spawn gate, and signs the call. The gateway
   verifies it and re-stamps it for the target. The target honors the caller's
   verdict and never upgrades it.
7. **Another gateway relays a call only after a gateway pairing.** The relayed
   claim is signed with the pairing's credential, and the target's gateway
   re-stamps it. An unpaired peer's call lands unattested.
8. **A loopback peer may pair over plain `http`, and over `https` with any
   certificate.** Nothing crosses a network on loopback, so TLS adds nothing.
   Any other address still pairs over verified `https` only.
9. **A local process's pairing waits for a paired device's Allow** once any
   device is paired. A code a paired device minted never waits, and with
   nothing paired, nothing waits. No terminal command answers it.
10. **The local token is a door credential only.** One switch,
    `LOCAL_TOKEN_PROVES_OWNER` in `api/local_auth.rs`, holds its old meaning,
    off. It is a registered temporary measure, which the OS sandbox flips back.

## Rationale

A header is a string, and every local process can write one. Only a secret the
subprocess cannot read separates the owner's device from an agent pretending to
be it. A key held only in the gateway's and the engine's memory is such a
secret. No file or environment variable may ever hold it.

Refusing unattested callers outright would break a hand-typed `lucidos`
command and every script. Asking keeps them working at the cost of one tap, and
the tap comes from the one place a forger cannot reach.

The target honors the caller's verdict because only the caller can read its
own provenance. Both workspaces belong to the same owner, so the caller's
grants are the owner's grants.

## Consequences

- An agent can no longer start a coding agent, flip a grant, or apply a change
  by naming a device or a workspace. Its attempt shows as a card naming it.
- A hand-typed `lucidos` command in a plain terminal is unattested. Its
  owner-only acts cost a tap until the sandbox flips the local-token switch.
- A direct-port browser (`http://localhost:<engine port>`) is unattested. It is
  for dev testing only.
- A thread's `lucidos spawn-thread --to B` works with no tap on one gateway,
  and after one gateway pairing across two.
- **The e2e harness is the starter of its engine.** It pipes a key into the
  engine's stdin, as the gateway does, and its test processes sign with it.
  Those processes start later, so the key also sits in a 0600 file in the
  disposable e2e workspace's `.lucidos/` (`scripts/lib/e2e_stamp.sh`). No
  production key touches disk.
- Past events keep their old reading. The gate cannot tell a forged past
  origin from a real one, and the event log is never rewritten.

**Residuals.** A process running as the owner can still rewrite Lucidos's own
files: the paired-devices and remote-gateways files, and the shared Postgres
password. That raises the bar from one header to tampering with Lucidos
internals, and only an OS sandbox closes it.

Revoke stays open to a local process, because a desktop-only user whose window
lost its cookie needs `lucidos devices revoke --all` to get back in. So an
agent can win a free pairing, but only by logging out every one of the owner's
devices. That is loud, and the owner sees it at once.

This amends:

- ADR 0169: a caller that identifies itself is not thereby the owner. Only a
  signed identity is.
- ADR 0192: the typed-device-id residual is closed. A delete from an unattested
  caller waits for the owner.
- ADR 0590: a loopback peer may pair over `http` (decision 8), and a pairing
  secret a local process minted waits for the owner (decision 9).
- ADR 0641: the "token-less paths" its Consequences left open are closed. A
  `caller_workspace` builds `Workspace` only under a stamp, and carries the
  caller's verdict.

## Alternatives considered

- **Refuse every unattested owner-only act.** Rejected: it breaks scripts and
  a hand-typed CLI, with no way back short of opening the app.
- **Hand the stamp key over in an environment variable or a file.** Rejected:
  every subprocess inherits the one and can read the other.
- **Let the target workspace walk the caller's provenance.** Rejected: the
  target cannot read the caller's events.
- **Answer a pairing request from the terminal.** Rejected: an agent runs any
  command the owner can, so a terminal Allow undoes the protection.
- **Close revoke to a local process.** Rejected: it strands a desktop-only
  user whose stale device row blocks the free first pairing.

Plan: `docs/plans/2026-11-10-unattested-callers-and-gateway-stamps.md`.
