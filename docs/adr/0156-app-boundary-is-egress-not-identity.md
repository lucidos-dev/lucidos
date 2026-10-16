# 0156: The app boundary is egress, not caller identity: no app principal is possible while apps stay same-origin

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

A security row asked for an app principal and a capability boundary. Its wording
asked for three things:

- An app-origin request must be identifiable as coming from app `<id>`.
- It must not mutate `config/`, `auth-modules/` or `scripts/`.
- It must be authorized per proxy provider, rather than being allowed to call
  anything the workspace has configured.

The row absorbed three earlier duplicates, so its scope is their union.

Its named chain ran in three calls. Malicious JavaScript in a same-origin app
iframe PUTs a Python file under `data/scripts/`. It PUTs an `apis.json` entry
under `data/config/` naming that file. It then calls the configured proxy, and
`script_handshake` runs the file on the host with `CRED_*` and `OAUTH_*`
injected.

**That chain is closed.** ADR 0144 landed two days before this one and cut it in
two places. `proxy_script_runner` refuses any script whose path and SHA-256 are
absent from `<workspace>/.lucidos/approved-handshake-scripts`, a record no API
caller can write. And every place the proxy pipeline resolves a credential now
checks `credential_base_url_matches`, so an `apis.json` entry pointing at an
attacker host gets no credential attached.

What ADR 0144 did not settle is the row's own questions, and one defect it named
as a follow-up: **app documents carry no policy on where they may send what they
read**. This ADR settles the questions and decides that defect.

## Decision

**1. There is no app principal, and there will not be one while apps share the
origin.** An app-origin request cannot be identified, so no rule may be written
as though it could.

ADR 0144 established this for headers. The sharper form, which matters because
the obvious fix looks obviously right, is about direction:

- A capability the engine puts only in the app document is **unforgeable
  upward**. App A cannot present itself as app B, because it never held B's
  capability.
- Nothing is **unforgeable downward**. App A can always present itself as the
  shell. It runs `window.top.fetch` in the top document's realm and omits the
  capability entirely.

Every rule the row asks for is enforced downward. "An app may not write
`config/`" refuses a request that says it is an app, and a hostile app simply
does not say so. So a per-app capability token buys attribution between honest
apps and nothing at all against a hostile one. It would read as a boundary in
the code and be none, which is worse than the gap.

**2. Writing stays open, and each mutable prefix is guarded at the point of
use.** This confirms ADR 0144's decision 5 and gives the reason in the shape the
row asked about. The guard per prefix:

| Prefix | Guard at use |
|---|---|
| `scripts/` | the handshake approval record: path plus content hash, written only by an in-process file tool or `lucidos handshake approve` |
| `config/apis.json` | credential scope, so an entry naming a credential cannot send it to a host that credential is not scoped to |
| `auth-modules/` | the wasmtime sandbox and its declared host imports, plus the same credential-scope check on every handle |
| `artifacts/`, `apps/`, `knowhow/`, `triggers/` | none needed: they are content, and an app writing them is an app doing what the user can do |

A planted `.wasm` is therefore not host code execution. It runs inside the
sandbox, reaches no filesystem and no network of its own, and gets no credential
outside that credential's scope.

**3. A proxy call is authorized per credential scope, never per app.** This
confirms ADR 0144's decision 4. Per-app authorization needs decision 1's
principal, so it is not available. Scope is the property that actually protects
the secret: it binds a credential to the host it belongs to, whoever asks.

**4. The remaining defect is egress, and egress is where the boundary goes.**
ADR 0144's own line is that authority converted into host code execution or into
a secret leaving to a third party stays a defect. It closed the first. The
second is half closed. A credential cannot be *attached* to an attacker host.
But an app that reads anything through the ordinary API can still send it
anywhere it likes.

Egress is decidable without knowing who is asking, which is exactly why it is
the right boundary. The engine serves the app document, so it sets that
document's `Content-Security-Policy`, and page JavaScript cannot remove it.

The target state is three parts, and all three are needed:

- The app document gets `connect-src 'self'` plus the hosts its app manifest
  declares.
- The shell document gets `connect-src 'self'` plus the gateway origin. Without
  this, `window.top.fetch` walks around the app document's policy.
- The app iframe drops `allow-popups`, `allow-popups-to-escape-sandbox` and
  `allow-forms`, or those channels carry the data out instead. CSP has no
  shipped directive for a top-level navigation, so the sandbox attribute is the
  only lever there.

**5. Nothing silently breaks, so egress lands in three phases.** Apps and
plugins that write data today are untouched by decisions 1 to 3: writing stays
open exactly as it is. Decision 4 is the one that can break a shipped app, and
its migration mirrors the `CredentialScopeInferred` pass ADR 0144 already ships.

- **Phase A, observe.** Serve `Content-Security-Policy-Report-Only` on the app
  document and collect what shipped apps actually call. Nothing is refused.
- **Phase B, declare and enforce on the app document.** Add an outbound-hosts
  field to the app manifest. An app that declares nothing gets `'self'` plus a
  one-time inference from Phase A's observations, announced to the workspace,
  correctable in the manifest. One-time, and never re-run: re-inferring on every
  start would bless whatever an attacker added since, which is the trap
  `seed_if_absent` already avoids.
