---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Memory module
The workspace setting for how a turn gets its past, `memory_module` (ADR 0362). **Classic** is the conversation summariser, *memory recall* and *memory search*. It is almost free: on long threads it makes a small summary call now and then. **Tree** gives each turn two memory views: one over every thread and artifact write, and one of the thread itself. It also gives the agent the `recall` tool, which opens any line down to the exact message. Tree costs money, because a compactor model writes the summaries after each turn.

A new workspace asks in the *memory choice*, and runs Classic until the user answers. A workspace used before the memory choice existed stays on Classic, and is not asked. The user can change it any time under Settings → System → Memory.

Choosing Tree there first shows what its one-time background summarisation would cost, and starts it only on Start Tree. Turns stay on Classic until the workspace and the last week's threads are summarised, and a progress bar shows how far it has got. Older threads fill in after. On Tree, the page also lets the user browse those summaries, line by line, down to the exact message. `system-knowhow/preferences/how-a-write-propagates.md` § The memory module has the detail.

### Memory recall
Part of the Classic *memory module*. The engine reaching into long-term memory **for** the *Lucidos Agent*, automatically, before a turn starts. A classifier splits the user's message into sub-queries, the engine vector-searches memory with them, and the hits enter the turn's context. The transcript shows a step reading "Recalled 12 memories" (or "No memories recalled"). Tapping it lists each memory, linked to its source, above the sub-queries that found them.

Recorded as the `MemoryRecalled` *event*, which a *trigger* can subscribe to. Recall happens to a turn; a *memory search* is something the agent decides to do.

### Memory search
Part of the Classic *memory module*. The *Lucidos Agent* searching long-term memory itself, mid-turn, with its own query: the `memory` tool's `search` action, shown as "Searching memory for ...". It backs up a *memory recall* that missed, since recall runs once, from queries made before the agent read anything. Both rank the same corpus the same way, so a search never reorders facts already in context.

### Model registry
The database-backed list of chat models the user manages in **Settings → Models**. It is the catalogue the Lucidos Agent's *intelligence ladder* picks from, and tells the engine which *provider* serves each model. The model picker lists the models with a rung on that ladder. The engine seeds known models. The user can add models and delete their own (a builtin is never deleted).

A model the user adds starts off the ladder, and **Settings → Intelligence** turns it on. Adding one takes an id, a label and a provider, plus an unusual *context window* and the *vision flag*. Each row also carries its *default effort*, which the engine seeds and the user cannot edit.

A model listing several providers is served by whichever the workspace has credentials for: see *preferred provider*. The *rung control* lists *intelligence rungs* from the *intelligence ladder*, not these rows.

Manage models folds two groups below the main list, both closed at first:

- **Needs a provider** holds every current model with no provider set up and switched on. Each row links to its providers' blocks on **Settings → Providers**.
- **Retired** holds every model with a successor, set up or not. The router reaches its successor, so it never lists under Needs a provider.

### Vision flag
Whether a model reads images: the image icon on each row in **Settings → Models**, the `vision` field of `manage_models` and `lucidos models`. It is off unless declared. A chat turn withholds images from a row whose flag is off, and puts a short note in each image's place.

Image description offers, defaults to and runs only models that have it, and a set `model_image_description` without it is refused rather than replaced. The engine seeds it on the Claude, Gemini and GPT-5/6 builtins. Any row, builtin included, can be switched, since a seed can be wrong. A model with no registry row has no flag: a chat turn sends it images and lets the provider decide, while image description never picks it.

### Monogram tile
The stand-in for a missing *app icon* or plugin icon: the first letter of the name on a tile. A stable hash of the id picks its colour from the theme, so each one keeps its colour.

### Retired model
A *model registry* row whose provider retired the model. An engine update takes it off the Lucidos Agent's *intelligence ladder* and names its **successor**, the `successor` field of `GET /api/v1/models`. The same update moves every saved setting off it: model *preferences*, the Lucidos Agent's rungs, trigger pins and each thread's remembered model, at a reasoning tier the successor takes. A call that still names a retired model is sent as its successor, never left for the provider to reroute.

The successor runs only where this install reaches it: its provider is set up and has not answered that it lacks the model. Otherwise a saved setting keeps running the newest earlier version that is served, and Settings names it. A task's default never steps back to an earlier version. Past turns keep the old name, since that model answered them. Decisions: ADRs 0418 and 0519.

### Served model
The model a provider's reply says answered a call: Gemini's `modelVersion`, or the `model` field from OpenAI and Anthropic. Recorded as `served_model` on `ContextCaptured`, beside `model`, the one Lucidos asked for. A served model that is neither the requested one nor a dated snapshot of it means another model answered. Either the provider rerouted the call, or Lucidos sent a *retired model*'s successor. A Claude Code call records the reply's id in both fields, so it never reads as rerouted.

