---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

<!-- domain-event: ComfortStateImported: Example domain event emitted by a comfort-data import. -->
<!-- domain-event: AnalysisCompleted: Example domain event emitted by an analysis script. -->
<!-- domain-event: DataImported: Example domain event emitted by a data-import script. -->

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos webhooks list | create | update --id <id> | delete --id <id>`

An endpoint a third party posts to, emitting one **pinned** domain event that a
trigger can react to. The event is fixed when you create the webhook, so an
endpoint you gave GitHub can only ever fire that one event.

```bash
lucidos webhooks list
lucidos webhooks create --name deploys --event-type DeployFinished
lucidos webhooks update --id <uuid> --enabled false
lucidos webhooks delete --id <uuid>
```

`create` prints the webhook plus a **token**, and that is the only time the
token exists in readable form. Only its digest is stored. A sender presents it
as `Authorization: Bearer <token>`.

**A signed webhook gets no token.** Configure `--hmac` and the hook
authenticates by signature alone (see § "Changing what a hook verifies with").

Deliveries go to `{host}:{hook_port}/<slug>/<webhook-id>`, on the gateway's
*hook socket* rather than its main port. `list` prints the path half of that as
`delivery_path`; the host and port are your own. The hook port is the gateway's
plus ten, so 5261 in dev and 5262 packaged.

GitHub, Stripe and Slack authenticate by signing the request body with a shared
secret. Save that secret as a credential, then name it in `--hmac`:

```bash
lucidos webhooks create --name github --event-type PullRequestOpened \
  --hmac '{"credential":"example-repo-webhook",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}'
```

The secret stays in the credential; the webhook holds only its name. Slack adds
`"timestamp_header":"X-Slack-Request-Timestamp"` with
`"template":"v0:{timestamp}:{body}"` and `"prefix":"v0="`. Stripe packs both
fields into one header, so it takes `"signature_key":"v1"` and
`"timestamp_key":"t"` with `"template":"{timestamp}.{body}"`. A template that
signs `{timestamp}` always has a replay window: a delivery signed more than
`tolerance_secs` from now is refused, and leaving it out gives a default.

A webhook needs at least one verifier and every one it has must pass. There is
no LLM tool and no SDK namespace for any of this, deliberately: a webhook opens
a publicly reachable door, so only you create one.

#### Where the shared secret comes from

Which side invents it depends on the sender, so `--signing-secret` saves the
credential in the same call rather than making you save it first:

```bash
# GitHub lets you choose the secret. Lucidos mints it and prints it ONCE.
lucidos webhooks create --name github --event-type PullRequestOpened \
  --hmac '{"credential":"github-deploys",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}' \
  --signing-secret '{"mode":"generate"}'
```

Paste the printed value into the Secret field on GitHub's own webhook form.

**Slack and Stripe issue their own**, shown on the app's Basic Information page
and on the endpoint's dashboard page. A secret you invent could never verify
their deliveries, so those take `{"mode":"provided","value":"..."}`.

**Slack's setup order matters.** Slack checks a Request URL the moment you save
it, so the hook must exist first:

1. Copy the Signing Secret from the Slack app's Basic Information page.
2. Create the hook with that secret:

   ```bash
   lucidos webhooks create --name slack --event-type SlackEventReceived \
     --hmac '{"credential":"slack-signing","signature_header":"X-Slack-Signature",
              "timestamp_header":"X-Slack-Request-Timestamp",
              "prefix":"v0=","template":"v0:{timestamp}:{body}"}' \
     --signing-secret '{"mode":"provided","value":"<signing secret>"}'
   ```

3. Paste the delivery URL into **Event Subscriptions → Request URL**.

Slack then posts a signed `url_verification` challenge. A hook verifying
`X-Slack-Signature` echoes it with a 200 and emits no event, so no trigger
fires on it. The echo runs only after the signature verifies, so a forged
challenge gets a 401.

A provided value is stored byte for byte. One that starts or ends with
whitespace is refused by name, rather than trimmed: trimming can break a value
that needs it, and keeping it breaks every delivery with nothing saying why.

On `create` a name that already exists is refused, so nothing you saved earlier
is overwritten by accident. On `update` it replaces the value, which is how you
rotate.

#### Changing what a hook verifies with

`--hmac` on `update` takes the same object, and the hook keeps its delivery URL.
Delete and recreate would change the URL and break the sender.

```bash
# Fix a wrong signature header, or point at a different credential.
lucidos webhooks update --id <uuid> \
  --hmac '{"credential":"github-deploys",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}'

