# 0157: Every proxy arm that carries a credential passes one scope gate, and insecure transport is opt-in

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

ADR 0144 decision 4 says every place the proxy pipeline resolves a credential
checks `credential_base_url_matches` against the outbound URL. It was
implemented as a call per arm inside `proxy_pipeline_builder`, and a nightly
review found two arms that never made it.

**A `script_handshake` layer with no credential was bound to no host.** The
check sat inside `if let Some(name) = credential`. The credential-less form is
documented and supported: the script sources its own secret from a keychain or
an OAuth exchange. It mints a live token, and that token followed whatever
`base_url` the entry carried. `config/` is in `MUTABLE_PREFIXES`, so a hostile
app UI or a prompt-injected agent rewrote the base URL, kept the script, and
received the token.

**The builtin providers built their layers outside the builder.**
`proxy_handle_inner` called `dispatch_with_layers` directly with pre-built
layers, so the gate never ran for any of them. `local` is the one that matters:
`local_base_url` is an ordinary settable Global/Text preference, and
`resolve_local` reads it as the upstream and attaches the `local` credential (or
`LUCIDOS_LOCAL_API_KEY`) as `Authorization: Bearer`. Write the preference,
receive the key.

**A third hole meant the check would not have helped anyway.** The shared proxy
client set `danger_accept_invalid_certs(true)` for every provider, and nothing
stopped a credentialed provider sitting on plain `http://`. The finding was
filed for the local-device and self-signed-dev case and closed `wontfix` on
volume. Validation was disabled globally rather than per provider, so an on-path
attacker captured API keys for ordinary public APIs. Severity is a property of
the product across all users, never of one machine's setup.

**A fourth arm had the same shape.** `vertex_region` is also a settable
Global/Text preference with no format check, and `vertex::vertex_host`
interpolates it into a host string. `vertex_region = "evil.test#"` builds
`https://evil.test#-aiplatform.googleapis.com/...`, whose host is `evil.test`,
and `resolve_vertex` mints a live Google ADC access token and sends it there.

## Decision

**1. One gate, held by a type.** `api::proxy::ScopedPipeline` carries the
layers, the outbound base URL and the transport. Its only constructor runs the
gate, and `dispatch_scoped` takes nothing else. Both arms pass it, and an arm
added later cannot reach the network without one.

**2. Every layer declares what binds it.** `AuthLayer::scope_bindings` is
required, with no default body, so a new layer has to answer. Three variants:

- `StoredCredential`, bound by the credential's own `base_url`.
- `HandshakeScript`, bound by the scope recorded against the script.
- `Pinned`, bound by an upstream the engine chose from an input no API caller
  can write.

**3. A minted handshake token is bound in the approvals record.**
`.lucidos/approved-handshake-scripts` already keys a script by path and SHA-256,
outside anything the data API can write. Each line gains a `base_url` column.
The boot seed fills it from `apis.json` for a workspace upgrading. A script
recorded with no scope binds on the first call that would use it, announced as
`HandshakeScriptScopeBound`. That mirrors `infer_scope_if_empty`, which
ADR 0144 already blesses for the credential half of the same problem.

**Only bytes that would actually run may bind.** The bind re-checks the file on
disk against its recorded hash. Otherwise a caller swaps the script out, points
the entry at their host, and lets the call the runner refuses persist that host.
Restoring the approved bytes then collects the token.

**4. A builtin key is bound by where the key came from.** A stored credential is
bound by its own `base_url`. A boot pass infers one for an unscoped `local`
credential, from the resolved local base URL. `LUCIDOS_LOCAL_API_KEY` has no row
to scope it, so process env names the host too: `LUCIDOS_LOCAL_BASE_URL`, or the
built-in default. The `local_base_url` preference cannot speak for an
env-supplied key. Every other builtin is pinned to a compile-time constant or to
the engine's boot-resolved Vertex prefix.

