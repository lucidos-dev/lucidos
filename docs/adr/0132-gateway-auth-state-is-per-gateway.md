# 0132: Auth state is per gateway; the local token stays machine-wide

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Two gateways run on one machine on purpose. The packaged app holds 5252 and a
dev checkout holds 5251, and their data dirs differ: one under the app's
support directory, one under `~/.lucidos/gateway`. The plan that split those
ports recorded that they were "otherwise already isolated".

ADR 0094 then added inbound auth and put the paired-device store at
`~/.lucidos/paired-devices.json`, derived from `HOME`. It never asked which of
the two owned it, because the question did not come up while pairing was being
designed. Both did.

The result was reported as "pairing is broken, and a new code does not help".
Each gateway loaded that one file at boot and treated memory as the truth. Each
then rewrote the whole file from that memory on every change, so every write
deleted the other's devices. Asked for their lists, the two disagreed, and the
file on disk matched only the one that wrote last.

Pairing codes made it worse. They live in process memory, so a code minted on
one gateway could never be redeemed on the other. With `tailscale serve`
fronting one gateway and a browser open on the other, every code the phone typed
was refused.

## Decision

**A device pairs to a gateway, not to a machine.** The paired-device store moves
to `<data dir>/paired-devices.json`, so each gateway owns one file and nothing
else writes it. Pending pairing codes stay in process memory, which is now
correct rather than merely cheap: the gateway that mints a code is the gateway
that stores the device.

**The credential cookie is named per gateway too**, `lucidos_device_<id>`, where
the id digests the same data dir that scopes the store. A cookie is scoped to
the host and ignores the port. Two gateways on one hostname therefore shared one
slot, and each pairing evicted the other. The old shared name is still read as a
fallback. A credential arriving under it is re-issued under the gateway's own
name on that same response.

**The local token stays machine-wide** at `~/.lucidos/local-token`, and every
gateway accepts it. It answers whether a caller is a process on this machine,
which is a property of the machine.

**`updates.toml` stays machine-wide too**, and its write became a partial update
that re-reads first.

Two consumers had assumed one machine-wide store and are corrected here.
`lucidos pair` probed `[5252, 5251]` and minted on the first answer. It now
refuses two answering gateways and asks for `--port`. The pairing QR's origin
must reach the gateway that minted the code. The existing derivation already
held that, and it is now stated and pinned by a test.

## Rationale

**The split follows what the state is about.** A paired device is a grant to
reach a set of workspaces, and the workspaces are the gateway's. Two gateways
serve different workspaces, so one list covering both was never one fact. The
local token is the opposite: it says "this caller is local", which is true of
the machine and of nothing smaller.

**One writer per file beats a lock.** The alternative kept the shared store and
made it multi-process safe. That needs a lock file, a re-read before every
write, and a freshness check on a path that runs for every proxied request. All
of it exists to reconcile two processes that have no reason to disagree once the
file is theirs alone.

**Scoping the token with it would break the CLI.** `lucidos pair` finds a
gateway by probing ports. It cannot know which data dir the answering process
uses, so it could not find that gateway's token.

**Isolation is only safe once the guessers stop guessing.** With one store, a
code minted on the wrong gateway still worked. The two callers that pick a
gateway for you therefore had to be corrected in the same change. Otherwise the
fix trades a silent clobber for a silent refusal.

## Consequences

- A device is paired once per gateway. Settings shows the list of the gateway
  serving that page, and revoking there revokes there.
- `~/.lucidos/paired-devices.json` becomes a read-only seed. Each gateway copies
  it once, the first time it finds no store of its own, so nobody paired before
  the change is locked out. It is copied rather than moved: the other gateway
  still needs it, and no gateway can tell whether the others have read it. It is
  never deleted and never written again.
- `lucidos pair` fails on a two-gateway machine until told which one. That is
  the point, and the message names the ports it found.
- One browser can hold a pairing to every gateway on a hostname at once, which
  the shared cookie name made impossible.
- A device paired before the cookie split keeps working and is moved to its
  gateway's own cookie on its next authorized request. The legacy cookie is left
  in place rather than cleared: clearing it would sign the browser out of every
  other gateway on the host.
- The remaining machine-wide state is the local token, `updates.toml`,
  `network.toml` (read-only here) and the port registry, which belongs to the
  dev scripts and the engines.

## Alternatives considered

**Refuse to start a second gateway.** A machine-global owner record beside the
shared secrets, with the second gateway exiting and naming the first. It would
have prevented the reported failure outright and it matches ADR 0021's shape.
Rejected because running the packaged app beside a dev checkout is a supported
setup that the maintainer relies on, and this would have banned it.

**Keep one shared store and make it multi-process safe.** The file becomes the
source of truth. Mutations take an `fs2` flock and re-read before writing, and
pending codes move to a shared file holding digests. It preserves "pair the
phone once for this Mac", which reads well.

Rejected on two counts. It puts a lock and a freshness check on the request
path, to reconcile two processes that need not share the state at all. And it
widens a code minted on a dev checkout into authority over the packaged
install's workspaces.

**Warn loudly and keep both running as they were.** The second gateway logs the
conflict and the picker shows a banner. Rejected as a diagnosis rather than a
fix: the store still gets clobbered and half the codes still fail.

**Scope the local token per gateway as well.** Symmetrical, and wrong. Locality
is a machine fact, and the CLI resolves a gateway by port with no way to find
that gateway's data dir.
