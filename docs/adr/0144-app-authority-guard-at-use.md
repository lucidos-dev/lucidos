# 0144: App authority is accepted; the engine guards at the point of use, not the point of write

- **Status**: Accepted
- **Date**: 2026-08-27

## Context

A security review flagged a write-then-execute chain as critical. Three
unauthenticated calls walk it:

1. `PUT /api/v1/data/scripts/auth/x.py` lands a Python file. `MUTABLE_PREFIXES`
   in `api/data_api.rs` allows `scripts/`.
2. `PUT /api/v1/data/config/apis.json` adds a `script_handshake` entry naming
   that file. The same list allows `config/`.
3. `GET /api/v1/proxy/<entry>/x` runs it. `resolve_proxy_target`
   (`api/proxy.rs`) re-reads `apis.json` on every proxy call, so no restart is
   needed. `run_handshake_script` then spawns `python3` as the engine user with
   `HOME` set.

**The escalation is an app UI, and only an app UI.** A process already running
as the same uid writes the file straight to disk and skips the engine, so the
chain gives it nothing. An app is browser code with no other route to the host.
The network attacker is gone: ADR 0096 put every engine on loopback, and
`scripts/lib/workspace.sh` stopped widening the bind for directly-launched
engines.

**A second attack needs no script at all.** An entry naming any stored
credential, with `base_url` pointing at an attacker host, makes the engine
attach that credential and forward it. `StaticHeaderLayer`
(`api/proxy_static_layers.rs`) never checks the credential's own scope, and
`GET /api/v1/credentials` lists the names. That is silent credential theft
through an `apis.json` write.

**Four doors reach those two folders**, not one:

- `PUT /api/v1/data/*path`.
- `POST /api/v1/data/edit`, which splices a provider in without a whole-file PUT.
- Plugin install. `PLUGIN_CONTENT_DIRS` in `core/plugins.rs` includes `scripts`,
  and an app can drive upload, stage and confirm itself.
- Asking the agent, whose file tools run in process.

**No header separates an app from the shell.** ADR 0117 established this for the
credential reveal routes. The enumeration is short and every entry fails:

| Lever | Why it fails |
|---|---|
| A secret held by the shell | `allow-same-origin` lets app JS read `window.top`, the shell's variables, its storage |
| A marker stamped into the app | Only an honest app wears it. A hostile one omits it and reads as the shell |
| `Referer` or `Sec-Fetch-Dest` | `window.top.fetch` issues the call in the shell's own realm |
| CSP on the app document | The top document's CSP governs `window.top.fetch`, not the iframe's |

The one real lever is `allow-same-origin` itself, and dropping it costs the
same thing a separate origin costs.

## Decision

**1. An app UI carries the user's authority over `/api/v1`, by design.** A
finding that reduces to "an app can do what the user can do in the workspace" is
accepted, not a defect. A review may cite this ADR and stop there.

**What stays a defect** is anything converting that authority into **code
running on the host** or **a secret leaving to a third party**. Those cross the
machine and the network, not the app boundary. This ADR closes one of each.

**2. Apps stay same-origin, deliberately.** The integration is built on it.
`packages/lucidos-sdk/src/_storage.ts` reads the theme, font, ligature setting,
UI scale, style overrides, device id and scroll memory from the shell's own
`ws:<slug>:<key>` keys. The *appearance boot contract*
(`packages/lucidos-sdk/src/appearance.ts`) names four surfaces that must paint
the same values at one instant. One of them is the parser-blocking app-iframe
script the engine serves at `/api/v1/sdk-prefs.js`. On another origin that
storage is empty, `postMessage` lands after first paint, and every app open
flashes the wrong theme.

So ADR 0014's distinct-origin residual is **not** a plan. Anyone revisiting it
should know two things this thread established. A subdomain cannot work: Route B
is `tailscale serve` against one MagicDNS name, with one certificate for that
exact name, so only a second port is available. And one shared app origin would
be enough for us, since our boundary is apps against the shell rather than apps
against each other.

**Every gate therefore has to work without telling an app from the shell.**

**3. A handshake script runs only with recorded authorship.** The runner
refuses any script whose path and SHA-256 are absent from
`<workspace>/.lucidos/approved-handshake-scripts`. The record sits outside
`data/`, so no API caller can write it.

Authorship, never assertion. Two writers record: the engine's in-process file
tools, when they write under `data/scripts/`, and `lucidos handshake approve`
through a route that refuses a browser-shaped caller. The Lucidos Agent gets no
approve tool, so an app cannot launder a script through it. To get one blessed
it would have to make the agent author that content, which is the same as making
the agent run code.

**4. A credential is presented only inside its own scope.** Every place the
proxy pipeline resolves a credential checks `credential_base_url_matches`
against the outbound URL: the static layers, `hmac_signed`, the WASM signer's
handles, and the `CRED_*` injection into a handshake script. A credential with
no recorded scope has one inferred once, from the `apis.json` entry that names
it. The inference is announced to the workspace, and enforced from then on.

**5. Writing stays open.** `scripts/` and `config/` remain writable through
every door they are writable through today, the Files panel included.

## Rationale

**Guarding at use is what the mature systems do, and it is the only rule that
covers all four doors.** A write lock cannot: plugin install is a browser
feature, so barring browser writes would take it down with the chain.

- **direnv** refuses to run a `.envrc` until `direnv allow`, keyed on path plus
  content hash, recorded in `$XDG_DATA_HOME/direnv/allow`. Their reason for that
  location is ours: a cloned repo must not ship its own pre-approval.
- **Jupyter** keeps an HMAC signature database outside the notebook tree and
  strips HTML and JavaScript from any notebook that is not in it. Output your
  own session produced is trusted automatically, which is where decision 3's
  authorship rule comes from.
