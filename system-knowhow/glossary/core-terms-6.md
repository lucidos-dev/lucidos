---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Provider
The backend that serves a *model*. Each *model registry* entry names its provider, whose credentials are configured once under Settings → Providers.

**Vertex AI** takes no stored credential: it resolves project and token from gcloud. **Anthropic** is direct via `api.anthropic.com`: a Claude subscription OAuth token or an API key. The `ANTHROPIC_API_KEY` launch env var is a fallback below the stored credential. **OpenAI** is direct via `api.openai.com`: an API key, with the `OPENAI_API_KEY` launch env var as a fallback. Below that, a key is auto-detected from the Codex CLI's `${CODEX_HOME:-~/.codex}/auth.json` `apikey` login, as Vertex reads the gcloud ADC file.

**OpenRouter** is `openrouter.ai/api/v1`: a Bearer API key, with `LUCIDOS_OPENROUTER_API_KEY` as fallback, serving e.g. GLM 5.2. **xAI** is direct at `api.x.ai/v1`: a Bearer API key, with `LUCIDOS_XAI_API_KEY` as fallback, serving e.g. Grok 4.6. **Local** is any OpenAI-compatible server (Ollama / LM Studio / vLLM / llama.cpp) at a configurable base URL. The default is Ollama's `http://localhost:11434/v1`, and the API key is optional.

**OpenCode Free** is `opencode.ai/zen/v1`, the one provider with no credential. The relay serves free models anonymously, so no key and no account exist. It is off by default and switched on with a toggle. Requests leave the machine to a third party, and several free models may train on them, so the toggle says so.

**Claude gateway** (`claude-gateway`) is an organisation's own *Claude gateway*, signed in to with a device code rather than a stored key. No launch env var sets it up, since the sign-in needs a person.

**ChatGPT plan** (`chatgpt-plan`) is the *ChatGPT plan provider*: GPT models on the user's own ChatGPT subscription, set up with Sign in with ChatGPT. No launch env var sets it up either.

OpenAI, OpenRouter, xAI, OpenCode Free and Local all speak the OpenAI Chat Completions wire format but are distinct backends.

A model naming several providers stays ONE picker entry, served by whichever has credentials. The first-party Claude models list Vertex and Anthropic, since their ids match on both. A backend that spells a model differently says so per provider: Grok is `grok-4.6` on xAI and `x-ai/grok-4.6` on OpenRouter.

### Claude gateway
An organisation's own front door to Anthropic's API: the Messages API behind the organisation's sign-in. Lucidos serves Claude models through it on the `claude-gateway` *provider*. Set up under **Settings → Providers** or with `lucidos claude-gateway sign-in`.

Signing in shows a code to confirm on the gateway's page. Lucidos signs in as itself, apart from Claude Code, and keeps and refreshes the tokens. No API response, event, app or provider proxy hands them on. Like every stored secret, they sit sealed in the workspace database, and `LUCIDOS_DATABASE_URL` cannot read them. The URL field starts on the gateway Claude Code's managed settings name, when that file exists, and nothing signs in until you confirm.

The gateway decides which models your account may use, and Lucidos never sends a model it did not list. Its *gateway managed settings* can narrow that list further. When the gateway ends the sign-in, Settings and the failed turn both say to sign in again. Changing the gateway URL signs out of the old one, and signing out asks the gateway to revoke the tokens.

Web search does not reach it. Apps and scripts use its *builtin provider proxy* with `POST /messages`.
See also: *provider*, *credential*, *gateway usage telemetry*.

### Gateway managed settings
The policy a *Claude gateway* serves for your account: the same document Claude Code's managed settings hold. Lucidos fetches it after signing in, after each token refresh and hourly, and keeps it until you sign out. Two parts of it apply in Lucidos:

- `availableModels`, when it lists any, decides which models the gateway serves here.
- Its telemetry switches turn on *gateway usage telemetry*.

Nothing else in it reaches Lucidos. It never hides your other providers: an organisation's policy shapes only its own gateway.
See also: *Claude gateway*.

### Gateway usage telemetry
A record of each model call made through a *Claude gateway*, sent when its *gateway managed settings* ask for it. It goes to the gateway, or to a collector the policy names. Each record holds the model, the token counts, the cost and the timing of one call. It also names the thread, and your account's email and id when the sign-in carries them. It never holds a prompt, a reply or a file. Settings and `lucidos claude-gateway status` say when it is on.

