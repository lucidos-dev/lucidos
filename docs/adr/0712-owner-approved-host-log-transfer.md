# 0712: Owners may authorize one exact host-log transfer without opening agent access

- **Status**: Superseded by [0716](0716-diagnostic-bundles.md)
- **Date**: 2026-11-16

## Context

An owner asked an agent to copy host logs to another machine for a crash
investigation and explicitly allowed the transfer. The agent could reach the
destination, but its OS sandbox hid the source logs. ADR 0667 intentionally
provides sanitized per-workspace diagnostics instead, and there was no
permission by which the owner could authorize this particular transfer.

Increasing diagnostic retention does not fulfill that request or recover
history that has already disappeared. Disabling the entire agent sandbox
would give every agent much more authority than the owner asked to grant.

## Decision

Add a once-only `transfer-host-logs` permission for one thread, one host-owned
log manifest and one SSH destination. The privileged host performs that
specific transfer after the real owner grants it. The agent remains confined
and receives only bounded metadata, integrity information and status.

The source is the installation's recognized log capture and rotation files,
never an arbitrary agent-supplied private path. The permission card names the
source files and exact destination and warns that raw host logs may contain
secrets and text from multiple workspaces. An operation cannot change its
source manifest or recipient after approval. Neither a thread-wide nor a
standing permission is offered.

## Rationale

The sandbox's authority belongs to the owner, but an ordinary tool permission
cannot remove an inherited OS sandbox. A specific host operation can honor
owner consent without changing protected paths. The agent gains no general
access to the control plane.

Existing permission events remain authoritative. An agent asks for the
operation; only the owner's actual permission resolution grants it. An agent
cannot turn a free-form claim that permission was given into an owner event.
The ordinary resolver may interpret the owner's words against the pending
permission request, just as it does for other owner-controlled acts.

Consent must bind both ends. Restricting source files alone still lets an
agent redirect a permitted export. Restricting a server alone still lets it
include neighboring keys. The source snapshot and destination form one
operation, with durable status and no destructive overwrite or blind replay.

## Consequences

- ADR 0667's agent-facing diagnostic endpoint stays sanitized, bounded and
  workspace-local. Raw host-log reads stay blocked to agent processes.
- This is an explicit owner-controlled export exception to ADR 0667's
  exclusion of host logs, not a new query input or an agent filesystem grant.
- The host operation must not echo file contents, transport output or
  credential-bearing errors into events, tool results or application logs.
- The transfer fails closed without owner consent, recognized capture files,
  trusted host keys, a usable SSH transport or host authority outside an
  inherited sandbox. It never turns off the sandbox to make progress.
- Captured files are bounded and private, selected without following links,
  and transferred to a fresh destination with integrity verification.
- Permission spending, source selection and status survive engine restart.
  A transfer whose remote outcome is unknown is reported as uncertain.
- macOS and Linux file-backed captures can participate independently of the
  installation layout. Missing capture history and journal-only sources
  receive explicit limitations rather than a claim of complete evidence.

## Alternatives considered

- **Leave the owner unable to allow it.** Does not fulfill an authorized
  diagnostic task and makes a deliberate security boundary look like an
  unexplained product refusal.
- **Increase sanitized log retention.** Useful for future investigations,
  but unrelated to authorizing the requested host-log transfer.
- **Grant general raw-file reads or disable sandboxing.** Exposes private
  siblings and other agent activity, exceeding the requested operation.
- **General unsandboxed command permission.** Gives a command interpreter
  substantially more authority than copying the named log files.
- **Reuse the confinement-test runner.** That permission authorizes a
  particular test run, not arbitrary owner operations.
- **Copy raw logs into workspace artifacts for an agent to send.** Makes raw
  contents accessible to other workspace readers and can put secrets in git.
- **An ordinary permission with an unbound recipient.** Can be redirected
  after the owner approves and is therefore insufficient consent.