**5. `vertex_host` refuses a region that is not `[a-z0-9-]+`**, falling back to
`global` with a loud log. The clamp sits at the derivation, so it covers the
preference, the env var and any writer added later.

**6. Insecure transport is an explicit per-provider opt-in.**
`"insecure_transport": true` on an `apis.json` entry, default off. One flag, one
meaning: the engine will not ask this upstream to prove who it is. It accepts an
invalid certificate, and it lets a credential travel over plain `http://`.
**Loopback is exempt from the plaintext half**, with no opt-in needed. Both
conditions are logged at boot and carried in one notification.

## Rationale

**Three per-arm checks is what failed.** The rule was right and its
implementation was a convention, so each new arm had to remember. Two did not.
Moving the check to the dispatch site, and making that site take a proof, turns
the convention into something one function enforces. The required trait method
is the other half: a new layer that says nothing does not compile.

**Binding a minted token needs a record, and only one place qualifies.** No
stored credential speaks for a token the script mints, and `apis.json` is the
thing the attacker writes. The approvals record is already the engine's answer
to "an API caller must not decide this". So the scope belongs beside the hash,
not in a second file answering the same question.

**Trust on first sight, again.** Refusing an unbound script would be
fail-closed and defensible. It would also break a script authored before this
change. The ordinary authoring order writes the script first, so authorship has
no `apis.json` entry to read. The first request decides, announces, and is
enforced from then on. ADR 0144 took the same trade twice, for the seed and for
`infer_scope_if_empty`.

**Loopback is where the line goes.** The prompt behind this change asked for
plain `http://` to need the opt-in and for the local-device workflow to keep
working. Both hold only if loopback is exempt: no attacker sits on the path to
`localhost`, and a process that could listen there already has the uid. A LAN or
tailnet address is not loopback and does need the flag, which keeps the rule one
sentence long.

**One flag, not two.** `accept_invalid_certs` and `allow_plaintext` would be two
names for one admission: this upstream never proves who it is. A single
`insecure_transport` is what an operator actually decides, and it cannot end up
half set.

## Consequences

- **A `local` credential scoped elsewhere stops working, and says so.** The boot
  pass scopes an unscoped one to the resolved local base URL, so the common case
  is silent. A row scoped to a different host is a row the user set that way.
- **`LUCIDOS_LOCAL_API_KEY` plus a `local_base_url` preference pointing
  somewhere else now answers 502.** The message names both routes back: store a
  `local` credential with that base URL, or export `LUCIDOS_LOCAL_BASE_URL`. The
  same pairing in the LLM provider is untouched, and is named below.
- **A self-signed dev backend needs one line of config.** It stops working until
  the entry sets `insecure_transport`, and the 502 says exactly that.
- **A handshake script serving two hosts is refused after the first.** The
  recovery is a hand edit of the record's `base_url` column, which the file
  header documents. There is no button and no CLI flag, for the reason
  ADR 0144 gives about buttons.
- **A workspace whose approvals file predates this change binds on first use**
  rather than at boot, because the seed runs once and already ran. The window is
  between the upgrade and the first proxy call.
- **`llm::provider_build::build_local_provider` still has the finding.** It
  pairs the same preference with the same env key, outside the proxy pipeline.
  Rewriting `local_base_url` therefore sends that key, and every prompt, to the
  attacker's host. Named here deliberately: the fix belongs to provider
  construction, and folding it in would widen a proxy change into a fourth
  subsystem.
- **An unparseable `base_url` is refused at the gate**, rather than compared
  against and silently matching nothing.

## Alternatives considered

- **Patch the three sites.** Rejected, and it is the shape that produced the
  bug. It leaves the next arm to remember, and the ADR that told it to was
  already in the tree.
- **Refuse a credential-less `script_handshake` outright.** Fail-closed and
  cheap. Rejected: the form is documented and supported, and the secret it
  sources is exactly the kind no credential row holds.
