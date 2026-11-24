# 0699: Permission boundaries follow execution and agent cards use the question tool

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: ADR 0645, ADR 0664, ADR 0671, ADR 0685, ADR 0688

## Context

The command classifier treated machine-made workspaces inside hidden gateway
storage as secret, even when the executing OS sandbox deliberately opened them.
Unstamped command cards offered lasting grants without the engine's judgment.
Batches raised a card outside the agent's blocking question call, so its next
text could overtake it. The broker spent Allow once before a definite gateway
refusal, and device sessions could name a paired owner's device.

## Decision

- The classifier adds the executing sandbox's resolved hidden roots, including
  canonical and home spellings, while keeping every legacy private suffix.
  Open roots are carve-outs only under that process's enforced sandbox.
  Traversal stays secret. Chat classifies and spawns with one snapshot;
  coding-agent checks use the session's spawn snapshot and fail closed without it.
- Command cards offer only engine-stamped scopes. Unstamped cards offer once
  and deny. No lasting grant covers a secret read.
- Agent-raised batches prepare a permission request and return `request_id`.
  The agent asks its question tool with that id alone, as for ask-permission.
  Both chat and coding-agent question handlers swap in the engine's card.
  Each re-ask is separate consent. Digest, drift checks and one spend remain.
- The broker spends durably before calling the gateway. Only an explicit
  refusal marked `changed: false` restores a fresh once grant, through
  `PermissionRestored`, once per spend. Unknown outcomes stay spent.
- A device-session precheck refuses predictable failures before a card.
  The real mint rechecks under the operation lock. Paired device ids and a
  failed paired-device lookup refuse; a session cannot impersonate a paired device.

- Eligible device-session cards offer thread and standing consent for the same
  device and exact TTL across current and future machine-made targets.
  The gateway-recorded creator must be the requesting engine's own workspace. The
  typed scope uses existing breadth/event storage, never action-wide null.
  Missing or foreign provenance keeps exact once consent.

  The gateway precheck
  reports eligibility; each lasting mint carries an engine-derived creator
  restriction revalidated under the operation lock. Existing revoke, holder,
  restore-floor and batch restrictions apply. Old cards stay once-only.

## Rationale

The OS policy owns the boundary. Only an enforced sandbox can judge symlink
resolution at open time, so current preferences cannot justify a carve-out for
an older unsandboxed process. Engine stamps keep every answer surface within
the same permission judgment.

The agent's blocking question call keeps the card live until the owner answers.
An Allow once must survive a refusal that changed nothing without authorizing
two acts after an ambiguous response. A unique restore of a durable spend
preserves both properties. The gateway owns the paired-device directory and
therefore makes the identity decision at mint time.

## Consequences

This covers shipped macOS and Linux, source and packaged, moved and second
installs, chat shell/Python foreground/background, and both coding-agent
backends and their file tools. An opted-out, inherited or missing sandbox
snapshot keeps the secret-read floor. Direct/keyless engines cannot mint.

Host-log outcomes steer to `lucidos diagnostic-log query`, never another
spelling of private storage. Chat receives the steer on allow and deny.
Claude Code's MCP denial can carry it, while its allow payload cannot.
Codex's boolean permission bridge carries neither explanation; do not claim
that backend receives the text.

## Alternatives considered

- Classify from current settings: cannot describe the already-running process.
- Widen grants for all gateway paths: exposes private/control storage.
- Derive unstamped scopes in the client: bypasses the engine's judgment.
- Raise batches directly and end the turn: the agent can still overtake the card.
- Spend after success: a lost response can authorize a second act.
- Restore on any HTTP refusal: a status alone proves no absence of a mutation.
- Trust only the device-session precheck: state may change before the mint.
