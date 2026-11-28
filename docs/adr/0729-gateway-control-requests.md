# 0729: Agents use owner-approved gateway control requests

- **Status**: Accepted
- **Date**: 2026-11-17
- **Amends**: 0715 and 0727: channel selection and agent Local publication.

## Context

Agents cannot read the machine-local token. Local publication needs the staging
directory from gateway status, and channel selection needs a gateway control write.
The owner rejected a separate permission action for each gateway operation.

## Decision

A *gateway control request* uses one action, `control-gateway`. An engine route
table permits exactly `GET /gateway/status` and `PUT /release-check` initially.
Reads need no card. Changes need a spent Allow once for the exact method, path
and canonical body on the calling thread. The action cannot be proposed through
`lucidos ask-permission`; the typed engine route builds the card.

The engine contacts only its own gateway, at the port supplied at spawn.
It adds the local token internally and redacts it from every returned body.
Unknown routes, fields and methods are refused. Credential routes never join
the table, including pairing, device sessions and the workspace broker.

The CLI uses the token directly when readable. Otherwise an agent uses its
engine and receives a request id, with exit status 3, when approval is needed.
An explicit gateway port is refused on that path.

Channel selection validates the machine-global setting against the machine.
Beta and Local are accepted if this install supports them or the install
inventory finds a packaged desktop app. Nightly and unknown custom slots remain
refused. Local publication stays in shell, with every existing verification gate.

## Rationale

One permission expresses the trust boundary without minting actions for each
operation. A reviewed route table bounds that permission. Exact requests keep
Allow once narrow, and contacting only the known gateway prevents token theft
by a listener squatting on another port.

## Consequences

Agents in any workspace can read gateway status and request a channel change.
Local and Beta still require a supporting install on the machine. Local
publication also requires filesystem write access to the Lucidos home.
The owner's shell retains its direct path. No release or publish is authorized
merely by adding this capability.

## Alternatives considered

- **Act-specific permissions:** rejected by the owner; they do not scale across operations.
- **Expose the token:** breaks the agent credential boundary.
- **Unbounded proxy:** makes credential and owner routes reachable.
- **Probe for the packaged gateway:** could send the token to a squatter.
- **Card Local publication:** publication only stages bytes; selecting the channel is the protected change.
