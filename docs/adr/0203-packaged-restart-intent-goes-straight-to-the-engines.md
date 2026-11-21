# 0203: The packaged client announces a restart intent to each engine directly, not through the gateway

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A thread interrupted by an engine restart auto-resumes only when the teardown
`ResponseAborted` carries a device actor. That actor is a *restart intent*
(`docs/glossary.md`), stashed before the signal and spent at teardown. Until
this change two things could stash one, and both are the engine's own gateway:
the in-workspace *Switch to new version*, and the gateway picker's Restart or
Stop.

The packaged macOS install reaches its engines by neither route. Every restart
a person clicks there ends at `desktop::restart_service`, which runs `launchctl
kickstart -k`. launchd SIGTERMs the service supervisor, the supervisor SIGUSR1s
the gateway, and then the supervisor SIGUSR1s each engine pid itself. Nothing on
that path carries a device, so a user taking an app update got the crash
treatment for something they deliberately did.

Two clicks reach it, not one. The workspace UI's *Switch to new version* and
*Restart Engine* take a packaged branch straight to the `restart_service`
command, never touching `/api/v1/restart`. The in-app updater calls the same
function after the bundle swap.

## Decision

The **packaged client** announces the intent itself, immediately before the
launchctl call, by POSTing `/api/v1/internal/restart-intent` on each running
engine's own loopback port. It enumerates the engines exactly as
`stop_workspace_engines` does, and names the device from its own durable
slug-to-device-id store.

`stop_service` ("Quit and Stop Background Service") does the same, for the same
reason: a person clicked it.

## Rationale

The client is the process that owns this teardown and knows the device, so it is
the honest announcer. The gateway is neither here. It did not initiate the
restart, it is not told one is coming, and it dies in the same teardown.

The endpoint already exists and does exactly one thing: stash an actor and
answer 204. Reusing it means the engine learns nothing new. The two shipped
announcers therefore stay indistinguishable downstream, which is the property
`stash_first_restart_actor` and both resume gates are built on.

Going direct also satisfies the endpoint's own guard for free. A restart-intent
that arrived through the gateway proxy is refused, because a page on the gateway
origin could otherwise set an engine's restart actor. A loopback POST from the
client carries no `x-forwarded-prefix`, so it is accepted while that refusal
stays exactly as strict.

## Consequences

- Every packaged teardown a person clicked now settles `paused` with "Paused by
  restart" and auto-resumes, matching the dev *Switch to new version*.
- A teardown nobody clicked is untouched. That covers a crash, a launchd respawn
  of a dead service, the supervisor's health respawn, `stop.sh`, and a bare
  external `SIGUSR1`. Each keeps System attribution and a manual Continue.
- The client learns two things about a workspace layout it did not read before:
  the `.lucidos/ports` file, and the pidfile. The enumeration is shared with
  `stop_workspace_engines` so the set we attribute cannot drift from the set the
  teardown signals.
- A workspace this client has no device id for is skipped. Absent attribution
  stays absent, which is what keeps the loop-safety guarantee honest.
- The announce cannot be withdrawn. It runs immediately before the launchctl
  call, so only launchctl itself failing can strand a stash, and that case is
  logged. The residual is bounded: a stale device actor and the device actor it
  would refuse give the same verdict.
- The client speaks plain HTTP only. It has no HTTP client crate, so the scheme
  is read from the ports file and a TLS engine is skipped rather than guessed
  at. A packaged engine always serves plain HTTP, because the packaged gateway
  strips `LUCIDOS_TLS_*` from what it spawns.

## Alternatives considered

**A gateway control route that fans out to every stack.** The gateway holds the
ports, the schemes and `notify_restart_intent` already, so this reads like the
tidy option. It loses on authentication and on lifetime. A call from the client
authenticates as `LocalProcess`, never as a device. The route would therefore
have to accept a claimed device id anyway, which is no stronger than the direct
POST. It also adds a hop that is torn down by the very teardown it is
announcing.

**Let the page call a gateway route before invoking the Tauri command.** The
page is authenticated as a real device, so the attribution would be the
gateway's own rather than a claim. Rejected on timing. The only safe moment to
announce is immediately before the launchctl call, and that is inside the Rust
command. An announce made earlier can be abandoned by a cancelled or failed
update, and a stash nobody spends is worse than no stash: under
first-writer-wins it also refuses the next restart's actor.

**Fan out from the gateway's own SIGUSR1 handler.** The gateway is signalled a
few seconds before the engines are, so it could announce on its way out. That
signal carries no device either, so it would still need a stash from somewhere.
The gateway also leaves its engines running on its own stop, deliberately. This
would be the one place it spoke for a teardown it is not doing.

**Pass the page's device id into the Tauri commands.** Workable, and it needs no
new file reads. Rejected because a restart takes down every workspace rather
than the one the page is on. The client already holds a per-workspace map that
survives a reinstall, so reading it attributes them all and changes no frontend
code.
