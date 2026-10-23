# 0404: AppDeleted is a persisted event carrying the deleted app's kind

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

The thread filter lists every app a coding-agent thread worked on, including
removed ones. Widgets (ADR 0402) are app folders that the apps list never
holds. A removed widget must still list as a widget. Its kind lived only in its
manifest, which leaves with the folder.

`AppCreated`, `AppUpdated` and `AppDeleted` were all broadcast-only list
refresh hints. The git commit was the durable record of an app change.

## Decision

`AppDeleted` is persisted and carries `kind: Option<AppKind>`, read from the
manifest before it goes. The `app_kinds` projection keeps that kind, and the
thread filter facets read it for an app whose folder is gone.

`AppCreated` and `AppUpdated` stay broadcast-only.

## Rationale

A projection written in the emit transaction can be rebuilt from events, which
is the engine's model (`repo_names` from `RepositoryAdded` is the precedent). A
removal is also a real state change worth a durable row, as artifact events
already are.

## Consequences

- App removals appear in the event history and can match event triggers.
- A removal from before this change has no recorded kind. Such a widget lists
  under Apps in the filter, marked deleted.
- Every `AppDeleted` site must read the manifest before deleting it.

## Alternatives considered

- **A bus subscriber writes `app_kinds` from the broadcast.** No event change,
  but the table cannot be rebuilt from events. A crash between emit and write
  also loses the kind.
- **Read the kind from the deleted manifest in git history.** No event or
  schema change, and it covers past removals. It costs about 250 ms per removed
  app on a 35,000-commit workspace, once per engine start.
- **Persist all three app events.** Create and update fire often while an app
  is built, and nothing needs their history.
