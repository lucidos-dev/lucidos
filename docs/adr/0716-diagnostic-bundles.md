# 0716: Owners send sanitized diagnostic bundles in one tap, never raw host logs

- **Status**: Accepted
- **Date**: 2026-11-16
- **Supersedes**: [ADR 0712](0712-owner-approved-host-log-transfer.md).
- **Amends**: ADRs 0667, 0108 and 0139.
- **Amended by**: [ADR 0806](0806-query-gateway-and-client-diagnostic-logs.md).

## Context

Debugging another install needs evidence from the engine, gateway and client,
including when the workspace cannot boot. Raw host logs mix workspaces and
can contain secrets and conversations. Asking users to choose files or
review log contents makes reporting too difficult.

## Decision

The gateway builds one fixed diagnostic bundle from sanitized Lucidos sources
and sends it only after the owner's tap. Destinations are the maintainer's
encrypted diagnostic drop, a local or paired workspace, or save to file.

The consent sentence names the recipient and whether conversation text stays.
There are no tiers, item lists or review steps. A Settings tap authorizes
sending; an agent needs a once-only `send-diagnostic-bundle` grant that the
owner resolves directly. Triggers and schedules never send bundles.

## Rationale

The gateway runs when an engine cannot boot and can collect each component's
own diagnostic log. Typed values make removing conversation text deterministic
([ADR 0717](0717-typed-log-values.md)). No runtime guess decides which text
is safe for the maintainer.

The workspace-call path already supports local and paired destinations.
Strangers use the diagnostic drop without pairing or exchanging secrets.
Each receiver controls its own inbox and reactions through existing triggers.

## Consequences

- Engine, gateway and desktop client write capped, sanitized diagnostic logs
  with UTC RFC 3339 timestamps. Panic hooks and stderr tees capture failures.
- The gateway uses a small shared crate; it never links the engine (ADR 0014).
- Bundles include versions, health, internal faults, recent failure events,
  crash leftovers and a config summary naming configured features without values.
- Maintainer bundles remove private values, untyped line text and event message
  text. Owner-workspace bundles retain sanitized private text.
- The compressed cap is 32 MiB. Trim oldest lines first and record trim counts
  and missing sources in the bundle manifest. Never split an upload into parts.
- Failure events cover seven days, with at most 2,000 rows. Logs merge into
  one ordered timeline.
- age encryption targets a maintainer key list pinned in the release.
  The diagnostic drop receives only ciphertext, retains it for 30 days,
  and accepts at most ten uploads per address per hour.
- A report code identifies an upload. Listing and downloading require a
  maintainer drop token; each fetching workspace keeps its own cursor.
- Received bundles live in Postgres, outside git, for 90 days or until deleted.
  Direct arrival and fetch emit exactly one persisted `DiagnosticBundleReceived`
  per bundle. Agents read through bounded queries, never raw files.
- `DiagnosticBundleSent` records destination, report code, size and sources.
  The sender stores no bundle copy.
- Panics, crashes, boot failures, task crashes and Lucidos's own 5xx responses
  emit `InternalFaultRecorded`, deduplicated by kind until the next report.
  Provider, network and rate-limit failures never raise the count.
- Settings shows the fault count and **Send diagnostics**. Boot-failure and
  crash-loop screens offer the same entry point.
- ADR 0667's workspace-local agent query and secret-read floor stay intact.
  Gateway and client diagnostics join a bundle only through owner consent.
- ADR 0712's host-log transfer routes, broker and CLI verbs retire together.
  `transfer-host-logs` becomes a retired permission action.
- `PRIVACY.md` must state that diagnostics leave only when the owner taps Send.
  The update check stays the only automatic request to a server we run.
- The drop runs as a Pages Function with R2, built in the site workspace.
  Publish from the maintainer's machine (ADRs 0031 and 0108).

### Reachable population

Every shipped install: the macOS desktop app and macOS/Linux headless tarballs,
including journald-only hosts. Sources come from Lucidos's own paths and registry.
History before the new logs starts is unavailable. Without outbound HTTPS,
users can save to file or send locally. Windows has no shipped build.

## Alternatives considered

- **Raw host-log export.** Secrets and other workspaces' text cannot leave
  safely through fixed consent. Sanitized sources replace the need for SFTP.
- **Per-item choices or a review step.** Adds decisions users cannot assess
  and makes reporting harder.
- **Automatic reporting.** Violates the requirement for the owner's tap.
- **Pairing with the maintainer.** Grants full trust to solve a narrow upload.
- **A receiver secret pasted by the sender.** Adds setup before reporting.
- **GitHub, email or chat as the inbox.** Adds service-specific accounts and
  transports instead of using the receiver's workspace and trigger.
- **An engine-built bundle.** Cannot report a workspace that will not boot.
- **Multipart uploads.** Capped sources and oldest-first trimming keep one upload viable.
