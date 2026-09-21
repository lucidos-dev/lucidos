# 0238: An app frame carries a short-lived capability into its own subresource URLs, so it can load its files behind a gateway

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

[ADR 0227](0227-app-frames-get-their-own-renderer-process.md) gave the app frame
an opaque origin. Its site-for-cookies is null, so the browser withholds the
`SameSite=Lax` device credential from every subresource of the frame's document.
Behind a gateway `auth_api::enforce` refuses those requests.

The `/api/v1` half was closed by exempting a handful of assets by name.
`/<slug>/data/*` and `/<slug>/app/<id>/*` were left broken on purpose: an
exemption by name there hands an unpaired device the user's artifacts and an
app's source. `docs/temporary-measures.md` registered the gap as "Inline your
own CSS and JS" advice to app authors. Its removal condition named two routes
and asked for a decision.

Measured cookieless against a running dev gateway, before this change:

| Request | Accept | Answer |
|---|---|---|
| `/dev/data/artifacts/web/lucidos-me/index.html` | `text/html` | 200, 62061 bytes, the pairing shell |
| the same path | `application/json` | 401 `this device is not paired with Lucidos` |
| `/dev/app/site-publisher/` | `text/html` | 200, the same 62061-byte shell |
| `/dev/api/v1/threads/list` | `text/html` | 200, the same shell |
| `/dev/api/v1/sdk.js` | either | 200, 96348 bytes |

Two defects, not one. The gate refuses workspace content an app frame owns, and
the refusal wears the whole Lucidos shell. The Site Publisher app previews a
landing page in a nested iframe. What a user sees there is the blue Lucidos boot
splash reading "Tap to retry", inside their own app.

## Decision

**1. A frame capability, in the URL, as one path segment.** The engine mints a
short-lived pass when it serves an app frame's document, signed with a key both
processes derive from the machine-local token. The gateway verifies it and
strips it before proxying, so the engine's URL space is unchanged.

**2. It reaches `/<slug>/data/*` and `/<slug>/app/<app_id>/*`, and nothing
else.** GET and HEAD, one workspace, one app, one hour. No `/api/v1` route, no
control plane, nothing under the picker's namespace.

**3. The frame's own document carries it in `<base href>`, not in its URL.**
Every relative ref the app writes then resolves through it, with no attribute
rewritten. The engine's rescope threads the same pass onto root-absolute
`/data/` and `/app/` refs, which ignore a base.

**4. The host re-mints at half-life and pushes.** The SDK swaps the token into
the base element. Nothing reloads and no app notices.

**5. A refusal never wears the pairing shell unless it is a real top-level
navigation.** `Sec-Fetch-Dest` decides. A nested frame gets one honest line and
a subresource keeps its 401 JSON.

**6. One implementation, in `lucidos-frame-capability`**, which the engine and
the gateway both depend on.

## Rationale

**A path segment, because relative resolution carries a prefix down every hop
and drops a query.** The nested case is the one that bit us, and it is the one a
naive design gets wrong. An app previews an artifact in an iframe, and that
artifact is an HTML document. Its own `./next/index.html` resolves against its
url, with no query on it. A path prefix survives that for free, to any depth,
with the artifact never rewritten.

An app in this workspace settles it from the other side too. Site Publisher
writes `lucidos.data.url(cfg().src) + '?t=' + Date.now()`. A query-borne pass
turns that into a URL with two `?`.

**`<base href>` for the frame's own document, because the host owns that URL and
because a base can be RENEWED.** The host sets the iframe `src`, so the URL is
fixed before the engine can mint. `history.replaceState` would move a document's
url in place. It throws at an opaque origin on WebKit, which is the iOS PWA and
the packaged desktop client, as the `app-frame-escape-hatches` investigation
measured. A `<base>` write is plain DOM and works in both engines.

It also gives the design one source of truth. The browser resolves the app's own
refs against the base. `lucidos.data.url` reads the same element, so the two
cannot disagree about which pass is current.

**The grant is what the app already had, reachable as a URL.** An app frame
reads the whole `data/` tree through `lucidos.data.read` over the host bridge
(ADR 0231). The pass adds no reach. It adds a shape: bytes a browser can load as
a tag, which a bridge cannot carry.

**HMAC off the machine-local token, because both processes already read it.**
The gateway mints that file before it spawns any engine, mode 0600. So the two
agree with no handshake, no shared state, and nothing to lose when either
restarts. The key is derived with domain separation, so a leaked pass is not a
step towards the token.