- **WordPress** (`DISALLOW_FILE_EDIT`) and **Home Assistant** (whose frontend
  cannot write `configuration.yaml`) instead restrict the write. Both lack a
  use-time gate, so it is the only lever they have.

**Decision 4 makes two halves of this codebase agree.** `core::git_auth`
already re-checks `credential_base_url_matches` on every git credential
callback, so a redirect cannot carry a secret to a host the user never scoped
it to. The proxy pipeline is the sibling that never asked. `find_by_url`
likewise ignores an unscoped credential, so treating scope as load-bearing is
established here, not invented.

**Inferring a missing scope, rather than refusing it.** Refusing is fail-closed
and would be defensible. It also breaks a working proxy entry on upgrade, for a
user who did nothing wrong. The rows affected are legacy or hand-made: every
other way of saving a credential names a host. The inference reads the config as
it stands, announces what it did, and leaves the user able to correct it in
Settings.

**Amended by the `secret` type**, which is signed with rather than sent and so
names no host by design. An empty scope is that type's answer, not a gap, so the
inference skips it: see `CredentialStore::infer_scope_if_empty`. The sentence
above once read that `request_credential` had always required a `base_url`.
It no longer does, for exactly that type.

**The handshake runner is the only place the engine executes a file from
workspace data.** Every other `Command::new` in the engine spawns a fixed
binary, such as `git`, `python3` for the agent's own tools, or `pg_dump`. The
rest run a script from the engine's own checkout, resolved through
`crate::paths::script`. So one guard covers the category, not just one bug.

## Consequences

- **A handshake script edited outside Lucidos stops running until approved.**
  Editing `data/scripts/auth/foo.py` in the Files panel or in vim changes the
  hash, and the next proxy call answers 502 naming `lucidos handshake approve`.
  Asking the agent to make the edit records it with no extra step, which is the
  prompt-first route and needs no terminal.
- **The Files panel says so at edit time.** Open a handshake script whose hash
  no longer matches and a banner names both routes back. The rule surfaces where
  the edit happens, rather than as a later 502 inside some app.
- **There is no in-product approve button, and there cannot be one.** Any button
  is an assertion an app can make. Decision 2 is what forecloses it.
- **Existing handshake scripts are seeded once** at first start after the
  upgrade, from what `apis.json` currently names, and the workspace is told. A
  workspace already carrying a planted script would have it blessed. That is
  accepted: the alternative breaks every working install.
- **An app can still break things.** It can scribble into `apis.json` and
  `scripts/`. The proxy stops working until the file is fixed, the write is in
  the timeline, and git holds the previous version.
- **An app can still spend a credential at its real host** through the proxy,
  which is what the proxy is for. No gate can tell that call from a legitimate
  one.
- **ADR 0117's residual stands, and is now permanent.** A hostile app reaches
  credential plaintext through `window.top.fetch`. Two things make it narrower
  than the route this ADR closes: it emits `CredentialRevealed`, and it needs
  the deliberate `window.top` trick rather than an ordinary SDK call.
- **App documents carry no CSP**, so an app can send anything it reads anywhere.
  Named here as a follow-up, deliberately not fixed in this change: locking
  `connect-src` reaches every app document and would break any app that calls a
  public API directly. **Taken up by [ADR 0156](0156-app-boundary-is-egress-not-identity.md)**,
  which makes egress the boundary and phases the rollout so nothing breaks
  blind.
- **The packaged install needs no terminal for the common path.** The agent
  writes and the record follows. The CLI is for a hand edit.

## Alternatives considered

- **Drop `scripts/` and `config/` from `MUTABLE_PREFIXES`.** Rejected, and the
  brief's reasons for rejecting it turned out to be half wrong, which is worth
  recording. The CLI does *not* need them: `DATA_PREFIXES` in
  `crates/lucidos-cli/src/data.rs` is `artifacts/`, `knowhow/`, `apps/`,
  `triggers/`, and anything else is rewritten under `artifacts/`. The documented
  agent workflow does not need them either, since the file tools run in process.
  The real cost is the Files panel, which can edit any text file under `data/`.
  It loses two folders for a rule that still misses plugin install.
- **Refuse a browser-shaped write to those two folders.** Structurally sound,
  unlike a `Referer` rule, because page JavaScript cannot suppress `Sec-Fetch`
  headers. Rejected because it takes the Files panel's editing of `apis.json`
  and handshake scripts with it, and still leaves plugin install open. The same
  mechanism survives in one narrow place: the approve route in decision 3.
- **Refuse an app `Referer`, as ADR 0117 did.** Rejected. That ADR is explicit
  that a deliberately hostile app defeats it, and a deliberately hostile app is
  the whole threat here.
- **A narrow rule for plugin install** ("may not overwrite a script `apis.json`
  names"). Cheap and shippable, and it closes today's second door. Rejected as
  the primary fix. It is a rule about one door. The next feature writing into
  `data/` reopens the chain, with nothing in the code saying why it must not.
- **Approve every handshake script explicitly, including one the agent wrote.**
  Matches direnv exactly. Rejected: it adds a step to the middle of the
  documented workflow for an actor that already has `run_python`.
- **Serve apps from their own origin.** The root-cause fix for the whole class,
  including ADR 0117's residual. Rejected in decision 2 on the integration cost,
  with the port-not-subdomain finding recorded for anyone who reopens it.
- **A native confirmation the engine brokers out of band.** Would genuinely
  close it, since an app cannot answer for the user. Rejected for the reasons
  ADR 0117 already gave: the headless install has no such surface.
- **Sandbox the `python3` child.** Rejected. A handshake script legitimately
  needs the network and a credential. A sandbox would take away most of that,
  and the platform work is per-OS.
