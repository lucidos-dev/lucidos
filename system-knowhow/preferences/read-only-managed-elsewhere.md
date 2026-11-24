---
name: Preferences, Read-only / managed elsewhere
description: Part of Preferences. Its index lists the sections it covers.
---

One part of `system-knowhow/preferences`, which lists every part.

## Read-only / managed elsewhere

`preferences(action="get")` also lists the settings among these so you can explain them,
and `preferences(action="set")` refuses every one with a hint naming the right surface:

- `command_guard`, `command_guard_judge`: the command guard (safety gate over
  the agent's own bash/python) and its LLM judge. Both default on: a workspace
  with no row runs guarded. Settings → Permissions only. You must not disable
  or weaken your own safety gate.
- `model_command_judge`, `reasoning_command_judge`: the model the judge runs
  on, for the guard and for coding-agent auto-answer. Settings → Models
  (Command judge) only. Unset, it runs the first measured model a provider
  serves (`gemini-3.1-flash-lite`, `gpt-6-luna`, `claude-haiku-5-5`), never
  the chat model. With none served and no System One judge picked, a turn
  under the guard and its judge refuses to start and names the fix.
- `coding_agent_auto_answer`: whether Lucidos answers coding-agent command
  cards with the guard's checks and judge. Off by default, and independent of
  `command_guard`. Only it lets the judge answer a coding agent. Settings → Permissions → Coding agent permissions only.
- `agents_unsandboxed`: Default `false`. The **Sandbox agents** toggle shows the inverse of this stored value.
  With the toggle on, every process an agent steers
  runs in an OS sandbox that hides Lucidos's private files. The sandbox also keeps each
  workspace's legacy grant-file paths read-only. That covers your own commands and
  scripts, coding agents, background tasks and MCP servers. Where the machine
  cannot apply the sandbox, such a process does not start, and the owner gets
  one notification naming the reason.

  With the toggle off (`agents_unsandboxed=true`), agents in this workspace run unsandboxed and the app shows a standing warning.
  Other workspaces keep their own setting.
  Settings → Permissions only. Never ask the user to turn Sandbox agents off to get past a
  refusal: name the reason and let them decide.
- `judgment_command_guard`, `judgment_query_classification`: which backend
  answers each judgment: `chat`, or a System One model (`jev`, `openrouter`,
  `clef`, `clef-flash`, `openai`, `custom`). Both ask typed questions and decide the outcome in
  code.

  `chat`, the default, asks the surface's own chat model. A System One
  model answers faster and sends what it judges to that vendor, and the chat
  model takes over if its call fails. The command judge's fallback is its
  judge model, and with none served the System One model answers alone.
  Neither key has its own control: **the
  System One models are rows** in that surface's model dropdown. Choosing one
  writes its id; choosing any other model writes `chat`.
  - The first dropdown is the Command judge row in Settings → Models
    (Background tasks). The second is the Query classification row there.
  - A System One row appears only once its provider is set up in Settings →
    Providers, and each sits under that provider's switch below.
  - `openrouter` and `openai` reuse the OpenRouter and OpenAI chat providers'
    keys, and sit under `provider_enabled_openrouter` and
    `provider_enabled_openai`. `openai` asks OpenAI's Decisions API on
    GPT-6 Luna.
- `judgment_read_decision`: the same choice for the *read decision* a turn
  ended without, in the Read decision row of Settings → Models (Background
  tasks). Unset runs on the chat model, `model_read_decision`. A System One
  model there reads every undecided reply, so only the user decides. A failed
  call or a missing answer still asks to be read.
- `judgment_memory_find`: the same choice for the recall tool's `find`. It has
  no control yet, so `find` runs on chat (`model_memory_find`) unless the user
  sets it. Picking a System One model there would send memory lines to that
  vendor, so only the user decides.
- `judgment_wake_decision`: the same choice for a child thread's *wake
  decision*. Its control is the Wake decision row in Settings → Models
  (Background tasks). Unlike most, unset is not chat: it runs on
  OpenAI Decisions while the OpenAI provider is configured and on and
  `model_wake_decision` is unset. `chat`, or a stored `model_wake_decision`,
  keeps it on the chat model. A System One model there reads the child's
  reply, so only the user decides. Any doubt or failed call still wakes the
  parent.
- `judgment_expected_wake_decision`: the same choice for a turn's *expected
  wake decision*, in the Expected wake row of Settings → Models (Background
  tasks). Unset runs on the chat model, `model_expected_wake_decision`, which
  picks one time bucket. A System One model answers with a probability per
  bucket, and the keep-alive plans from that distribution. It reads what the
  thread waits on, so only the user decides.
- `judgment_permission_answer`: the same choice for the *permission answer*,
  in the Permission answer row of Settings → Models (Background tasks). It
  reads the owner's spoken words against an open *permission request*
  and says which option they choose. Unset runs on the chat model,
  `model_permission_answer`. Only a clear pick grants. An unsure answer or a
  failed call grants nothing, and a tap still works. It decides who may grant
  a permission, so only the user sets it.
- `model_claude_code`, `reasoning_claude_code`, `model_codex`,
  `reasoning_codex`: the *default implementer*, the pair a coding-agent thread
  runs on when nobody picked one. Claude Code's ships as `claude-opus-5-5` at
  `medium`; Codex's ships as `gpt-6.2-sol` at `medium`. Lucidos pins the pair
  at spawn, so neither backend's config file decides. A pick that names a model
  and no effort runs at that model's default effort. Only the owner sets them, with the star in Settings →
  Intelligence.
- `intelligence_ladder_chat`: the owner's *intelligence ladder* for the
  Lucidos Agent, the same shape as the two below. It is the only switch for
  what the model picker lists: a model with a rung that is on. It also binds an agent's chat
  pick: `run_thread`'s `model` and a trigger's `model`. Its rungs may name
  `none`, which no coding agent takes.

  The owner's own picks are never held to it. The owner sets it in Settings →
  Intelligence, and an agent edits it only with the owner's
  `edit-intelligence-ladder` permission (`lucidos-cli.md`). The Lucidos Agent's *rung control* lists its rungs
  that are on. `manage_models`
  `enable` / `disable` and
  `lucidos models update --on-chat-ladder` turn one model on or off it, each
  behind that permission. A
  workspace upgraded from the old per-model on switch starts with the models it
  had on. `lucidos intelligence-ladder --surface chat` prints the effective one.
- `intelligence_ladder_claude_code`, `intelligence_ladder_codex`: the owner's
  *intelligence ladder* for each backend, as ordered `model=effort` rungs, least
  to most intelligent. Unset means the shipped ladder's rungs that are on. Set
  means exactly that list, minus pairs the backend no longer offers. Lucidos
  refuses an agent's pick off the ladder.

  The owner sets it in Settings →
  Intelligence, and an agent edits it only with the owner's
  `edit-intelligence-ladder` permission. The editor writes unset when the list equals the
  shipped one. **Copy from Lucidos Agent** there writes it once from the Lucidos
  Agent's ladder: the pairs this backend runs, keeping their on and off state, in the
  backend's own order. **Sort by intelligence** restates it in profile order,
  keeping what is on, and clears it when that equals the shipped list.
  `lucidos intelligence-ladder` prints the effective one.
  - **Building one from your own measurements.** Replay a few settled tasks on
    the candidate rungs, compare outcome and cost, and list the rungs that earn
    their place. A rung no other rung beats on both quality and cost belongs on
    it.
- `long_prompt_share`: the percentage of coding-agent calls whose prompt
  passes 100k tokens, from 0 to 100. A model that bills the whole call higher
  past that line, such as Haiku 5.5, shows its cost *band* at this mix. Unset
  means the share measured from the last 30 days of Claude Code calls. Only
  the owner sets it, with the slider in Settings → Intelligence.
- `backup_last_run`: internal backup state (the last run's outcome), not a
  setting. The backup *schedule*, *provider* and *retention* ARE settable (see
  the table above). `get_backup_status` reads the current schedule, next and
  last run, and recent history.
- `max_tool_calls`: how many tool calls you may make in one turn before the
  engine stops it with an `[ENGINE-LIMIT]` message. It counts calls, not
  replies: three calls in one reply spend three. Default `500`. The user may set
  any number of at least `1` in Settings → Models → Chat & triggers. A value
  below `1` is raised to `1`.

  Never via `preferences(action="set")`: it is the backstop over
  your own loop, so never raise it. The `[ENGINE-LIMIT]` prefix is your only signal the cap was hit.
- `keybindings`: Settings → Keyboard Shortcuts.
- `capture_context`: a debug-only toggle.
- `cache_cards`: whether a thread draws its prompt-cache cards (Cache kept warm
  and the cache-warm line). Default off. A sub-setting of the keep-alive: the
  cards draw only while `cache_keep_alive_mode` is `on` and this is on. Display
  only: pings and costs never change. The user changes it in Settings → Models
  → Cache keep-alive.
- `voice_input_device`: which microphone a call opens, per device. The value is
  a browser's own opaque handle, meaningless in another browser or on another
  machine. The user picks it by holding the call control in the composer. You
  cannot know what the ids stand for, so never write one.
- `network_bind`: this workspace's engine network bind (`loopback` / `all` / a
  specific tailnet IP). A security setting, changed in Settings → Access →
  Network access, never via `preferences(action="set")`. Takes effect on the next engine
  restart. The machine-global gateway bind and the engine-inherit toggle live in
  `~/.lucidos/private/network.toml` (`~/.lucidos/network.toml` in a home an older release created), not here.
- `engine_switch_dismissed_build`, `client_refresh_dismissed_build`: internal UI
  state. Each holds the build id the user deferred a "new version" toast for.
  The first holds the on-disk engine binary's id (the *Switch to new version*
  toast). The second holds the served client's id (the *refresh to sync*
  toast). Workspace-global (`device_id IS NULL`): a dismiss on one device defers
  the toast everywhere, until a newer build (a different id) re-surfaces it.
  The version-update toasts manage them, never `preferences(action="set")`.