**The shell defect is separate and lands on its own.** `wants_html` reads a
nested iframe navigation as a page load, because an iframe navigates. Rendering
the whole Lucidos shell inside somebody's app reads as the app being possessed.
It happens on every install with a gateway in front, which is every packaged
install and every remote-access URL. `Sec-Fetch-Dest` is browser-set and
unforgeable from script, so it tells the address bar from a frame.

## Consequences

- An app ships a separate `style.css` again, and `lucidos.data.url(path)` works
  behind a gateway. The inlining advice comes out of the knowhow.
- A nested preview's relative links work, to any depth, with the artifact served
  byte-for-byte off disk.
- **A pass is a bearer token in a URL, and the URL is visible to any document
  the frame embeds.** Script in a previewed artifact can read `document.baseURI`
  and send it elsewhere. Bounded by: read-only, two sub-trees of one workspace,
  one hour, and a grant the app already holds over the bridge. Accepted, and
  stated here rather than left for a reader to notice.
- **A url captured at load time keeps the pass it was born with.** A renewal
  moves the frame's `<base href>`, which is what the DOCUMENT resolves against.
  Three things resolve against something else, and each keeps its original pass:
  a nested document (its own url), a linked stylesheet's `url()` (the
  stylesheet's), and a dynamic `import()` inside an ES module (the module's).
  So a preview left open past the hour loses its in-page navigation, and a
  module that lazily imports a chunk an hour on is refused. The refusal is a
  loud 401 rather than a wrong answer. `lucidos.data.url()` is normally called
  at render time, so an app that re-renders is unaffected.
- `lucidos.data.url('system-knowhow/…')` routes through `/api/v1/data/…` and
  stays refused behind a gateway. Widening `/api/v1` would put the whole engine
  API behind a token the frame hands out.
- An app that declares its own `<base href>` gets no pass and loads as it does
  today. The first base in a document wins, so injecting ours would override a
  choice the author made.
- A standalone app tab gets nothing. It is a top-level document on a real
  origin, so the cookie still reaches its subresources.
- Direct to an engine nothing changes, and the served bytes are identical. There
  is no device gate there to pass.
- An app id that is not URL-safe mints nothing, because the gateway compares the
  path segment as raw bytes rather than carrying a percent-decoder.
- The gateway gains one dependency it did not have, and a second shared crate
  beside `lucidos-local-token`.

## Alternatives considered

**A query parameter.** The obvious cheap answer, and it fails the case this
exists for: a nested document's relative refs resolve with no query. It also
collides with an app appending its own query to the result, which one app in
this workspace already does.

**A cookie the engine sets on the served document.** Rejected on three counts. A
cookie is host-scoped rather than per frame, so it would reach the shell and
every sibling app. It needs `SameSite=None; Secure` to survive a null
site-for-cookies, which hands the workspace's artifacts to any third-party page
that can name a URL. And `Secure` rules out a plain-http packaged install.

**Propagating the pass on the rewriter's output.** The engine would rewrite each
nested document's relative refs the way the WIP preview already appends
`?thread_id=`. It misses CSS `url()`, `@import`, `srcset` and every URL built in
JavaScript, so a previewed landing page renders without its background. It also
means parsing and editing the user's artifacts, and `/data/` is a static mount
that should hand back what is on disk.

**A separate port, or a separate hostname.** Priced and rejected in ADR 0227,
and nothing has changed. Both need port plumbing, a CORS policy and a device-auth
story for an origin whose storage starts empty.

**Exempting `/<slug>/data/*` by name, the way the `/api/v1` assets are.** That
is the thing the temporary measure refused to do: it hands an unpaired device
the user's artifacts and an app's source for a guessed URL.

**A per-boot key the engine registers with the gateway.** It would invalidate
every pass on a restart, which is a real property. It needs a registration hop,
in-memory state on the gateway, and an answer for an engine the gateway adopted
rather than spawned. The file both already read costs none of that.

**A Playwright spec against the real gateway binary.** The binary refuses to
boot from a coding-agent worktree, deliberately and with no opt-out
([ADR 0021](0021-long-lived-stack-never-runs-from-a-worktree.md) § "the opt-out
stops at the gateway"). That guard was not widened for a test. `chain_tests.rs` covers
the chain instead, by binding the real gateway router in front of the live e2e
engine from a test process. It registers nothing, adopts nothing, spawns no
engine and dies with the test, so it is not the daemon the guard exists for.