- **Phase C, enforce on the shell and tighten the sandbox.** This is the part
  that closes `window.top.fetch`. It also has the most ways to break a working
  install, so it lands last and on its own.

## Rationale

**Naming the direction is what makes decision 1 stick.** ADR 0144 enumerated
four levers and showed each fails. That reads as a list of near-misses, and
invites a fifth attempt. Upward-versus-downward explains why there is no fifth:
the property every proposed rule needs is the one property `allow-same-origin`
removes. Anyone reopening this should propose removing `allow-same-origin`, or a
distinct origin, and argue the cost. Nothing else changes the answer.

**Egress is the boundary that survives having no principal.** It is also where
the harm is. An app reading a credential and keeping it inside the workspace is
ADR 0144's accepted case, an app doing what the user can do. An app reading a
credential and posting it to a host the user never chose is theft, and it is
decidable from the destination alone.

**The three parts of decision 4 are one decision, not three.** Shipping only the
app-document policy would be the misleading patch the row warns against. It
would look like the app is contained while `window.top.fetch` runs unpoliced. A
popup carries the same bytes out with no policy involved at all. Splitting the
*delivery* into phases is fine. Splitting the *claim* is not, so no phase before
C may be described as closing egress.

**Report-only first, because the blast radius is every app document.** ADR 0144
rejected a `connect-src` lock on exactly this ground, that it reaches every app
and breaks any app calling a public API directly. That objection is about
landing it blind, not about the policy. Observing first turns an unknown
population into a list.

**Why the inference is one-time and announced.** A default derived from
observation is trust on first sight, the same trade `seed_if_absent` makes for
handshake scripts, and it earns the same guard rail. Re-deriving it later would
let an app widen its own allowance by making one call.

## Consequences

- **The row's named chain is closed, and this ADR records where.** A future
  review citing the write-then-execute chain should be answered with ADR 0144
  decisions 3 and 4, not with new work.
- **The row's requested shape is refused, with a reason.** No app principal, no
  prefix rule keyed on one, no per-app proxy allow-list. Anyone re-proposing one
  is re-proposing decision 1.
- **`config/`, `auth-modules/` and `scripts/` stay writable** from the data API,
  the Files panel, plugin install and the agent's file tools.
- **Egress stays open until Phase C.** An app can still read anything the user
  can read and send it anywhere. That is stated plainly here so no reader
  mistakes the phases for the fix.
- **Phase B adds a manifest field**, so an app that calls a third-party host
  directly gains something to declare. Apps that only use the proxy declare
  nothing.
- **Phase C narrows the iframe sandbox**, which will break an app that opens a
  popup or submits a form to a third party. That population is what Phase A
  measures.
- **ADR 0117's residual and ADR 0144's are unchanged.** A hostile app still
  reaches credential plaintext through `window.top.fetch`, and it still leaves a
  `CredentialRevealed` row. The backup key now leaves a `BackupKeyRevealed` row
  on the same terms.

## Alternatives considered

- **A per-app capability token, delivered into the app document and required on
  the app-facing API.** The proposal the row implies, and the reason decision 1
  is written as a direction rather than a list. It is unforgeable upward and
  worthless downward, and every rule it would carry is enforced downward.
  Rejected.
- **Drop `config/`, `auth-modules/` and `scripts/` from `MUTABLE_PREFIXES`.**
  ADR 0144 rejected it by name: it costs the Files panel two folders and still
  misses plugin install. Nothing has changed. Rejected again here only because
  the row asks for it directly.
- **Refuse a browser-shaped write to those prefixes.** Structurally sound, since
  page JavaScript cannot suppress `Sec-Fetch` headers, and rejected in ADR 0144
  for taking the Files panel with it. Worth restating because it is the one
  proposal in this space that decision 1 does not defeat: "is this a browser" is
  answerable, "which document in that browser" is not.
- **Serve apps from a distinct origin, or drop `allow-same-origin`.** The root
  fix for the whole class, including decision 1. Rejected in ADR 0144 on the
  appearance-boot contract, with the finding that only a second port is
  available under `tailscale serve`. Named here so decision 1 has a stated
  escape hatch rather than reading as permanent physics.
- **Enforce `connect-src` on the app document now, and leave the shell.** Cheap,
  shippable, and misleading, which is the specific failure the row calls out.
  Rejected as a stopping point; it is Phase B of a plan that ends at C.
- **A per-app egress allow-list the user approves at install time.** A better
  end state than a manifest declaration the app writes for itself, since an app
  cannot then widen its own reach. Not chosen now. The manifest field is what
  Phase B needs, and an approval surface is a separate change that should follow
  the handshake-approval precedent.
- **An egress proxy, so every outbound call from an app goes through the
  engine.** Would make egress observable rather than merely policed, and would
  work with no CSP. Rejected on cost and reach. It needs the browser to have no
  other way out, which is CSP again, so it adds to decision 4 rather than
  replacing it.