Calls made while it was off are never sent later, and one sign-in's records never reach another gateway. An engine started with `CLAUDE_CODE_ENABLE_TELEMETRY=0` or `DISABLE_TELEMETRY=1` sends nothing.
See also: *Claude gateway*.

### ChatGPT plan provider
The `chatgpt-plan` *provider*: GPT models served on the user's own ChatGPT plan instead of an OpenAI API key. Set it up with **Continue with ChatGPT** on the ChatGPT plan card under **Settings → Providers**, or with `lucidos chatgpt-plan sign-in`. OpenAI decides which plans may use it.

Signing in opens ChatGPT's own page. ChatGPT then sends the browser back to the Lucidos gateway on that computer, so start it from a browser on a computer running Lucidos. A phone is refused before any page opens. Each workspace signs in as its own app, named `Lucidos (<workspace name>)` in ChatGPT's usage settings. After the first sign-in, a one-time dialog explains the plan, with **Manage usage** opening ChatGPT's usage page.

While you are signed in, GPT models run on the plan ahead of the `openai` route. Background calls such as titles and summaries do too. A route you picked yourself still wins. Lucidos signs in as itself and never reuses the Codex CLI's ChatGPT login. The tokens stay sealed in the workspace database, and no response, app, script or proxy receives them.

Web search uses the plan when Vertex, Anthropic and OpenAI API key search are unavailable. The *builtin provider proxy* never uses the plan.

When the plan's usage limit is reached, the failed turn says so and Lucidos sends no plan requests for ten minutes. It never moves the call to your API key. When OpenAI ends the sign-in, Settings and the failed turn say to sign in again.

Only the first ChatGPT account may sign in to a workspace again. To switch accounts, sign out, then press **Use another account**, or run `lucidos chatgpt-plan sign-out --forget-account`. Lucidos then registers with OpenAI afresh, and a usage limit set for the old registration does not carry over.
See also: *provider*, *preferred provider*.

### System One model
A model that answers typed questions with a probability for every answer, rather than writing text. It cannot hold a conversation, so it never appears in the chat model picker. Six are offered: TypeSafe's **Jev**, Jev through **OpenRouter**, Cloudflare's **Clef** and **Clef-flash**, **OpenAI Decisions**, and a **custom** endpoint. OpenAI Decisions is GPT-6 Luna on OpenAI's Decisions API. A custom endpoint can be a model hosted on the user's own machine.

Each is set up on a row in Settings → Providers. OpenRouter and OpenAI use the key and the switch of their chat provider rows.

A System One model is picked in the model control of a decision Lucidos makes: the Command judge, Query classification, the Read decision and the Wake decision. Picking one sends what that decision judges to its vendor. If a System One call fails, the chat model answers instead. For the Wake decision that holds only for a chat model Lucidos measured for it, or one you picked: otherwise the parent wakes.

Most decisions run on a chat model until you pick, and storing a key moves none of them. One is the exception: the Wake decision runs on OpenAI Decisions while the OpenAI provider is set up, unless you picked a model for it.

### Builtin provider proxy
A *provider*'s API exposed through the engine's proxy **without** re-entering the credential in `data/config/apis.json`. Apps reach it with `lucidos.proxy(<name>).fetch(path, init)`, scripts with `lucidos proxy <name>`, and the Lucidos Agent with `proxy_request`. The builtin names are `anthropic`, `claude-gateway`, `local`, `openai`, `openrouter`, `typesafe`, `vertex` and `xai`. `opencode-free` has none (ADR 0104), and neither has `chatgpt-plan`: apps and scripts never spend the user's ChatGPT plan (ADR 0636). The `claude-gateway` proxy uses the engine's issuer-bound sign-in and managed model policy (ADR 0614).

When `<name>` matches one and no `apis.json` entry exists, the engine forwards to that provider's API root (every default root includes `/v1`). It injects the credential from Settings → Providers server-side, so the secret never reaches the caller. A same-named `apis.json` entry is consulted first and overrides the builtin. The Lucidos Agent's context lists every builtin and whether it is configured. `request_credential` refuses a key a configured builtin already injects (ADR 0350).

`vertex` takes the publisher/model suffix only: the engine owns the `…/projects/<project>/locations/<region>` prefix and mints the access token. So the app never needs the project id or a token. See `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`.