- `chatgpt_plan_welcome_shown`: internal UI state. True once the one-time
  "You're using your ChatGPT plan" dialog has shown after the first Sign in
  with ChatGPT. Workspace-global, so it shows once per workspace. The Settings
  card writes it, never `preferences(action="set")`.
- `release_notice_cursor`: internal UI state. It holds a notice item id in
  the authored order. Visible items after it are still owed. Answers are
  workspace-global (`device_id IS NULL`), so answering on one device settles
  the same items everywhere. The engine advances it to the last item in
  an accepted answer and can place a fresh workspace at startup. Only the
  engine writes it.
- `backfill_trigger_id_from_events_done`,
  `backfill_trigger_id_v5_to_config_id_done`,
  `backfill_repo_names_from_changes_done`,
  `backfill_cc_repo_id_to_deterministic_done`: engine bookkeeping, never shown
  by `preferences(action="get")`. They are one-shot migration markers. `vapid_keys` is
  retired: the Web Push signing keypair is a sealed secret in its own table now.
- `provider_enabled_vertex`, `provider_enabled_anthropic`,
  `provider_enabled_openai`, `provider_enabled_openrouter`,
  `provider_enabled_xai`, `provider_enabled_local`,
  `provider_enabled_claude_gateway`, `provider_enabled_chatgpt_plan`: the
  per-provider switches on Settings → Providers. Absent means **on**; only an explicit `false`
  switches a provider off. Off drops it from the model picker and from web
  search, with no restart, and leaves the stored key untouched for later. Never
  via `preferences(action="set")`: the provider you switch off may be answering this turn.
  `opencode_free_enabled` above IS settable by you, because turning a keyless
  free tier on cannot leave the workspace unable to answer. An app may not set
  it: sending chat to an anonymous relay is the user's informed opt-in.
