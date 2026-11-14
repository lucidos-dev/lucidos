# 0636: The ChatGPT plan is its own provider kind, signed in as Lucidos itself

- **Status**: Accepted
- **Date**: 2026-11-09

Plan: `docs/plans/2026-11-09-chatgpt-plan-provider.md`.

## Context

A user with a ChatGPT plan wants the Lucidos Agent to run on it: chat,
triggers and sub-threads. Today OpenAI models reach the agent only through an
API key. The Codex auth detector (`llm/openai/codex_detect.rs`) reads only an
`apikey` login and ignores a `chatgpt` login on purpose.

OpenAI now documents *Sign in with ChatGPT* for open-source and locally hosted
apps (`developers.openai.com/siwc/token-sharing-open-source`). The user approves
the app once and can grant "Use your ChatGPT plan". The app's calls then count
against the plan's limits. The user can cap each app's weekly share under
ChatGPT Settings > Usage. OpenAI names Plus and Pro as eligible.

What the docs settle, read on the date above:

- **No partner approval for this track.** The app registers itself per user,
  with `client_id=dynamic_agent_client` on the first sign-in. The callback
  returns an issued `client_id` (`oaiapp_...`), bound to that user and ChatGPT
  workspace. There is no client secret and no partner key.
- **A paid or remotely hosted app** must use the interest form instead.
- **A public client with PKCE S256**, a fresh `state` and a `nonce`. The
  redirect URI is HTTP loopback on `127.0.0.1`, never `localhost`. Only the
  port may vary.
- **Scopes** `openid profile email offline_access resource.invoke
  chatgpt.tokens.use.direct`, with `resource=https://api.openai.com/v1`.
- **Every install sends a stable `ext_agent_host_id`**, an opaque value that is
  not a credential.
- **Inference is the public Responses API**, `POST
  https://api.openai.com/v1/responses` with the OAuth bearer. The docs forbid
  ChatGPT's `backend-api`. Several request fields must be omitted.
- **No device-code flow** is documented for third-party apps. A headless VM
  completes the sign-in on a machine with a browser.
- **Plan errors stop inference.** OpenAI never switches billing silently, and
  says an app may offer the user a choice.

## Decision

1. **A provider of its own, `ProviderKind::ChatGptPlan`** (wire
   `chatgpt-plan`). It shares the OpenAI Responses wire with `OpenAi`, as an
   endpoint variant, not a base-URL or key override.
2. **Lucidos registers per workspace through dynamic registration** and ships
   no client id. Each workspace keeps its own host id and issued client id.
3. **Tokens live in their own table, `chatgpt_plan_accounts`**, in encrypted
   secret columns. They never enter `oauth_accounts`. The provider refreshes on
   its own path, one refresh in flight per workspace.
4. **No reuse of the Codex login.** Lucidos never reads Codex's tokens.
   `codex_detect.rs` stays `apikey`-only.
5. **Sign-in uses the gateway-owned loopback callback** (ADR 0622). A browser
   with no Lucidos gateway on its own machine is refused upfront.
6. **No silent fallback.** A plan error fails the call with a typed reason. The
   router never moves it to another provider, and keep-alive beats never run on
   this route.
7. **Plan calls price at list**, per ADR 0525, and each capture names
   `chatgpt-plan` as its provider.

## Rationale

**Each credential stays on its own host by construction.** This is ADR 0508's
argument again. One OpenAI slot with a switchable token would let an API key
and a plan token cross. Two kinds make that a type error. An endpoint variant,
not an auth variant, keeps the plan token away from the web-search backend and
the app proxy, which take the platform credential.

**Dynamic registration is what an MIT project can ship.** There is no secret to
hide and no shared id to leak or revoke. Each workspace owns its registration,
so the user sees it and can cap it in Settings > Usage. Nothing in Lucidos is
closed, and nothing depends on a server we run.

**The Codex token is the wrong credential, and taking it breaks Codex.** It was
issued to Codex's client, with Codex's scopes, for ChatGPT's `backend-api`,
which the docs tell apps not to call. Refresh tokens rotate, so whichever copy
refreshes second gets `refresh_token_reused` and is signed out. It would also
skip the per-app consent and the per-app cap the user is promised. ADR 0508
already settles that Lucidos never reads another application's secrets.

