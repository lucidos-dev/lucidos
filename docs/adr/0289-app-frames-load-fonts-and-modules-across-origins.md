# 0289: An app frame may load fonts, and modules behind the gateway, across its opaque origin

- **Status**: Accepted
- **Date**: 2026-09-26

## Context

[ADR 0227](0227-app-frames-get-their-own-renderer-process.md) gave the app frame
an opaque origin. Everything it loads is therefore cross-origin to it, including
its own files. Most loads do not care: a stylesheet, an image and a classic
`<script src>` are no-cors requests. Two kinds of load are always CORS-mode: a
font, and a `<script type="module">`. Chromium refuses either one when the
response carries no `Access-Control-Allow-Origin`. WebKit loads a font anyway.

So every app frame in Chromium rendered `--font-mono` in the system fallback
instead of Fira Code, the default UI font. Nothing on screen said so. Measured
with the shell's exact sandbox against a live gateway:

| Browser | `fira-code.css` | `fira-code-6.2.woff2` | `document.fonts` |
|---|---|---|---|
| Chromium, Chrome 151 | 200 | 200, refused by CORS | not loaded |
| WebKit | 200 | 200 | `Fira Code:loaded` |

Both gates already passed the request without a credential. The gateway exempts
`/<slug>/api/v1/fonts/*` and the engine's same-origin gate exempts
`/api/v1/fonts/*`. Only the response header was missing, and a preflight got 405.

An app's OWN bundled font and its own module script failed in exactly the same
way, from `/app/<id>/…` and `/data/…`.

## Decision

1. **`/api/v1/fonts/*` answers with `Access-Control-Allow-Origin: *`**, and
   answers a preflight for `GET` and `HEAD`.
2. **`/app/*` and `/data/*` answer with `Access-Control-Allow-Origin: *` for
   a `font` load, and for a `script` load the gateway forwarded.** A grant
   carries `nosniff`. Every response on those two trees carries
   `Vary: Sec-Fetch-Dest`.

No other route gains a CORS header.

## Rationale

**The fonts are public bytes.** Both gates already treat them as "the same bytes
for every caller", and they ship on the public mirror. `*` forbids a
credentialed read, and the frame sends no credential. The header grants nothing
that was private.

**The app's own files are not public, so a plain `*` is out.** Direct to a
loopback engine `/data/` has no gate at all. `*` there would let any web page
`fetch()` the user's artifacts.

**`Sec-Fetch-Dest` separates a load from a read.** The browser sets it, and
page script cannot. A font load hands the page no bytes. A `fetch()` is `empty`
and never gets the grant.

**A module script needs a gate in front.** An `import()` hands its caller the
module's exports, which a classic `<script src>` of an ES module never could.
Direct to a loopback engine nothing gates `/data/`, so any web page could read
the exports of the user's modules. So `script` is granted only when the request
carries `X-Forwarded-Prefix`. A page cannot set that header on a load, so it
means the gateway forwarded the request after checking the device or the frame
capability. A non-browser client can forge it and gains nothing, since CORS
never bound such a client.

`style` stays out: a CORS-granted stylesheet exposes its rules through the
CSSOM, which is a read. `image` stays out for the same reason, through a canvas.

**A granted answer also carries `X-Content-Type-Options: nosniff`.** A classic
`<script crossorigin>` is a `script` load too, and the grant lifts the
browser's "Script error." mask on it. Without `nosniff`, a browser runs a
`.txt` or `.md` file as a script, and the full `SyntaxError` quotes the file's
own words to the page. With it, only a file served as JavaScript runs, which a
classic script could already do.

**`Vary: Sec-Fetch-Dest` is what makes the destination rule hold.** Measured in
Chrome against a server that granted only `font`: a page loaded a file as a font
and then `fetch()`ed the same URL. Without `Vary`, Chrome served the fetch from
the cached font response, grant included, and the page read all 113088 bytes.
With `Vary`, Chrome and WebKit both refused. So the refused answer carries the
`Vary` too.

**Behind a gateway the frame capability still decides admission**
([ADR 0238](0238-app-frame-carries-a-capability-to-its-own-files.md)), and the
gateway relays the engine's headers. Direct to an engine the destination rule is
the whole gate, which is why `script` waits for the gateway.

## Consequences

- Every app frame renders in Fira Code again, in every browser.
- An app may ship its own `@font-face`, and its own ES modules on every
  install. Every shipped install runs a gateway. A dev engine hit directly has
  none, so there an app's module still fails in Chromium.
- An app's own `fetch()` of its own files is still refused, as ADR 0227 intends.
  `lucidos.data.read` is still the way to read.
- A web page elsewhere can use a loopback engine's fonts. It reads no bytes.
- The browser suite runs direct to the engine, so it covers fonts only. The
  gateway chain test covers the module grant.
- A browser too old to send `Sec-Fetch-Dest` gets no grant, which is the old
  behaviour.
- The woff2 keeps its URL. Chrome did not reuse the refused copy it had cached:
  after the fix it went back to the network and loaded the font.
- No local-network preflight is granted (`Access-Control-Allow-Private-Network`).
  No probe produced one, and granting it would let public pages load from a
  loopback engine.

## Alternatives considered

**Route the font through the frame capability.** ADR 0238 decision 2 reaches
`/data/*` and `/app/<id>/*` and names "no `/api/v1` route" as its boundary. A
capability also proves nothing a public file needs. And a capability URL is
itself cross-origin to the frame, so it would still need this header.

**Inline the font as a `data:` URL in the stylesheet.** It puts about 150 KB of
base64 in a stylesheet cached for an hour, where the woff2 is cached for a year.
It also forks the one copy of the bytes the host and the frames share. And it
fixes nothing for an app's own font.

**`Access-Control-Allow-Origin: null` instead of `*` on the app's files.** It
admits every opaque-origin document on the web, which is where a hostile
sandboxed frame lives, so it narrows nothing that matters. The destination rule
is the real boundary.

**Grant `script` everywhere.** It lets any web page import a module from a
loopback engine with no gateway and read its exports. That setup ships nowhere,
but the grant bought nothing there worth the exposure.

**Grant `script` when `Referer` names the engine's own host.** Chrome sends no
`Referer` for a `<script type="module">` tag in a sandboxed frame, so every
static module import would fail.