### Price table
The model list prices Lucidos uses wherever it estimates a cost: how long a *cache keep-alive* runs, the *handoff estimate*, the Tree backfill estimate, the intelligence ladder and the cost on usage telemetry. Each model has dated price cards, each naming its source, which Lucidos ships and updates with each release. A workspace overrides them card by card in `data/config/model-prices.json`. The table carries the day its prices were last checked, which Settings → Intelligence shows. **Keep prices current** there creates a trigger that runs the price check: it reads the providers' price pages and writes only the cards that differ to the override (`model-price-check` knowhow). `GET /api/v1/model-prices` and `lucidos.priceTable.get()` read the merged table.

### Motion
How much Lucidos moves on one device: **Settings → Appearance & Behavior → Motion**, the device-scoped `motion` *preference*. **System** follows the device's reduce-motion switch. **Reduce** calms the app regardless: nothing slides, pulses or spins, and changes appear at once. **Full** keeps every animation even when the device asks for less. The page gets `data-motion="reduce"` or `data-motion="full"` before first paint, and apps read the same attribute. The Animation speed slider under **Settings → System → Debugging** is a diagnostic, and has no effect while Motion reduces.

### Move to top level
Cutting a *child thread* loose from its *parent thread*, so the parent stops waiting and the child becomes top-level. The thread menu item, the `threads` tool's `detach_child` action and `lucidos threads detach` all do it, recorded as `ChildThreadDetached` on the former parent. Nothing stops: the child finishes its turn and keeps its work.

The parent gets no further result and cannot follow up. The moved child keeps holding the parent's child slot while it is a *live child*, so a move frees nothing until the child finishes. It cannot be undone. Not the same as a *detached* event wait, a subscription holding no turn.

The user can move any nested thread. An agent can move only its own direct children, and needs no other permission: any thread may create a *top-thread*.
See also: *child thread*, *parent thread*.

### Ready signal
An app's call to `lucidos.ui.ready()`, saying its first content is drawn. The host covers an opening app and shows a progress bar until it can reveal it. An app whose *app manifest* declares `"reveal": "on-ready"` is revealed by its ready signal, with a 15 s fuse. Any other app is revealed on page load, and the call does nothing. See `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Showing the app once its content is ready.

### Read marker
The "Sent" or "Read" label on a message sent to a thread. "Sent" means the engine handed it to the agent. "Read" means the agent took it in. A message stuck on "Sent" never reached the agent.

On a *coding-agent thread* the engine records each read as `CodingAgentInputRead` (ADR 0268). While the agent is busy, an unread message waits at the bottom as "Queued". Once read it moves into place, with what the agent does next below it.

On a *Lucidos Agent* thread a message is read once the agent starts its turn, or takes it into the running one. What a caller says on a call carries no marker.

Until read, a queued message offers **Edit** and a bin, on a Lucidos Agent or Claude Code thread. The bin takes it back. Edit takes it back into the compose box, text and images, so resending puts it at the back of the queue. Once read, both refuse and say so. Codex cannot take a message back, so its queued messages offer neither.

A Claude Code process can stop before it reads a queued message, as when Lucidos restarts under it. The next session then sends the message again, with a note that it is late. Until the agent reads it, it stays queued and removable (ADR 0506).

### Reasoning effort
How hard a *model* is told to think before it answers, chosen beside the model in the picker and in **Settings → Models**. Six levels: Off, Low, Med, High, X-High, Max.

Which levels a model offers depends on its *provider* as much as the model. Gemini stops at High, since every level above sends an identical request. OpenAI before GPT-5.6 stops at X-High. A non-OpenAI OpenAI-compatible server stops at High: X-High is OpenAI's own word, and a third party answers 400.

A level the model lacks snaps to the nearest one it offers, ties going up. So switching model never quietly spends less thought than you asked for.

Chat remembers a level per model, set in **Settings → Models** (the `chat_reasoning_efforts` *preference*). A model you never set one for runs at its *default effort*, and the picker's **Default** row returns a model to it. Switching a thread to another model never carries the old model's level along.

### Default effort
The *reasoning effort* a model runs at when nothing stored names one: the level its *provider* documents as the default, for example Medium for Opus 5.5 and High for Opus 5. Each row in the *model registry* carries it, and `manage_models` `list` shows it. A model whose provider documents none runs at "Provider default": Lucidos sends no level, and the provider picks. A background task keeps its own level on its recommended models, and runs any other model at that model's default effort. A coding-agent pick that names a model and no effort runs at that model's default effort too.

### Response style
How an answer comes back, chosen in **Settings → Models → Response style**. Two independent parts. The **style** is the shape: how much comes back and what for, from outcome-only to explaining the why. The *technical literacy* is how technical the words are.

The style applies to chat and every *trigger* from the next message, and never to a *coding-agent thread*. **Standard**, the default, adds nothing. Whatever the style, Lucidos keeps every warning, every caveat that changes the answer, and every step the user must take. That rule sits outside the editable text and cannot be written away.

### Theme
A named set of design-token values that retunes how Lucidos looks: colours, the header bar, focus, radii and shadows. It can suggest a UI font and a code font from the *font catalog*; the user's own font pick wins. A theme styles one *theme mode* or both, picked by the device-scoped `theme` *preference* (**Settings → Appearance & Behavior → Theme**). It holds only catalog tokens and *theme parts*, never CSS: it can recolour a control but never move or hide one. The *style overrides* still win over it, and it never repaints a *protected surface* past its clamped palette.