- **Key the handshake scope by provider name rather than script path.**
  Rejected. An attacker then adds a second provider naming an already-approved
  script, and the new name binds fresh to their host. The script path is the
  thing that mints, so it is the thing to bind.
- **Record the scope at authorship, from `apis.json`.** Rejected as the only
  mechanism. The documented flow writes the script first, so there is nothing to
  read, and the entry can be written in either order.
- **A separate `.lucidos/handshake-scopes` file.** Rejected. Two records about
  one script, both answering "who says so", is worse for a reader than a third
  column.
- **Two flags, `accept_invalid_certs` and `allow_plaintext`.** Rejected under
  decision 6.
- **Exempt RFC1918 from the plaintext rule as well as loopback.** Rejected. An
  on-path attacker on a shared LAN is a real threat, and "loopback" is a line
  that needs no qualification.
- **Validate `vertex_region` at the preference write instead.** Rejected as the
  only guard: `VERTEX_REGION` reaches the same code without passing a
  preference, and the catalog governs only the agent's tool. The derivation is
  the one place every writer meets.
- **A new `SystemEvent` for the insecure-transport warning.** Rejected. It is a
  boot observation rather than a state change. Nothing is subscribed to SSE that
  early, so the notification is the half a user actually sees.

## Amendment (2026-08-29): the handshake arm binds what goes IN, not the host

Decision 2 gave a `script_handshake` layer two bindings: `HandshakeScript` for
the token it mints, and `StoredCredential` for the credential its entry names.
The second one was wrong, and it broke every OAuth handshake on upgrade.

**A handshake credential does not travel to `base_url`.** The engine injects it
into the script's environment. The script presents it to the provider's own
token or exchange endpoint, and only the header it mints goes upstream. So
`credential_base_url_matches` judges a request that never happens.

The failure is not an edge case, it is the normal shape. An OAuth client is
scoped to `oauth2.googleapis.com` while its API sits on `www.googleapis.com`. A
Firebase web API key is scoped to `identitytoolkit.googleapis.com` while the
database it unlocks sits elsewhere. Both were refused.

ADR 0144's safety valve could not catch it either. `infer_scope_if_empty` fills
only an empty column, and every one of these rows already had a scope. Even
empty, `credential_scopes` declines when two entries name one credential with
different upstreams, which is what a shared OAuth client always looks like.

**What replaces it.** `ScopeBinding::HandshakeInjects` binds the set of secrets
an entry may hand a script. It lives in the `injects` column of
`.lucidos/approved-handshake-scripts`, on exactly the terms decision 3 gives the
scope. Members are `c:<credential>` and `o:<provider>`.

That is stricter than what it replaces. The old check let an `apis.json` rewrite
name any credential scoped to the same host; the new one lets it name none. It
also covers `oauth_providers`, which decision 2 left unbound on the reasoning
that the script approval stood behind it. The script approval fixes the code,
not the secrets handed to it, so that half is now bound as well.

**Injecting nothing is not a state.** An entry with no credential and no
providers declares no binding and records none. No secret moves, so there is
nothing for a record to protect.

`check_credential_scope` is unchanged everywhere the secret itself travels: the
static layers, `hmac_signed`, and the WASM signer's handles.

## Amendment: a credential's scope is a set, not one base URL

Decision 2 says `StoredCredential` is bound by "the credential's own
`base_url`". The binding is unchanged; the column is not. It is now
`base_urls TEXT[]`, and the gate asks whether ANY member covers the outbound
URL.

The single value could not express one key serving several hostnames of one
provider, which is the ordinary shape at Binance and Helius. Every member is
still judged by `credential_base_url_matches`, so no member covers more than it
did before.

[ADR 0161](0161-a-credential-scope-is-a-set-of-base-urls.md) is the record, with
the wildcard alternatives it rejects and the two surfaces that now edit the set.

The `base_url` column of `.lucidos/approved-handshake-scripts` is a different
field and stays single-valued, so the consequence above about a handshake script
serving two hosts still holds.
