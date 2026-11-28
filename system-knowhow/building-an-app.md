---
name: Building an App
description: Use when the user wants to build, scaffold, edit, or extend an app: "make me an app", "build a tracker", "dashboard for X", "habit app". Also covers widgets (small inline answers), motion, live data. Covers when an app is the wrong fit, what to clarify before scaffolding, and how to iterate.
---

<!-- domain-event: FaresRefreshed: Example domain event emitted by a fare-data refresh trigger. -->

# Building an App

How to guide a user from "I want X" to a working Lucidos app. File layout, SDK, and frontmatter rules live in `system-knowhow/best-practices.md`, `system-knowhow/js-sdk.md`, and `docs/taxonomy.md`. Load those when you need them.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/building-an-app/when-an-app-is-the-right-answer` | When an app is the right answer; Widgets: a small answer inline in the thread; Questions to settle with the user before creating |
| `system-knowhow/building-an-app/scaffolding-defaults` | Scaffolding defaults; App icon; Responsive by default: a phone is a first-class target (The host paints over a fullscreen app: keep three regions clear) |
| `system-knowhow/building-an-app/visual-quality-bar-make-it-look-sleek-an` | Visual quality bar: make it look sleek and native to Lucidos (Smell test before finishing: self-apply every time); Interaction primitives: use the shell's, not the browser's; Motion that explains; Live data (The events a file change emits; Some writes announce nothing; One `render()`, called from every place the data can change; Your own write; Outside data: a trigger keeps it fresh) |
| `system-knowhow/building-an-app/updating-an-app` | Updating an app (Editing an app with a coding agent; Checking your work with `capture_app` / `refresh_app`); Common mistakes to avoid |