Built-in themes ship with the engine. A workspace theme is a file at `data/themes/<id>.json`, which a *plugin* can ship in its `themes/` folder. The engine validates it and derives what its three seed colours (background, text, accent) imply. Every surface, app frames included, paints the result. See `system-knowhow/themes.md`.

### Theme effects
Whether a *theme*'s part shadows, filters and scanlines show on one device: **Settings → Appearance & Behavior → Theme → Effects**, the device-scoped `theme-effects` *preference*. **Reduce** drops every part `text-shadow`, `box-shadow` and `filter`, and the screen's scanlines. It keeps part colours, letter-spacing, the caret shape and borders.

**System**, the default, drops them when the device asks for more contrast or less transparency. **Full** always shows them. The page gets `data-theme-effects` before first paint, and apps read it too; battery state plays no part. See `system-knowhow/themes/switch-themes.md` § Theme parts.

### Theme family
The group a *theme* belongs to in the picker: `blue`, `violet`, `warm` or `neutral`. The picker labels `blue` "Cool". A theme names it in its own file, so similar themes sit together. A theme with no family shows last, under "Other". See `system-knowhow/themes.md`.

### Theme part
A named region of the UI that a *theme* may style with capped paint-only properties. Shell parts include chat text, the actor icons, header titles, the composer, step cards, floating surfaces and the screen. Three more paint inside app frames.

A theme names the part and the property, such as `"parts": {"chat-text": {"text-shadow": "0 0 0.3em var(--accent)"}}`, and never writes a selector: Lucidos owns them all. Each property has a grammar and caps, so no part can cover, hide or fake anything. Parts may target both modes or one, and none reaches a *protected surface*. `GET /api/v1/themes/parts` lists every part, and each part property compiles to a *part token*. See `system-knowhow/themes/switch-themes.md` § Theme parts.

### Theme token catalog
Every token a *theme* can tune, with its group, a description, its default in each *theme mode*, and the seeds it derives from. Served at `GET /api/v1/themes/tokens`, so a theme-building app can list every tunable element. A test pins it to the stylesheet, so no declared token is missing from it. See `system-knowhow/themes/themes-in-a-plugin.md` § The theme token catalog.

### Theme mode
Light, dark, or system (follow the OS): the `theme-mode` *preference*, per device. The shell and every app frame paint the resolved value as `data-theme-mode` on `<html>`. Distinct from a *theme*, which picks the colours a mode paints.

### Technical literacy
How technical the user is, and so how technical the words are in every answer. Three levels, shown on every card as **Keep it plain** (non-technical), **Technical** and **I write software** (developer). It is the *response style*'s second part. The first chat asks for it during first-run setup. It can also be set in the *setup interview*, by telling the agent, or in Settings (row **How technical**). It reaches chat and *triggers* from the next message, and a *coding-agent thread* or a voice call from its next start.

Unset adds nothing. Lucidos stores only a level the user stated, never one guessed from their writing. The level changes the words, never the substance or the amount: warnings and steps stay.

At **Keep it plain**, the agent never asks a question the user cannot answer, such as what to do with a git branch. It decides, says what happened in their terms, and does the work itself rather than suggesting a *coding-agent thread*.

### Style library
The *response styles* a workspace can pick between. Lucidos ships four:

- **Standard**, the off switch, which cannot be edited or deleted;
- **Concise**, the answer first with no preamble;
- **Minimal**, the outcome and not the process;
- **Learning**, which explains the why as it goes.

The last three can be edited. An edited one keeps its shipped description, shows as edited in Settings, and offers *Reset* to restore the shipped wording. The user can add any number of their own, each a name plus an instruction in their own words. One style is selected at a time.

### Preferred provider
The *provider* a *model* was last picked on, remembered per model on its row. So Grok stays on xAI and Opus on Anthropic, neither overwriting the other.

The picker asks for a provider last, after the model and its *reasoning effort*, and only when two of the model's providers are set up. With one set up it never asks, but still names the provider. A pick sticks to the thread or draft it was made in, so a later pick elsewhere never moves a running conversation. A *trigger* can pin its own. Settings → Models → Manage models shows and edits which providers serve each model, in the order tried.

A model with no stored choice runs on its first backend with credentials. If the stored choice is later removed or switched off, the turn is refused rather than quietly moved. The picker badges it "not set up", so one click fixes it.

### Billing attribution
The *provider* a measured call's serving runtime identifies, independently of its model or producer.
Missing or ambiguous evidence gives `unknown`; a model name or current setting never fills that gap.
Older captures without attribution remain unattributed.

A **billing observation** records fresh, secret-free evidence from a coding agent, even when the attribution matches its previous report.
It is passive metadata, not another usage row.
The capture carries the call's billing attribution.

**Billing mode** distinguishes `token-metered`, `plan` and `unknown`.
A plan's token cost is a list-price equivalent, not an extra charge.
Knowing the provider alone does not prove its billing mode.
See `system-knowhow/coding-agent-events/billing-evidence.md` § Billing evidence.