### Rejected proxy entry
An entry in `data/config/apis.json` the engine will not serve, because it does not parse or uses a retired shape. Only that entry is rejected: every other entry works, and the workspace still starts (ADR 0135). A notification names the entry and the reason at boot, and a call to that proxy answers `502`. Fix it by editing the entry and restarting the workspace.
See also: *derived proxy entry*, *builtin provider proxy*, `system-knowhow/building-an-auth-handshake.md`.

### OAuth provider registry
The list of OAuth providers Lucidos knows the endpoints for, stored as
`system-knowhow/oauth-providers.json`. Each row carries a provider's
authorization, token and userinfo URLs, its userinfo method, its authorization
parameters and its base URL. It also says where to register an app with it: the
console link, which client type to pick, which permissions to enable.

With it, **Settings → Accounts** offers a quick button per provider and
prefills a whole app registration. So an OAuth Client *credential* needs only a
Client ID. Saving copies the endpoints into the credential, so it fully describes
its own flow. A registry that later moves an endpoint cannot silently break a
working one; the *Lucidos Agent* repairs a stale credential on request. A
provider absent from the registry still connects: the form asks for its
endpoints, or which known provider a *derived provider* name runs on.

`system-knowhow/oauth-providers.md` is the prose beside it (redirect URI forms,
confidential versus public clients, scope notes) and does not restate the rows.
Adding a provider is a JSON edit, never an engine change.
See also: *connected account*, *credential*, *derived provider*,
*OAuth client type*.

### OAuth redirect URI
The URL the provider sends the user back to after they authorize. Lucidos must repeat it byte-for-byte when it redeems the authorization code. For a browser on a computer running Lucidos it is a loopback URL, and three host forms work: `http://127.0.0.1:14981/oauth/callback` (the default), `http://localhost:14981/oauth/callback`, and `http://[::1]:14981/oauth/callback`. The gateway holds that port and routes each callback to its workspace by `state`. Only the host form is configurable, via the optional `redirect_uri` key on the *credential*. Providers disagree: Spotify rejects the name form, and Microsoft's Entra portal rejects the IP form under its Web platform.

A browser on any other device gets a *web redirect URI* instead. The user must register the resolved URI with the provider exactly. Which form a provider wants lives in `system-knowhow/oauth-providers.md`, never in engine code.

### Web redirect URI
The *OAuth redirect URI* Lucidos sends when the browser runs on a device with no Lucidos, such as a phone: the browser's own https address plus `/~/oauth/callback`. It works only if the provider has it registered on a Web client. The *credential* must also list it under Web redirect URIs and carry a client secret. Otherwise Connect refuses at once and names the fix. See `system-knowhow/oauth-providers.md`.

### OAuth client type
Whether Lucidos authenticates the token exchange as a **confidential client** (sends the `client_secret`) or a **public client** (no secret, PKCE instead, per RFC 8252). It follows only from whether the *credential* carries a `client_secret`, so the engine never needs to know the provider. It must match the app's registration: a web/confidential one rejects a secret-less redemption, and a desktop/native/public one rejects a secret. Public fits best wherever offered when the browser runs on the engine host, since the loopback callback reaches it. Leaving Client Secret blank in the credential modal selects it.

### OS surface
The notification surface outside the Lucidos UI: an OS-level banner. Two transports, chosen by client:
- **Web push** (browser / PWA): delivered by the device's push service (APNs on iOS, FCM on Chrome/Edge, Mozilla autopush on Firefox) and drawn by the service worker. The browser requires each push to show a visible `showNotification()` (`userVisibleOnly: true`); silent pushes are penalised and can revoke the subscription.
- **Native desktop** (Tauri app): a macOS notification driven by the *NativePushRequested* SSE. The app's `show_native_notification` command shows and tap-routes it via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). WKWebView can't subscribe to Web Push, hence the SSE route. Requires a packaged `.app` build (inert in `tauri dev`).

Both ride the engine's single push-allowed decision (see PresenceCheck protocol). So a notification reaches a device through exactly one transport. A push goes out only when no page reported showing the *in-app surface*, so the two collide only when that report misses the deadline.
See also: `system-knowhow/notifications/1-conceptual-model.md` §1, `system-knowhow/notifications/3-the-presencecheck-protocol.md` §§3, 4.

