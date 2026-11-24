---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos proxy <name> [path] [-X METHOD] [-H "Hdr: val"] [-d body | --data-stdin] [-i] [--fail]`

Call a backend through the engine: an entry in `data/config/apis.json`, or a builtin provider proxy (below). The engine resolves the credential from the workspace's credential store and injects the configured auth header. It strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request. It strips every `x-lucidos-*` header and the two `x-forwarded-*` ones the gateway owns with them, so no Lucidos credential reaches the upstream. **The credential value never reaches the script**: not in `argv`, env vars, the request line, nor any log.

The response comes back with an allowlisted set of upstream headers, the same for every caller. `Set-Cookie` and the other headers a browser acts on never arrive, so `--include` cannot read a login cookie. A cookie login belongs in a `script_handshake`, which makes its own requests. The full list is in `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`.

**This is the preferred way for scripts to call external APIs.** The old pattern, `curl -H "Authorization: Bearer $CRED_FOO" ...` with `$CRED_FOO` in the script's environment, leaks the secret into process args and shell history. Configure the API in `data/config/apis.json` once, then use `lucidos proxy` everywhere.

#### Configure the backend (one-time)

`data/config/apis.json`:

```json
{
  "sonos":   { "base_url": "http://localhost:5005" },
  "comfort": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": { "pipeline": [
      { "type": "static_credential", "kind": "bearer", "credential": "comfort-cloud" }
    ] }
  },
  "weather": {
    "base_url": "https://api.weather.example",
    "auth": { "pipeline": [
      { "type": "static_credential", "kind": "api_key",
        "credential": "weather-api", "header": "X-API-Key" }
    ] }
  }
}
```

`auth.pipeline` is an ordered list of authentication layers. Each layer has a
`type`:

- `static_credential`: chooses `kind` (`bearer`, `api_key`, `basic` or
  `query_param`) and a `credential` name. `api_key` accepts `header`;
  `query_param` accepts `param_name`.
- `hmac_signed`: takes `key_credential`, `secret_credential`, `algorithm`
  (`sha256` or `sha512`) and `signed_payload: "query_string"`.
  Optional fields are `key_header`, `signature_param` and `timestamp_param`.
- `script_handshake`: takes `script`, an optional `credential` and optional
  `oauth_providers`. The script returns cached headers and an expiry.
- `wasm_signer`: takes `module` and optional `credential_handles`, each mapping
  a logical `name` to a stored `credential`.

Credential references name `service_name`s in the engine credential store.
Omit `auth` for an unauthenticated backend. The legacy `auth.type` shape is
retired; startup migration handles old files, but new entries need a pipeline.
See [Building an auth handshake](../building-an-auth-handshake.md) for layer
examples, defaults, secret injection and WASM capability declarations and grants.

**A credential goes only where it is scoped.** The engine checks the entry's `base_url` against the *credential scope*, the set of base URLs the credential declares, before attaching it. Rewriting an entry's `base_url` therefore cannot redirect a secret (ADR 0144). One key covering several hostnames of one provider declares each of them, and `lucidos credentials` below is how you read and set that. A `script_handshake` credential is the exception, because the script presents it rather than the proxy: what binds there is the `injects` column of the approvals record, which pins the exact secret set that entry may hand the script.

The engine also validates the upstream's certificate, and refuses a credential over plain `http://` unless the host is loopback. A self-signed dev backend or a keyed LAN device needs `"insecure_transport": true` on the entry, which is documented in `system-knowhow/building-an-auth-handshake.md`.

#### Builtin provider proxies (no `apis.json` entry)

Every model provider the engine holds auth for is also a proxy, with no `apis.json` entry and no credential to request. The names are `anthropic`, `claude-gateway`, `local`, `openai`, `openrouter`, `typesafe`, `vertex` and `xai`. The engine injects the key it already uses for chat. So never ask the user for a key these already cover. The ChatGPT plan (`chatgpt-plan`) has no proxy: apps and scripts never spend it.

**Each default base URL already includes `/v1`.** Send the path after it. A `local` base you configure may lack it, and the agent's context says which. `/v1/models` against `openai` reaches `https://api.openai.com/v1/v1/models` and answers 404.

