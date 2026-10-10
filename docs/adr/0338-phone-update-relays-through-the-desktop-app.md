# 0338: A phone can update the desktop app through a relay to the running client, which runs the install it already has

- **Status**: Accepted (supersedes [ADR 0190](0190-mobile-update-route-is-a-toast.md)
  in part: its "one answer on a phone" decision and its rejection of a remote
  install)
- **Date**: 2026-10-01

## Context

ADR 0190 gave a phone one answer about updating: do it on the desktop app. It
listed "let a phone trigger the install remotely" as out of scope, "a project
rather than a route". It leaned on ADR 0108, where a gateway that runs the
installer kills itself: `launchctl bootout` tears down the whole process group
of the job, installer included.

That objection is about an installer the **gateway** spawns. The macOS install
does not work that way. `install_app_update_and_restart` runs inside the
**desktop client**, which sits outside the service job. It downloads the signed
bundle, swaps the `.app`, restarts the service and relaunches itself. And the
client is usually resident in the menu bar from login onward.

So the detached helper ADR 0190 priced in already exists. It is the client.

## Decision

**A session that cannot install gets a new update route, `relay`, when a desktop
client is attached and can install unattended.** Following it sends a request to
the gateway. The client picks it up on its next heartbeat and runs the same
install a click on the Mac runs. Without an attached client the route is what it
was: `desktop` on a phone, `guide` elsewhere.

- **The client's Rust side carries the relay,** never its webview. It sends a
  heartbeat every 5 seconds with its version and its *remote install blocker*,
  and posts each progress phase back.
- **Attached means a heartbeat in the last 15 seconds.**
- **A remote install blocker** is anything that would stop or stall an install
  with nobody at the Mac: the existing disk-image and cross-device checks, plus
  a bundle folder this user cannot write, which raises an admin prompt.
- **A request is handed out once,** and expires unclaimed after 2 minutes.
- **The phone proves success by the version.** It holds its own in-flight
  marker, because the relay state dies with the service restart. After it
  reconnects, the gateway's running version must be newer than the one running
  at the request, and at least the release it asked for.

## Rationale

The install has to run in the client, for ADR 0108's reason. The client is also
the one process that survives the restart it causes. A relay through it adds a
request and a progress feed, and no new installer.

**No new authorization.** A paired device already holds full authority over the
gateway's control routes: it can restart, stop and delete workspaces. Updating
the app to a signed, published release is no wider than that. The client's
heartbeat accepts only the machine-local token. That is the
existing local-process authority, and it stops a phone from posing as the
client.

**The webview cannot carry it.** At login the window is hidden and shows the
picker, and a hidden WKWebView is throttled and can be suspended (ADR 0180).
The client's Rust poll of the gateway runs with no window at all.

**Never a prompt on an empty desk.** A local click can raise the admin password
dialog, because a person is there to answer it. A relayed install would hang on
it, so the client refuses up front and re-checks when it picks the request up.

**The route stays total** (ADR 0142). `relay` is offered only when the
heartbeat says it would work. Every other state keeps its old answer, so no
surface names a release without a route that works.

## Consequences

- A phone shows **Update Desktop App** on What's New and on Settings, System,
  Overview, when a client is attached. So does a desktop browser session.
- ADR 0190's suppression stands: a phone still raises no offer toast and no
  update dot. The control sits where the reader goes on purpose.
- The gateway learns, for the first time, whether a desktop client is running.
- The relay state lives in gateway memory. A gateway restart that is not the
  update itself loses a pending request, and the phone reports "the update did
  not run" rather than success.
- The restart is attributed to the client's own device, not the phone's. That
  still reads as a user-initiated switch, so coding-agent and chat threads
  auto-resume.
- A headless install gets no relay. It has no client, and ADR 0108's objection
  applies to it in full.
- A failed bundle swap reaches the phone with its full recovery message (ADR
  0073), and the service keeps running the version it loaded.

## Alternatives considered

**Keep ADR 0190's one answer.** Rejected. It was right while a remote install
needed a new detached helper. The helper exists, so the cost that justified the
answer is gone.

**Relay through the hidden webview.** Rejected. The webview already listens for
progress, so it looks cheaper. But at login it shows the picker, not a
workspace, and a hidden WKWebView can be suspended. A relay that works only
while a window happens to be awake is not a route.

**Have the gateway run the install.** Rejected for ADR 0108's reason:
`launchctl bootout` kills the installer part way through.

**Persist the relay state to disk.** Rejected as unneeded. The phone already
holds the request and the target version, and the running version settles the
outcome. A file would add a second authority for a state that lasts minutes.

**Add a confirmation step or a narrower credential for the request.** Rejected.
A paired device can already restart and delete workspaces. A tier that guards
only this action would protect nothing the device cannot already do.

**Offer `relay` on phones only.** Considered, and rejected in favour of every
session that cannot install. A desktop browser on another machine has the same
need, and one rule is simpler than two.