- `local_base_url`: the base URL of the `local` OpenAI-compatible provider,
  `http://localhost:11434/v1` by default. The user sets it in Settings →
  Providers. Neither you (`preferences(action="set")`) nor an app may set it: that host
  reads every prompt and writes the replies you run as tool calls. The host must
  be loopback, a private or tailnet address, or a `.local` or `.home.arpa` name.
  Any other host is refused on save, and one stored earlier is ignored, so the
  local provider drops out. The saved `local` key goes only where its credential
  scope covers this URL.
- `claude_gateway_url`: the origin of the Claude gateway Lucidos signs in to,
  unset by default. The user sets it in Settings → Providers, never
  you: that host receives every prompt sent to a Claude model routed there,
  with the user's sign-in. It must be an `https` origin with no path, and a
  write that is not one is refused. A URL that stops naming the signed-in
  gateway signs Lucidos out of it. The `claude-gateway` provider serves only
  while this URL is set and Lucidos is signed in to that gateway.
- `provider_enabled_typesafe`, `provider_enabled_cloudflare_workers_ai`,
  `provider_enabled_system_one_custom`: the same switch for the three System
  One providers, on the same page: TypeSafe's Jev, Cloudflare's Clef and
  Clef-flash, and a custom endpoint. They keep the absent-means-**on** rule and
  the stored-key promise. Each is the master switch above every `judgment_*` key
  that picked one of its models: off, those run on their chat model whatever the
  key says. A System One model holds no conversation, so it is in no chat model
  picker and no `configured_providers` list. It is offered only in the judgment
  rows (ADR 0224). Never via `preferences(action="set")`, for a different reason:
  switching one off returns the command guard's backend to chat, which is not
  your decision.
- `system_one_custom_url`, `system_one_custom_model`: where the custom System
  One endpoint lives and which model it is asked for, set on its row in Settings
  → Providers. That host receives every state a judgment sends it and
  can answer the command guard, so you never set either.
- `system_one_openrouter_model`: the System One model the `openrouter` System One
  row asks, set on the OpenRouter row in Settings → Providers. Unset
  asks Jev. It picks which vendor sees what a judgment judges, so you never set
  it.

> Keep this file in lockstep with `core/preference_catalog.rs`. A `cargo test`
> sync test fails if a catalog key is missing here. It also fails if a default stated here
> differs from the catalog's (see `.claude/rules/system-knowhow.md`).
