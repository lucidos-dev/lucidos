# 0048: The deep-link mechanism is an ordinary HTTPS URL into the engine's own URL space; a `lucidos://` scheme is not it, and is not thereby rejected: one scheme cannot name one instance out of several, but an OS-level handoff stays open

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

Every notification Lucidos sends is tappable, and the tap has to land somewhere
specific: an inbox modal, a thread scrolled to one event, an app, a settings
sub-page, an external URL. That routing is already built and shipped. The
`send_notification` tool takes a structured `tap` of `{"kind": "modal"}` or
`{"kind": "navigate", "to": {...}}`, where `to` is the same argument shape the
`navigate_ui` tool accepts, so a tap can reach any surface the agent could
navigate to. `crates/lucidos-engine/src/scheduler/push.rs` turns that into two
URL forms carrying identical parameters: an absolute query URL built from the
subscription's stored service-worker scope (`https://<host>/<slug>/?notification=
…&thread=…&event=…&tap=…`), which iOS Safari's declarative-push handler navigates
an open window to, and a hash form inside `data.navigate` that feeds the Chrome
service worker's cold `clients.openWindow()` path.

None of that was ever written down as a decision, and the alternative is the
obvious one to reach for. A custom `lucidos://` URL scheme is what a desktop app
normally registers for exactly this job, and until this record the string did
not occur anywhere in the tree, so nothing explained why it was absent. A
contributor adding the next deep-linked surface had no record telling them the
question was settled.

The prompt to record it came from the product-philosophy review
(`docs/philosophy.md`), whose draft listed the custom scheme among the things
its lens rules out. **That was wrong, and the correction is why this ADR is
scoped the way it is.** A scheme handler routes the user into *our own client*,
so the lens does not decide it either way; the draft's stated reason (it reaches
only desktop installs) was a cost argument filed under a philosophy heading. The
real arguments are structural, they belong in a decision record rather than a
bullet on a philosophy page, and they settle a narrower question than the draft
claimed.

So this ADR decides **the deep-link mechanism** and nothing more. It does not
reject a `lucidos://` scheme as a category, and § *What stays open* below names
the one job a scheme could still do.

## Decision

**Where a deep link has to cross a process boundary as text, it is an ordinary
HTTP(S) URL addressed to a running engine through its gateway prefix.** There is
no custom URL scheme, no `Info.plist` `CFBundleURLTypes` registration, no Linux
`.desktop` MIME handler, and no Universal Links / App Links association.

The thing being carried is one structured target (`tap`, plus the notification,
thread and event ids), and every transport hands it to the same router,
`dispatchDeepLink`. There are three carriers, and none of them is a scheme:

| Carrier | How the target travels | Client |
|---|---|---|
| Declarative Web Push `notification.navigate` | absolute query URL built from the subscription's service-worker scope | iOS Safari / PWA |
| Service-worker `notificationclick` | `postMessage` to an open tab; the hash form in `data.navigate` only for the cold `clients.openWindow()` | Chrome, Firefox |
| `native-notification-tapped` | the structured link itself, stashed at show time and emitted by the `UNUserNotificationCenter` delegate | packaged macOS app |

The native desktop path uses **no URL at all**: the tap is delivered in-process
to a window that is already open on the engine's origin
([ADR 0028](0028-the-packaged-window-is-a-remote-origin.md)). A URL is what the
two web transports need in order to name a destination across a boundary they
cannot reach into. A link pasted into a note, a calendar entry or an email is the
same case as those two, and works the same way.

## Rationale

**A URL scheme is a single machine-global registration, and Lucidos is not a
single instance.** One machine can run several gateways side by side as named
instances, each with its own port, its own data directory and its own service
(`./install.sh --name test --port 5300`), and each gateway fronts many
workspaces addressed by path prefix
([ADR 0014](0014-multi-workspace-redesign.md)). A `lucidos://thread/<uuid>`
cannot say which of them it means. An HTTPS URL says it structurally: the host
and port name the gateway, the `/<slug>/` prefix names the workspace, and the
query names the thread and the event.

**The port is a mutable property, so even a scheme that encoded it would go
stale.** Re-running the installer with a different `--port` moves an instance;
the instance name is the stable identity, the port is not. A handler registration
baked at install time would keep resolving to whatever was true that day.

