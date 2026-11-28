---
name: Notification Routing
description: How Lucidos decides whether a notification reaches the user as an OS push, an in-app toast, a bell-badge increment, or is silently auto-marked-read: the two surfaces, device presence states, and the PresenceCheck protocol. Load for "why didn't I get a push on my X" or cross-device noise.
---

# Notification Routing

This is the canonical spec for what happens when a `NotificationCreated` event is emitted. It says who gets an OS push, who gets an in-app toast, whose bell badge changes, and what gets auto-marked-read. The rules apply to every notification source: `send_notification` from the chat LLM, scheduler errors, trigger output and push-from-trigger.

Section 5 names the current test files and the behavior each one verifies.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/notifications/1-conceptual-model` | §1: Conceptual model (Advising a user on enabling notifications: read the device first; Remote workspace web push) |
| `system-knowhow/notifications/2-the-fan-out-rule` | §2: The fan-out rule |
| `system-knowhow/notifications/3-the-presencecheck-protocol` | §3: The PresenceCheck protocol; §4: The in-app surface (page-local) |
| `system-knowhow/notifications/4-the-in-app-surface-page-local` | §4: The in-app surface (page-local) (continued) |
| `system-knowhow/notifications/4-the-in-app-surface-page-local-2` | §4: The in-app surface (page-local) (continued) (A notification also clears once you have looked at what it points at; Native desktop OS surface (Tauri)) |
| `system-knowhow/notifications/4-the-in-app-surface-page-local-3` | §4: The in-app surface (page-local) (continued) (App-icon badge (Badging API + native dock tile / menu-bar tray)) |
| `system-knowhow/notifications/4-5-service-worker-wedge-mitigation-stac` | §4.5: Service worker wedge (mitigation stack) (Layer 1: Declarative Web Push envelope + `launch_handler`; Layer 2: Liveness probe + recovery (full-wedge coverage)) |
| `system-knowhow/notifications/4-5-service-worker-wedge-mitigation-stac-2` | §4.5: Service worker wedge (mitigation stack) (continued) (Layer 3: Engine-scheduled wake-push (partial-wedge coverage, proactive); Net behaviour; Troubleshooting: push tap works on one device but not another (stale cached manifest)) |
| `system-knowhow/notifications/4-5-service-worker-wedge-mitigation-stac-3` | §4.5: Service worker wedge (mitigation stack) (continued) (History; User-only message recovery); §5: Verification (Engine and API; Frontend; Browser); §6: What this spec doesn't cover; §7: Implementation notes; Diagnostic evidence |
