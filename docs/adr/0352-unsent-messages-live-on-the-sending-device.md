# 0352: An unsent message lives on the device that sent it; the engine draft never holds a copy

- **Status**: Accepted
- **Date**: 2026-10-03

## Context

An *unsent message* is a send that got no answer. It shows as a **Not sent**
card with **Retry**. It lived only in page memory, so a reload lost it, and an
iOS PWA reloads constantly through eviction. A send cut off mid-POST was lost
the same way, with no card at all.

A thread's first message had a partial net. The engine kept the thread's
compose draft untouched until a send was accepted, so a reload brought back
whatever the last debounced write had stored: often a prefix of the message.
A follow-up had no net at all.

Plan: `docs/plans/2026-10-03-unsent-messages-survive-a-reload.md`.

## Decision

Every send is kept in a per-workspace IndexedDB store on the sending device,
from before its POST until the engine takes or refuses it. A record found at
startup comes back as a Not sent card with Retry. The engine's compose draft
holds what the composer shows, for a first send as for a follow-up, and never
a copy of an unsent message.

## Rationale

- **One copy, one place.** The record carries the exact request body and its
  client event id. Retry re-posts it, and the engine's repeat guard (*accepted
  messages*) runs it once even if the first POST landed. A second copy in the
  engine draft would come back beside the card after a reload, so the text
  would show twice.
- **The engine cannot tell an unsent send from a landed one.** A first send
  stored as the draft has to be fenced against the send having landed after
  all. The compose epoch fence cannot tell that submission apart from any
  other: a follow-up from this device, or a send from another device. So the
  copy is dropped exactly when it might still be needed.
- **A settlement is data.** A sender may owe something on a later decision:
  consume a first send's picks, roll its draft back, or take a follow-up into
  the draft. Each depends only on plain values. So `SendSettlement` names the
  kind of send, and a record kept across a reload settles exactly as the page
  that sent it would have.

## Consequences

- An unsent message is visible only on the device that sent it. Another device
  sees neither the card nor a draft holding its text.
- Storage that fails (no IndexedDB, quota) is said once per page. The send
  carries on in memory, and a reload would then lose it.
- A persisted card needs a way out that is not sending it, so the card has
  **Discard**.
- A message whose thread was deleted on another device comes back as a fresh
  draft on this one, with a toast, rather than being lost.

## Alternatives considered

- **Keep the first send in the engine draft, fenced at the pre-send compose
  epoch.** Built on a branch and not merged. It covers first sends only. It
  loses the copy whenever another message reaches the thread first. And once
  the card itself survives a reload, it shows the text twice.
- **`localStorage` instead of IndexedDB.** Synchronous, which is attractive.
  But one key shared by every tab is a cross-tab read-modify-write race, and
  per-record keys cannot be listed through the per-workspace storage shim.
- **Retry automatically after a reload.** Out of scope: the user presses
  Retry, as before, so nothing is sent that the user is not looking at.
