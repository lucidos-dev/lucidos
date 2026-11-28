---
name: Workspace audit: triggers
description: The triggers audit section. Procedure in intent, stale run.knowhow, subscriptions on retired event names, slugs, orphaned trigger knowhow, notification taps. Unlisted MCP servers a trigger uses. Grants: coding agents started without one and commands outside an approved program scope.
sources: ["docs/taxonomy.md", "crates/lucidos-engine/src/triggers/definition.rs", "system-knowhow/triggers.md", "system-knowhow/triggers/*.md", "system-knowhow/running-python.md", "system-knowhow/running-python/*.md", "system-knowhow/thread-events.md", "system-knowhow/thread-events/*.md", "system-knowhow/coding-agent-events.md", "system-knowhow/coding-agent-events/*.md", "system-knowhow/plugins.md", "system-knowhow/plugins/*.md", "system-knowhow/js-sdk.md", "system-knowhow/js-sdk/*.md", "system-knowhow/best-practices.md", "system-knowhow/best-practices/*.md", "crates/lucidos-agents-api/src/engine/chat/process/system_prompt.rs", "crates/lucidos-core/src/engine/thread_events/*", "crates/lucidos-engine/src/scheduler/notifications.rs", "crates/lucidos-agents-api/src/engine/agent_session/spawn_gate.rs"]
---

# Workspace audit section: triggers

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

Command permission events may carry a decision trace.
That optional payload does not change event names or triggerability.

A `BatchItemRan` receipt for `workspace_deletion` covers the named group.
Check its outcome before counting any of the group as moved to Trash.

Frontend preview storage prefixes affect browser app state, not trigger execution or permissions.

## Parts

This section is split into parts so each one fits a coding agent's tool output. To run the section, load every part below, in order.

| Part | Covers |
|---|---|
| `system-knowhow/workspace-audit/triggers/scan` | Scan; Checks |
| `system-knowhow/workspace-audit/triggers/checks` | Remediation (Listing an MCP server a trigger uses; Granting a trigger coding agents or a program; Rewriting an old-form `tap`); Diagnostic evidence |