**Only one of the three shipped shapes could register a scheme at all.** The
macOS `.app` could. The headless tarball, which is the only path on Linux and a
first-class one on macOS, is served to a browser and registers nothing. A PWA
installed from that origin registers nothing either. A deep-link mechanism that
works for one install shape out of three is not a mechanism, it is a special
case for the client that needs it least, given that the packaged window already
loads an HTTP origin.

**The web push transports require an HTTP(S) URL anyway.** iOS Safari's
declarative-push navigation applies a *cross-document* URL to an already-open
PWA window; a same-document hash-only URL is silently ignored, and a custom
scheme is not navigable at all from that context. Chrome's `notificationclick`
path calls `clients.openWindow()`, which takes a URL in the service worker's own
origin. Both are already handled by the two forms `push.rs` emits. Adding a
scheme would mean maintaining a third routing path that no transport can use.

**An HTTPS link degrades honestly.** Opened on a machine with no Lucidos
running, it fails to connect and says so. A custom scheme with no registered
handler does nothing at all, which is the worse failure because the user cannot
tell a broken link from a missing install.

## Consequences

- **The link is only as reachable as the origin it names.** A `localhost` link
  works on that machine; reaching it from a phone needs the same remote-access
  setup as reaching Lucidos at all (an ssh forward, `tailscale serve`, or a bound
  interface with TLS). The engine builds the URL from the scope the client
  actually subscribed with rather than a hardcoded host, so a device gets a link
  in the form it can use.
- **There is no OS-level "open in the desktop app" handoff.** A link opened from
  Mail lands in the browser, not the packaged window, even when both are
  installed. Accepted: they serve the identical origin, so the user sees the same
  page and the same state.
- **New deep-linked surfaces cost nothing new.** Adding one means adding a
  `navigate_ui` target; the `tap` payload and both URL forms carry it without
  change. That is the property a scheme would have taken away, since every new
  target would also need a route in the handler.

## What stays open

**A `lucidos://` scheme as an OS-level handoff into the packaged app is not
decided here, and must not be quoted as rejected.** It is a different proposal
from the one above, aimed at the gap in the second consequence: today a Lucidos
link opened from Mail or a calendar lands in a browser tab even on a machine
where the `.app` is installed. Nothing in this ADR argues against closing that.

The arguments above do constrain the shape any such proposal has to take, and it
is worth writing down what it would have to answer, so the next attempt starts
from here rather than from scratch:

- **Which instance and which workspace does a bare `lucidos://…` mean** on a
  machine running two gateways over several workspaces each? A handoff that
  guesses wrong opens the right thread id in the wrong workspace, which is worse
  than a browser tab.
- **What happens on Linux and on a browser-only macOS install**, where nothing
  registers the scheme? A handoff that silently no-ops for two of the three
  install shapes is a regression against a link that works everywhere.
- **What registers and unregisters it** across install, uninstall, and several
  installed versions, given the registration is machine-global and the instance
  is not.

The likeliest shape that survives those is a scheme that carries no routing of
its own and simply hands a full HTTPS URL to the packaged window, which keeps
the addressing in the URL where it already works. That is a proposal we would
read.

## Alternatives considered

- **A `lucidos://` scheme as the deep-link mechanism itself**, carrying the
  routing. Rejected for the reasons above: it cannot name an instance or a
  workspace, it would go stale when a port moves, and only the macOS `.app`
  could register it. This is the mechanism question only, not the handoff
  question above.
- **Encode the port in the scheme** (`lucidos+5252://`). Rejected: it hardcodes
  the one property the installer treats as mutable, and it multiplies handler
  registrations per instance.
- **Universal Links / App Links.** Rejected as inapplicable, not merely
  unattractive. Both require a public HTTPS domain serving an association file
  that claims the paths, and the origin a Lucidos link names is `localhost` or a
  private tailnet name. There is nothing for a public domain to assert.
- **Serve deep links from a lucidos.dev redirector** that bounces to the local
  engine. Rejected outright: it routes the user's thread and event identifiers
  through a server we run, for a link whose whole point is that it stays on their
  machine. It also fails the local-first principle in `docs/philosophy.md`.