# Rotate the secret and touch nothing else.
lucidos webhooks update --id <uuid> --signing-secret '{"mode":"generate"}'

# Stop signing. Prints a bearer token ONCE.
lucidos webhooks update --id <uuid> --hmac null
```

**A hook carries exactly one verifier kind**, so each of those moves the other
one too. Adding a signature drops any token, because a sender that signs
attaches no bearer token and a hook holding both would refuse every delivery.
Removing one mints a token, because a hook with no verifier at all cannot be
stored.

#### What a delivery becomes

Always three keys: `{summary, headers, payload}`. The sender's body is under
`payload`, so a trigger condition reads `payload.action`. `headers` holds the
request headers you allow-listed, read as `headers.X-GitHub-Event`. `summary` is
the sender's own if the body has one, and a generated line otherwise.

`--headers` is that allow-list. Without it the map is empty:

```bash
lucidos webhooks update --id <uuid> --headers '["X-GitHub-Event"]'
```

**`Authorization` and the hook's own signature header are refused**, since the
event log is append-only and a carried secret would stay on it for good.

#### Deduping a resend

Senders resend. GitHub retries a slow response and has a Redeliver button, and
Stripe retries for days. By default Lucidos emits on every arrival, so a resend
fires your triggers twice. That default is a real choice: every arrival stays on
the log, so with the delivery-id header allow-listed, a script trigger can count
how often a sender resends.

`--dedupe` opts out of that. Name the header carrying the sender's own delivery
id, and a resend inside the window emits nothing:

```bash
lucidos webhooks update --id <uuid> \
  --dedupe '{"header":"X-GitHub-Delivery","window_secs":3600}'
```

The resend answers 200 with `"duplicate": true` and the event id the first
delivery emitted, so the sender stops retrying. A resend that lands while the
first delivery is still being handled waits for it, then gets that same answer.
If the first one failed, the resend is delivered instead. Omit `header`
and the key is a digest of the body, which collapses two identical bodies inside
the window. `window_secs` defaults to an hour and is capped at seven days;
`0` switches deduping back off.

### `lucidos workspace-prompt-footprint show`

Prints the *workspace prompt footprint* as JSON: what this workspace's own
content adds to every chat turn, section by section.

```bash
lucidos workspace-prompt-footprint show | jq '.sections[] | {id, chars, over_ceiling}'
```

Each section has its `chars`, whether it is `over_ceiling`, and its `items`. An
item carries its own `chars` and `clipped_chars`, the part of its written
description the prompt leaves out. Apps, reusable widgets and knowhow docs also
carry `usage`: the days since last use and a `verdict` of `used`, `unused` or
`not-yet-judged`. The top level carries the total against the total ceiling,
the three preference values in force, and the *system prompt footprint* for
scale.

The *workspace audit* reads this in its prompt-footprint section. There is no
LLM tool for it, so measuring the prompt adds nothing to it (ADR 0413).

## Workspace resolution

The CLI finds **which workspace** to talk to in this order:

1. **`$LUCIDOS_WORKSPACE`**. The engine sets it on every spawned subprocess (Python, bash, coding-agent sessions), so it is authoritative there.
2. **Walk up from `$PWD`** to the first ancestor directory holding a `.lucidos/ports` file. The fallback for a terminal user running the CLI by hand without the env var.

**Reaching the engine API.** Once the workspace is found, the CLI picks the engine's base URL in this order:

1. **`$LUCIDOS_API_BASE_URL`**: set by the engine on every spawned subprocess when it is reachable somewhere other than the ports-file port. It is the exact base this engine answers on: loopback `http://` under the gateway, `https://` self-signed in the legacy single-engine model.
   - Under the workspace gateway (ADR 0014) the engine binds a **loopback HTTP** port. The user-facing port belongs to the gateway, which routes the workspace under `/<slug>/`.
   - So a bare `https://localhost:<gateway-port>/api/v1/...` request never reaches the engine: the gateway reads the first path segment as a workspace slug. Never build a URL from `$LUCIDOS_API_PORT`.