```bash
# List the OpenAI models the engine's own key can reach
lucidos proxy openai /models

# A chat call through the builtin Anthropic proxy
lucidos proxy anthropic /messages -X POST \
  -H "Content-Type: application/json" -H "anthropic-version: 2023-06-01" \
  -d '{"model":"<model-id>","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}'
```

- **Not configured answers 404**, and the message names what to set (Settings → Providers, or an env var).
- **An `apis.json` entry with the same name wins**, so you can still point `openai` at a gateway.
- **`vertex` takes only the suffix**, `/publishers/<publisher>/models/<model>:<method>`. The engine owns the project and region prefix, and any other path answers 400.
- **`opencode-free` has no proxy.** The keyless free tier serves chat only, so nothing may build on it (ADR 0104).
- **`claude-gateway` shares the engine's sign-in.** Use only `POST /messages` with the Anthropic JSON body and `anthropic-version: 2023-06-01`.
  The engine supplies its configured origin plus `/v1`, applies managed model policy, and records gateway usage.
- The Lucidos Agent reaches the same proxies with `proxy_request`, and its context lists each one with its base and whether it is configured.
- **A model call through the proxy records what it cost.** A builtin proxy, or an `apis.json` entry pointing at a model provider's host, writes a `ContextCaptured` with `purpose: "proxy"` from the usage block the reply carries. It lands on the thread that ran the script, else on the home thread. A call made during a trigger fire also names the trigger in `emitting_trigger_id`, the only link for a script fire, which has no thread. So a script never reports its own model spend, and the Token Cost app counts it like any other call. The engine asks a model provider for an uncompressed reply, so it can read that usage block.
- **OpenAI speech (`/audio/speech`) records too.** OpenAI reports speech usage only on an SSE stream. So for `mp3`, `aac`, `pcm` and `wav` the engine asks for one and hands you plain audio. `tts-1` records its input's character count, which is what it is billed by. **`flac` and `opus` are refused with a 400**, because neither can report its cost: ask for another format (ADR 0423).

The full table of bases and injected headers is in `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`.

#### Usage (curl-style ergonomics)

```bash
# GET; body to stdout, exit 0 even on 4xx/5xx (curl convention)
lucidos proxy sonos /living-room/play

# POST with inline body
lucidos proxy comfort /api/v1/devices -X POST \
  -H "Content-Type: application/json" \
  -d '{"deviceGuid":"abc"}'

# POST with body from stdin
cat payload.json | lucidos proxy comfort /api/v1/devices -X POST --data-stdin

# Status line + headers + body (curl -i)
lucidos proxy sonos /zones -i

# Exit non-zero on HTTP errors and suppress body (curl --fail), for scripts
# that need to react to upstream failures
lucidos proxy sonos /zones --fail
```

Output is the response body on **stdout**. With `--include`, the status line and headers come first on stdout (curl convention: one stream). With `--fail`, the body is suppressed and a one-line `lucidos proxy: HTTP <code>` summary goes to stderr instead. Transport errors (DNS failure, connection refused, …) print to stderr (`lucidos: ...`) and exit non-zero. Exit codes mirror curl: `0` on success (including 4xx/5xx by default), `22` when `--fail` and the response is 4xx/5xx, `1` on transport failure.

#### Timeouts

The engine waits **30 seconds** on the upstream by default, then answers `504 upstream timeout`. The wait covers the whole reply: the proxy reads a streamed body in full before it answers, so a long streamed model call is cut at the same point. Two settings raise it, and both accept 1 to 600 seconds:

| Setting | Where | Applies to |
|---|---|---|
| `proxy_timeout_secs` | a workspace preference | every proxied call, including the builtin model routes (`vertex`, `openai`, …) that have no `apis.json` entry |
| `timeout_secs` | a field on one `apis.json` entry | that entry only, and it wins over `proxy_timeout_secs` |

```bash
# Let every proxied call wait up to five minutes
lucidos preferences set --key proxy_timeout_secs --value 300
```

```json
{ "slow-model": { "base_url": "https://llm.example", "timeout_secs": 300 } }
```