**The relay already solves the headless case.** ADR 0622 forwards a loopback
callback from the browser's own gateway to a remote workspace. So a server with
no browser signs in from the user's laptop, and the server keeps its own host
id. That is better than the docs' file-copy procedure, whose transferred
sessions lose host-specific attribution.

**A silent fallback would move billing without consent.** The plan is a cap the
user set. Spending their API key or another provider when it runs out is a
decision OpenAI leaves to the user, and so do we. ADR 0508 made the same call
for the Claude gateway.

**Keep-alive beats would spend the user's plan.** A beat costs a request, and
the plan counts requests against a weekly limit. The docs also forbid
`prompt_cache_retention`, so the beat's premise is unverified here.

**List price keeps one rule.** ADR 0525 prices a subscription route at list,
because someone pays for it. The provider column on each capture still lets
Token Cost tell plan calls apart.

## Consequences

- A user signs in from Settings, the HTTP API or the CLI. The authorize page
  always opens outside the app (ADR 0128).
- The gateway forwards one more callback parameter, the issued `client_id`.
- A phone, or any browser that opens a remote gateway directly, cannot sign in.
  It is refused with one sentence naming the fix.
- Go, Business and Enterprise eligibility is OpenAI's call. Lucidos shows
  OpenAI's `subscription_sharing_user_not_eligible` answer and does not gate
  by plan.
- Each workspace is a separate app entry in the user's Settings > Usage.
- Only the first account may sign in again until the user forgets it, with
  **Use another account** or `sign-out --forget-account`. Forgetting clears
  the issued client id and the account, so the next sign-in registers afresh.
  A plain sign-out keeps them, and with them the user's per-app limit.
- A workspace restored onto another machine keeps its host id. The docs ask
  for a distinct id per host, so a restore should mint a new one (follow-up).
- Embeddings, speech, transcription and images stay on the API key: the plan
  serves only Responses.
- If OpenAI changes the preview's limits, the request profile and its pinned
  test change with it.

What we deliberately do not do:

- **No env var.** The sign-in needs a person.
- **No web search through the plan** in this change. The docs make it subject
  to account policy.
- **No app proxy access.** Apps cannot spend the user's plan.
- **No change to coding-agent threads.** Codex keeps its own login.
- **No usage meter.** OpenAI documents no endpoint for plan usage.

## Alternatives considered

- **Reuse the Codex `chatgpt` login.** Rejected above: wrong client, wrong
  scopes, a forbidden endpoint, and it signs Codex out on refresh.
- **Ship one Lucidos client id.** The docs issue ids per user through dynamic
  registration. A shared id would need the partner track, which is for paid or
  hosted apps.
- **A base-URL or key override on the OpenAI provider.** Rejected for the
  credential crossing, as in ADR 0508.
- **Store the tokens in `oauth_accounts`.** Every reader of that table hands a
  token on (ADR 0508 lists them).
- **Fall back to another route when the plan runs out.** Moves billing without
  consent. An explicit, user-set fallback is a follow-up.
- **A paste-the-redirect-URL fallback for phones.** ADR 0622 rejected it for
  reading as broken. It stays a follow-up for this provider only, because no
  web redirect URI exists here.
- **Price plan calls at zero.** ADR 0525 reserves "included" for a route that
  bills nobody.

## Notes

**A gateway older than this ADR breaks a relayed first sign-in.** The first
live sign-in failed this way. The browser's machine ran a packaged gateway
whose callback allowlist predates `client_id`. It forwarded `code` and `state`
to the remote workspace and dropped the issued id, which OpenAI did send.

We name the gateway in the refusal and fall back to nothing:

- **No fallback to the `id_token` audience.** The token exchange must send the
  issued id, and the docs forbid `dynamic_agent_client` there. Only that
  exchange returns an `id_token`, so its `aud` cannot supply the id it needs.
- **No pass-through of unknown fields in the gateway.** It would not reach the
  gateways already installed, which is where the skew lives. The allowlist
  also keeps a relayed callback to the fields an engine reads.
- **The refusal names the cause.** Every sign-in callback comes through a
  gateway, so a first sign-in with no client id says a gateway is likely
  outdated. Either can be: the browser's, or the remote host's, which trims
  the relayed query again. A retry works only after updating both.