2. **`.lucidos/ports`** (`API_PORT` + optional `PROTO`, default `https`): the legacy single-engine model, where the engine listens directly on the user-facing port. Used when `$LUCIDOS_API_BASE_URL` is absent (legacy, Tauri, terminal).

## Common patterns

### Write an artifact and emit a completion event

The canonical end of an analysis or report session. An analysis app's prompt uses this instead of writing into the worktree:

```bash
ARTIFACT="artifacts/data-analysis/$(date +%Y-%m-%d)/report.html"

# 1. Write the artifact under data/.
lucidos data write "$ARTIFACT" --from "${TMPDIR:-/tmp}/report.html"

# 2. Tell the workspace it's ready.
lucidos events emit AnalysisCompleted \
  --summary "UA analysis for $(date +%Y-%m-%d) finished" \
  --payload "{\"artifact\": \"$ARTIFACT\"}"
```

Both calls use the same `data/`-rooted path, so a frontend SSE listener calling `lucidos.data.url(payload.artifact)` resolves the link.

### Call an external API and persist the response

The canonical "pull from a service, store under `artifacts/imported/`, signal completion" loop. The engine handles auth, so the script never sees the credential.

```bash
DATE=$(date +%Y-%m-%d)
ARTIFACT="artifacts/imported/comfort/$DATE/state.json"

# Configured in data/config/apis.json under "comfort" with bearer auth.
# Engine injects Authorization: Bearer <stored credential> automatically.
lucidos proxy comfort /api/v1/devices --fail \
  | lucidos data write "$ARTIFACT"

lucidos events emit ComfortStateImported \
  --summary "Imported Comfort Cloud device state for $DATE" \
  --payload "{\"date\": \"$DATE\", \"artifact\": \"$ARTIFACT\"}"
```

Compare the pre-proxy form, which leaks the credential into argv and shell history:

```bash
# Don't do this: $CRED_COMFORT shows up in `ps`, in shell history, and
# in any log that captures the script's invocation.
curl -sf -H "Authorization: Bearer $CRED_COMFORT" \
  https://accsmart.panasonic.com/api/v1/devices > "${TMPDIR:-/tmp}/x.json"
```

### Idempotent skip-if-already-done

```bash
LATEST=$(lucidos events query --type DataImported --limit 1 | jq -r '.[0].payload.date // empty')
if [ "$LATEST" = "$(date +%Y-%m-%d)" ]; then
  echo "Already imported today; skipping."
  exit 0
fi

# ... do work ...

lucidos events emit DataImported --summary "Imported $(date +%Y-%m-%d)" \
  --payload "{\"date\": \"$(date +%Y-%m-%d)\", \"rows\": $ROWS}"
```

## How the CLI ends up on PATH

For every script the engine spawns, it:

1. Symlinks the bundled `lucidos` binary into `<workspace>/.lucidos/bin/lucidos` (idempotent, so safe on every spawn).
2. Prepends `<workspace>/.lucidos/bin` to `PATH`.
3. Sets `LUCIDOS_WORKSPACE=<workspace>` so the CLI's fallback always resolves.

For Claude Code sessions it also writes a skill file at `<worktree>/.claude/skills/lucidos-cli/SKILL.md`, so Claude Code discovers the CLI through its normal skill mechanism. Codex sessions get the CLI guidance in their system prompt. A one-line reminder ("Use the `lucidos` CLI for any data-dir writes or event emits.") in a trigger or app prompt still helps.

## Implementation

- Source: `crates/lucidos-cli/`
- Shared engine wiring: `crates/lucidos-engine/src/runtime/lucidos_cli.rs`. `lucidos_cli_dir` discovers the binary, `ensure_workspace_bin_symlink` installs the workspace-relative symlink, `workspace_script_env_vars` builds the env var bundle.
- Used by `claude_code.rs` (Claude Code sessions) and `build_script_env_vars` in `crates/lucidos-engine/src/engine/engine_impl/scripts.rs` (Python and bash tool calls, scheduled scripts).
- For the in-browser equivalent used by app UIs, see [`js-sdk.md`](.././js-sdk.md).