A value outside 1 to 600 is refused. For the preference, the write fails and names the key and the range. For an entry, the engine rejects that entry by name at boot, and every call to it answers 502 with the reason. Each upstream request gets the whole wait, so a redirect hop or the one retry after a 401 starts its own. One proxied call is capped at 600 seconds in total, and past that it answers 504. `lucidos proxy` itself waits a little longer, so it never gives up before the engine does.

A `script_handshake` proxy looks the same to the caller (`lucidos proxy comfort-cloud /devices/list`). The engine runs the login script and attaches the resulting headers. See `system-knowhow/building-an-auth-handshake.md` for authoring the script.

#### When to use which

| Want to … | Use |
|---|---|
| Call a backend the workspace will reuse | `lucidos proxy` (configure once in `apis.json`, then no auth in script) |
| Call a model provider (OpenAI, Anthropic, Claude gateway, OpenRouter, xAI, Vertex, local, TypeSafe) | `lucidos proxy <builtin> <path>`, no `apis.json` entry and no credential request |
| One-off `curl` to a service the workspace will never reuse | Plain `curl` (no proxy entry needed) |
| Emit a domain event, or query the event store (domain AND engine events) | `lucidos events …` |
| Write a file under `data/` | `lucidos data write …` |
| Push a notification to the user from a script | `lucidos notify --title … --message …` |
| Find a pending change's id from a script | `lucidos changes list` (read `.pending[].id`; don't scan `ChangeProposed` events) |
| Apply a coding-agent-proposed change from a script | `lucidos changes apply <id>` (never hand-roll the HTTP call: the actor stamps as "You") |

A script doing `curl -H "Authorization: Bearer $CRED_..."` against an API the workspace holds a credential for is drift. Add an `apis.json` entry and switch the script to `lucidos proxy`.

### `lucidos handshake list` / `lucidos handshake approve <path>`

The engine runs a `script_handshake` script only when it recorded who wrote it
(ADR 0144). `data/scripts/` is writable over the API, and an app UI reaches that
API with the user's authority, so the file existing is not enough.

The Lucidos Agent's file tools record as they write, so a script the agent
wrote or edited needs nothing more. This command is for a script that arrived
another way: the Files panel, an editor, or a plugin install.

```bash
lucidos handshake list
# approved     data/scripts/auth/firebase.py
# NOT APPROVED data/scripts/auth/comfort-cloud.py

lucidos handshake approve scripts/auth/comfort-cloud.py
# Approved data/scripts/auth/comfort-cloud.py
```

`approve` records the file as it stands now, so it has to run after the edit.
Any spelling of the path works: the `apis.json` value, the workspace-relative
form, or an absolute path inside the workspace.

**There is no button for this, deliberately.** The route refuses a
browser-shaped caller. An app UI shares the shell's origin, so a button would
let it approve a script it wrote itself. Revoking is a line deleted from
`<workspace>/.lucidos/approved-handshake-scripts`.

Each line there reads `<sha256>  <base_url>  <injects>  <path>`. `base_url` is
the one host the script's minted token may be sent to. `injects` is the set of
secrets `apis.json` may hand it, written `c:<credential>` and `o:<provider>`.
`-` means that column is not bound yet.

The first proxy call binds both from `apis.json`, and a later rewrite of either
is refused. Edit a column by hand to move a script, to share it with a second
provider, or to change which secret it receives.

### `lucidos credentials list [--name <N>] [--json]` / `lucidos credentials set-base-urls --name <N> [--auth-type <T>] --url <U> ...`

Read and set the *credential scope*: the base URLs one stored credential may be
presented to. The proxy sends a credential nowhere else, so this is the setting
behind a `will not be sent to` 502.

**A scope is a SET, because one key often covers several hostnames.** Binance
signs spot calls at `api.binance.com` and futures calls at `fapi.binance.com`
with the same HMAC pair, and Helius issues one key for two hosts. Each host is
named exactly. There is no wildcard, and nothing is inferred from a host's
spelling (ADR 0161).

**`list` also shows the variable names a command can use.** A `run_bash` or
`run_python` call gets a credential only when its text names one. So look the
names up here (`system-knowhow/best-practices/workspace-layout.md` § Credentials in subprocesses). An
`oauth_client` has none, because no subprocess ever gets it. Names only, never
values.