### Orchestrator
The role a *parent thread* plays while it runs several *child threads* at once: it scopes their work, rules when they disagree, and is the only thread that can direct them. Not a separate kind of thread or relationship, just a name for what the parent does. Children may observe each other freely (events, artifacts, transcripts) and may never direct each other. So the immediate parent settles a disagreement from the shared event record; children never negotiate.

The rule recurses: a child that spawns children is their orchestrator. Beyond that, the engine models nothing; the rest is the orchestrator's judgement. Reasoning: `docs/adr/0083-sibling-threads-observe-never-direct.md`.
See also: `system-knowhow/orchestrating-sub-threads.md`.

### Device
One browser storage container that has met Lucidos. Listed in **Settings → Devices**, one row each, however it arrived. A device is what a push goes to, what a device-scoped *preference* applies to, and who an actor chip credits.

A device has one name everywhere: the name typed on its row, else its *pairing label*, else its browser and machine, as in "Chrome on Mac (109371a3)". Failing all three, it is `device-` and the start of its id. See `system-knowhow/remote-access/settings-access-2.md` § What a device is called.

**Per browser means per browser, and on iOS that includes the home-screen app.** iOS gives it its own storage container, so it is a separate device from Safari on the same phone.

One row carries two facts with different reach. Its *pairing* (may it reach the machine at all) is machine-wide: **Revoke** cuts it off every *workspace* at once. Everything else is per workspace: where push goes, which preferences apply, the typed name. **Remove** forgets only that, here, and leaves the device paired.

Either half can be missing, and neither is an error. A device paired from another workspace holds nothing here yet. Its row says **Not set up in this workspace**, with push off and disabled until it opens this one. The row never claims the device was never here, since a missing half does not prove that. A browser reaching an engine directly never went through the gateway, so it has no pairing to revoke.

**One device is one row.** The id comes from the *workspace gateway* when there is one, and both halves key on it. A browser reaching an engine port directly mints and keeps its own, which is safe: no pairing list exists there to disagree.
See also: *paired device*, *pairing code*, *preference*, *active device*, *one-off device*.

### One-off device
A *device* used on one day and never again. Lucidos removes it once a week old, unless someone named it, paired it, or turned push on for it. Most come from automated browser runs, since a fresh browser profile mints a fresh device id. One that does come back registers again under the same id, without its old device-scoped *preferences* and pinned apps.
See also: *device*, `system-knowhow/remote-access/settings-access.md` § The list of devices lives in Settings → Devices.

### Pairing label
The name a device got when it paired, such as "Safari on iPhone". The pairing screen suggests it from the browser, and the person at the device may type their own. It is fixed until you revoke the device and pair again. The *workspace gateway* passes it with each request, so every *workspace* knows it. A typed **Devices** row name wins over it (see *device*).
See also: *device*, *paired device*, `system-knowhow/remote-access/settings-access-2.md` § What a device is called.

### Paired device
A *device* you have allowed to reach Lucidos over the network. The *workspace gateway* answers no unpaired network caller, so joining the tailnet is not enough. Pairing is per device and per browser, and survives restarts until revoked. A paired device reaches every *workspace* that gateway serves. Workspaces are no boundary against each other, since a *coding agent* in one can already read another through the shell.

**Pairing is per gateway, which shows only when a machine runs two** (see *cross-gateway link*). Each keeps its own device list, *pairing code* and cookie. So one refuses the other's code, **Devices** lists the gateway serving that page, and revoking there revokes there (ADR 0132). One browser can hold a pairing to both at once.

On iOS the home-screen app is its own *device*, so it pairs separately from Safari. That is why the pairing screen shows a phone browser the install steps first, and why a phone can appear twice in **Devices**.

**A pairing ends when you revoke it, and at no other time.** Lucidos runs no idle or absolute timeout, so a device unopened for a year still works. A stolen credential in use never goes stale, so an expiry would cut off only the devices you forgot. Revoking answers the one you know you lost.

**One upgrade was the exception: the move to the *local certificate*.** It dropped each browser and phone pairing once, so each pairs again with a code. The desktop window re-pairs by itself, and paired gateways keep their pairing.

The browser cookie carries a window, refreshed each day the device is seen, but the *workspace gateway* never reads it. So **Devices** shows when each device was last seen, to the day. That is what separates a phone in daily use from a laptop you sold, so read it before revoking. A device paired before Lucidos recorded this shows no last-seen until its next request.
See also: *device*, *pairing code*, `system-knowhow/remote-access/diagnose-first-never-assume.md` § Devices pair before Lucidos answers them.

