# 0275: A request the user must answer is a persisted form request, not a transient frame; navigation stays transient

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A user on a heavily swapping Mac reported that credential requests "couldn't open". The agent asked for an API key, was told a modal had been shown, and no form ever appeared.

The credential form, the plugin install and uninstall panels, and the email confirmation all opened from one transient stream frame. The client replaces its stream on every focus, visibility change and health recovery. A frame queued on the old stream, emitted in the gap, or dropped by a lagged broadcast was lost for good. A reload lost it too. Nothing recorded the user's answer either. The OAuth authorization page had the same shape, with a flow waiting 120 s on it.

Plan: `docs/plans/2026-09-24-form-requests-survive-a-reconnect.md`.

## Decision

A request the user must answer is a persisted *form request*. Five variants open one (`CredentialRequested`, `PluginInstallRequested`, `PluginUninstallRequested`, `EmailConfirmRequested`, `OAuthAuthorizationRequested`), each carrying a `request_id`. One `FormRequestResolved { request_id, outcome }` closes it. The client reads the open ones from `GET /api/v1/form-requests/pending` on every stream open. `NavigationRequested` stays transient.

## Rationale

- **Durability belongs to the fact, not the transport.** A persisted row survives a lost frame, a reload, a restart and a lag alike. It also lets the transcript reopen the form, and it records how the request ended. No transport fix does all of that.
- **Individual request variants, one resolution.** The requests stay separate variants, since a wrapper with a `kind` field is the discriminator shape CLAUDE.md bans. The resolution carries nothing kind-specific, so one variant is one fact rather than five copies of it.
- **The sync runs after the stream opens.** The engine persists before it broadcasts, so a request is either in the listed rows or arrives live. Nothing can fall between the two.
- **Navigation is an act-now hint, not a question.** Replaying a stale one on reconnect would move the user's view unasked. The OAuth page is the exception because a flow waits on it, so it moved off `NavigationRequested` onto its own form request.
- **An open form request changes no thread status.** The turn that asked has ended, and `WaitingForUserAnswer` belongs to a blocked loop. The pending list and the transcript row are how the user finds it.

## Consequences

- An unanswered request reopens after a reload, once per page load. A refocus does not re-pop a form the user walked away from.
- A message the user types supersedes the thread's open forms, as it does a permission card. An engine re-entry does not, and the OAuth page is left to its flow.
- Plugin requests and OAuth pages rest on engine memory. Boot expires them. The pending read expires stagings past their TTL. It leaves out a plugin request whose staging is missing but does not resolve it, since a Confirm in flight pops the staging first.
- The event row gains one action: an open form request carries its Open. ADR 0047's ranking still holds, since the row keeps its card weight below `.step-note-card`.
- `CredentialRequested { provider }`, defined but never emitted, is gone. The durable credential request took its name.
- The agent is still not told the outcome. `FormRequestResolved` is persisted, so a later change can feed it to the model.

## Alternatives considered

- **Replay transient frames on reconnect** (a ring buffer keyed by `Last-Event-ID`). Covers a reconnect gap only. A reload, an engine restart, a transcript reopen and "an answered request never reappears" all stay broken.
- **Patch credentials alone.** The plugin panels, the email confirmation and the OAuth page lose their frame the same way, so the same bug would remain four times over.
- **Mark the thread `WaitingForUserAnswer` while a form is open.** It would light the attention badge, but the turn has already ended and the projection's terminal events would overwrite it. It would need a second lifecycle for a state the pending list already answers.
- **A resolution variant per kind.** Four variants for one fact, and four arms in every consumer that pairs a request with its answer.
