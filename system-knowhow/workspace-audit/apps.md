---
name: Workspace audit: apps
description: The apps audit section. SDK boilerplate and what an isolated app frame can no longer do. Escaping, theme names and hand-drawn controls. Choices drawn as buttons, touch hover, the ready signal and widget manifests.
sources: ["system-knowhow/plugins.md", "system-knowhow/plugins/*.md", "system-knowhow/js-sdk.md", "system-knowhow/js-sdk/*.md", "system-knowhow/building-an-app.md", "system-knowhow/building-an-app/*.md", "system-knowhow/best-practices.md", "system-knowhow/best-practices/*.md", "packages/lucidos-sdk/*", "crates/lucidos-app/src/styles/global/shared-components.css", "crates/lucidos-app/src/styles/global/surface.css", "crates/lucidos-app/src/styles/global/text-input.css", "crates/lucidos-agents-api/src/api/sdk.rs", "crates/lucidos-agents-api/src/api/sdk_iframe.css", "crates/lucidos-agents-api/src/api/sdk_iframe_control_theme_parts.css", "crates/lucidos-agents-api/src/api/sdk_iframe_shapes.css", "crates/lucidos-app/src/components/apps/appFrameSandbox.ts", "crates/lucidos-agents-api/src/api/app_reach.rs", "crates/lucidos-app/src/store/actions/app-bridge.ts", "crates/lucidos-core/src/core/apps.rs", "crates/lucidos-app/src/components/apps/appFrameReveal.ts"]
---

# Workspace audit section: apps

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Parts

This section is split into parts so each one fits a coding agent's tool output. To run the section, load every part below, in order.

| Part | Covers |
|---|---|
| `system-knowhow/workspace-audit/apps/scan` | Scan; Checks |
| `system-knowhow/workspace-audit/apps/checks` | Remediation (Rewriting an app for an isolated frame; Opting an app into the ready signal; Giving an app its icon; Replacing a control the app draws itself; Renaming the board's old names); Diagnostic evidence |