```bash
lucidos credentials list
# binance-key              api_key          https://api.binance.com
#     names: CRED_BINANCE_KEY
# github                   bearer           https://api.github.com
#     names: GITHUB_TOKEN CRED_GITHUB
# google                   oauth_client     https://oauth2.googleapis.com
# webhook-secret           secret           (no base URL, so it is sent nowhere)
#     names: CRED_WEBHOOK_SECRET

lucidos credentials set-base-urls --name binance-key \
  --url https://api.binance.com --url https://fapi.binance.com
# binance-key now covers https://api.binance.com, https://fapi.binance.com
```

`set-base-urls` **replaces** the whole set, so pass every host the credential
should reach. To widen one, run `list`, then `set-base-urls` with the old hosts
plus the new one.

`--json` prints the engine's own array, for a script that wants to read the set:

```bash
lucidos credentials list --name binance-key --json | jq -r '.[0].base_urls[]'
```

`--auth-type` picks a row when an OAuth client registration shares a name with
an API key. Omit it unless the command asks; a name is otherwise unique.

Each URL needs its scheme, so `https://api.example.com` rather than
`api.example.com`. A value with no host is refused here rather than silently at
the proxy gate.

**Adding, rotating and deleting a credential are not here.** The secret itself
belongs in Settings (or the agent's `request_credential`), which is where a
person enters it. This command reads and moves the scope only.

### `lucidos claude-gateway status [--json]` / `sign-in [--url <U>]` / `sign-out`

Sign Lucidos in to a *Claude gateway*: the Anthropic Messages API that an
organisation runs behind its own sign-in. Settings → Providers does the
same through the same routes, so a sign-in started here shows there too.

```bash
lucidos claude-gateway sign-in --url https://gateway.example
# Open https://gateway.example/device?user_code=ABCD-EFGH and enter the code ABCD-EFGH.
# Waiting for approval. Press Ctrl-C to stop waiting; the sign-in keeps running.
# Signed in to https://gateway.example.

lucidos claude-gateway status
# Gateway URL: https://gateway.example
# Signed in to https://gateway.example as user@example.com.
# Your organisation's gateway collects usage telemetry for your account: model, tokens, cost and timing per thread, never content.

lucidos claude-gateway sign-out
# Signed out of the Claude gateway.
```

- `--url` sets the `claude_gateway_url` preference first. It must be an
  `https` origin with no path. A URL that names another gateway signs out of
  the old one. The URL is the owner's to set: from an agent's own process,
  `--url` answers 403.
- The person approves the code on the gateway's page, on any device. The code
  expires after the time the gateway sets, often ten minutes.
- The engine keeps the tokens and refreshes them. No command prints one.
- `status --json` prints the engine's own answer from
  `GET /api/v1/providers/claude-gateway`: `url`, `account`, `sign_in`,
  `sign_in_again`, `telemetry` and `suggested_url`.
- `suggested_url` is the gateway Claude Code's managed settings name on this
  machine (`forceLoginGatewayUrl`). Nothing signs in to it until a person runs
  `sign-in --url` with it, or confirms it in Settings.
- `sign_in_again` is true when the gateway rejected the stored sign-in and none
  has been made since. Run `sign-in` again.
- `telemetry` is true when the gateway's managed settings turn on usage
  telemetry, and the engine's environment does not opt out
  (`CLAUDE_CODE_ENABLE_TELEMETRY=0` or `DISABLE_TELEMETRY=1`). The plain status
  then prints the telemetry line above. Each record holds the model, the token
  counts, the cost and the timing of one call. It also names the thread, and
  the account's email and id when the token carries them. Never a prompt or a
  reply. The
  policy may send records to its own collector instead of the gateway.
- The managed settings can also limit the models: a non-empty `availableModels`
  list replaces what the gateway's model listing offers.
- `sign-out` also asks the gateway to revoke both tokens, when its discovery
  document names a revocation endpoint on the same origin.
- Starting is `POST /api/v1/providers/claude-gateway/sign-in` with a JSON body,
  `{}` or `{"url": "<U>"}`. Stopping is
  `POST /api/v1/providers/claude-gateway/sign-out`. Both need a person's
  credential, and an app cannot reach either.

