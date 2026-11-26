# 0811: A device on gateway A creates, starts, stops and restores workspaces on gateway B through a control hop, behind a control grant on B

- **Status**: Accepted
- **Date**: 2026-11-25
- **Amends**: [0656](0656-remote-gateway-manage-hop.md)

## Context

Restoring a workspace onto a remote gateway meant opening that gateway's own
web page in a browser and pairing a device there. The desktop app already
lists B's workspaces and opens running ones over the relay (ADR 0591). No
picker action reaches B: the relay carries workspace routes only, and the
manage hop (ADR 0656) kept "the control plane unreachable".

## Decision

1. **B serves a control hop** at `/~/api/gateway/v1/control/*`, and A forwards
   `/~/api/v1/remote-gateways/<name>/control/*` to it. A device on A drives it
   from A's own picker. The desktop window never leaves A's origin (ADR 0723).
2. **B serves it only when its pairing record for A carries the control
   grant** (`may_control`). The grant is off for new and existing pairings.
   B's user turns it on from the Paired gateways row, or with
   `lucidos pair --gateway --control`. A re-pair resets it.
3. **The hop serves an exact allow-list**: list workspaces, create one, start
   and stop one, the restore-upload routes, the restore commit, and the restore
   status. Delete, trash, rename, snapshot restore, adoption, pairing, the
   external address, TLS, updates and settings stay unreachable.
4. **A control claim names the fixed prefix `/~/control/`**, signed like the
   relay and manage claims. No claim kind verifies on another's route.
5. **`GATEWAY_PROTOCOL` becomes 6.** A checks B's protocol lazily, as the
   manage hop does.
6. **A forwards at most one restore chunk per request** (ADR 0810), so the hop
   passes through B's proxy whatever its body cap.

## Rationale

- **A separate grant from the manage grant.** Changing B's users and creating
  workspaces on B's machine are different trusts. One switch for both would
  widen whichever the owner meant to turn on.
- **Off by default.** An upgrade must not widen an existing pairing.
- **An allow-list, not a pass-through.** A compromised A can then use disk and
  memory on B, but cannot destroy B's workspaces, mint devices or pair a third
  gateway. Destructive actions stay with B's own devices.
- **A's picker, not B's page.** Pairing stays directional, and B's code never
  runs with A's browser authority unless trusted pairing (ADR 0807) says so.

## Consequences

- The picker gains a gateway switcher. With a remote gateway selected, it
  lists that gateway's workspaces and offers New workspace, Restore, Start and
  Stop there.
- B's log names each change made through the hop, with the device label and
  the gateway it came through.
- A maps B's 401 to a 502 with `reason_kind: remote-refused`.
- A B older than protocol 6 answers "update it first" through A.

## Alternatives considered

- **The desktop window navigates to B's origin.** Needs a device pairing on B
  and loses the relay workflow (ADR 0723). ADR 0656 forbids A minting a
  device on B.
- **Widen the manage grant.** One switch for two trusts, see Rationale.
- **Forward every control route.** One compromised A could then delete B's
  workspaces or change its network.
- **Include delete and rename.** Destructive and rare from a second machine.
  B's own devices keep them.
