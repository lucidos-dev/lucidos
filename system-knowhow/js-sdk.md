---
name: Lucidos JavaScript SDK
description: API reference for the `lucidos` JS SDK. Namespaces: data, events, proxy, apiUrl, oauth, triggers, apps, preferences, storage, notifications, threads, ui, sse. Also the component classes and theme variables apps style with.
---

<!-- domain-event: ReportOpened: Example domain event emitted when the app opens a report. -->

<!-- domain-event: HabitCompleted: Example domain event emitted by a habit-tracker app. -->
<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

# Lucidos JavaScript SDK

The SDK is available as the `lucidos` global in app UIs (loaded via `<script src="/api/v1/sdk.js">`). The host frontend imports it from the `@lucidos/sdk` package directly.

> From a coding-agent subprocess, prefer the `lucidos` CLI for `data.*` and `events.*` operations. See [`lucidos-cli.md`](./lucidos-cli.md).

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/js-sdk/remote-workspace-client-contract` | Remote workspace client contract; Setup (Text fields and autocorrect; Reduced motion; Theme parts; Pull to refresh; Pane swipe) |
| `system-knowhow/js-sdk/setup` | Setup (continued) (Dynamic bars; One sound at a time; Find in app; Theme variables) |
| `system-knowhow/js-sdk/setup-2` | Setup (continued) (Component classes; Widget shapes); Error Handling; lucidos.configure: SDK endpoint; lucidos.data: File Operations (Types; Examples; `url` and app-bundled assets) |
| `system-knowhow/js-sdk/lucidos-media` | lucidos.media; lucidos.events: Event Store (Types; Examples; Paging with `before_event_id` / `after_event_id`); lucidos.proxy: Call External APIs (Configure the backend (one-time); Examples; Built-in model-provider proxies (no `apis.json` entry needed); When to use which) |
| `system-knowhow/js-sdk/lucidos-apiurl` | lucidos.apiUrl (Why a hand-written `/api/v1/…` does not work); lucidos.request: an endpoint no namespace covers (Not every endpoint is reachable); lucidos.oauth: OAuth Token Access (When to use; Example: Spotify Web Playback SDK; Errors; Security note) |
| `system-knowhow/js-sdk/lucidos-triggers-scheduled-tasks` | lucidos.triggers: Scheduled Tasks (Types; Subscribing to multiple events from one trigger; Cron validation on create and update); lucidos.apps: App Management (Types; Example); lucidos.params: Widget Params (Example); lucidos.widget: Where a Widget Shows (Example) |
| `system-knowhow/js-sdk/lucidos-preferences-user-settings` | lucidos.preferences: User Settings (Types; Common keys); lucidos.priceTable: Model Prices (Types; Example); lucidos.storage: Per-Device App State; lucidos.notifications: Notification Center (Types; Tap shapes: examples) |
| `system-knowhow/js-sdk/lucidos-threads-thread-management` | lucidos.threads: Thread Management (Types; When to use which); lucidos.threads.users: Driver and Watchers; lucidos.modelRequest; lucidos.ui: UI Control |
| `system-knowhow/js-sdk/lucidos-ui-ui-control` | lucidos.ui: UI Control (continued) (Showing the app once its content is ready; Sizing a widget; Reading the thread the user has open; Navigation targets; Showing a cited file without leaving your app; Opening a link outside Lucidos) |
| `system-knowhow/js-sdk/lucidos-ui-ui-control-2` | lucidos.ui: UI Control (continued) (Starting a fresh chat with a prefilled prompt; Confirmation dialogs; Toasts; Prompts; Tooltips; lucidos.ui.ModelPicker: model, effort and provider; lucidos.ui.Select: themed dropdown) |
| `system-knowhow/js-sdk/lucidos-sse-real-time-events` | lucidos.sse: Real-time Events (Hearing the stream open: `onOpen`; One stream per workspace; A quiet stream is reopened; Types; Examples); lucidos.utils: Utilities (Escaping: which helper goes where); App UI Pattern; Diagnostic evidence |
