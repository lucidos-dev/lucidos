# 0415: Widgets take params and embed wherever a picture draws; built-in widgets ship with Lucidos

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

The Lucidos Agent asked "Which voice?" with four options: Marin, Cedar, Coral
and Ash. It put the four sample clips as links in the card's `message`, and the
options had bare labels. The user had to match a link above the card to a button
inside it. Each option should carry its own player instead.

Three things stood in the way:

- **A widget took no parameters.** `WidgetShown` carried only `app_id`, and the
  widget frame's src was `appUrl(appId)`. One sound player could not play four
  different clips.
- **An option cannot hold a frame.** An option is a `<button>`, and
  `renderMarkdownInline` strips every interactive element from its description
  and preview. A frame inside a button is invalid HTML and would swallow the
  taps that pick the option.
- **No widget shipped with Lucidos.** `AppManager` read only
  `<workspace>/data/apps`, so every workspace had to build its own player first.

The producers also differ. The chat tool's schema is ours. Claude Code's
`AskUserQuestion` is Claude's own tool, so we cannot add a field to it. Codex
asks through our MCP server, whose tool took plain string labels.

## Decision

**Widget params.** A widget receives a JSON object for each place it shows. The
host puts it in the frame URL as one query key, `?params=<url-encoded JSON>`,
and the widget reads it with `lucidos.params()`. Its app manifest declares the
names it takes: `"params": {"clip": {"description": "...", "required": true}}`.

**The widget embed.** Markdown `![label](app:<id>?params={...})` draws the widget
live wherever a picture draws: an option's description or preview, a card's
`message`, and a reply. The alt text is the label, and every producer writes the
same form.

For an option, the shared question parser lifts the embed into a typed
`QuestionOption.widget`, and the engine checks it before the card is asked. The
option draws the widget inside its box, under its label row, as a sibling of the
button. A settled card keeps every option's widget.

**Widget instances.** A widget with one set of params is a *widget instance*.
*Thread widgets* and the *widget shelf* key on `app_id` plus the canonical params,
so two instances of one widget can both be pinned. `WidgetShown`, `WidgetPinned`
and `WidgetUnpinned` gain optional `params` and `label`, and
`widgets(action="show")` takes both. An embed records no event. Its frame offers
a pin, and a pin alone creates the entry.

**Built-in widgets.** Lucidos ships reusable widgets in a `system-widgets/`
directory beside `system-knowhow/`, staged into every release the same way.
They are read-only, their ids start with `lucidos-`, and `create_app` refuses
that prefix. The sound player, `lucidos-sound-player`, is the first.

**The agent learns from its prompt.** The chat agent and the coding agents get the
same widgets section: the embed rule, a sentence on when to use one, and every
built-in and reusable widget with its params.

## Rationale

- **A query parameter names the resource; a hash names a place in it.** The
  hash already carries `navigate_ui`'s "open this item" for apps. "This player
  with this clip" is which resource, so it is a query.
- **One key, `params`, avoids every host-owned name.** The host and the engine
  already use `thread_id`, `device`, `download` and `_r` on an app URL.
- **JSON round-trips what the agent wrote.** The embed and the tool both give a
  JSON object, so numbers, booleans and lists arrive intact.
- **One embed form for every producer.** It is the only form Claude Code can
  write, since its schema is fixed. The agent already knows that only the leading
  `!` draws a picture. "A leading `!` on an `app:` link draws the widget" extends
  that rule, and has no exceptions to learn.
- **Everywhere a picture draws, because a rule with holes is harder to follow.**
  The user asked why an embed should work in an option but not in a reply.
- **Instances, because the user compares.** Night on one day and daytime on the
  next, or two bikes with live stock: each is a pin worth keeping.
- **Built-ins ship as a directory, like `system-knowhow/`.** That pattern already
  stages into the DMG and the headless tarball. A release updates every workspace
  at once, and no user edit can fight an update.
- **The whole widget list goes in the prompt.** A realistic footprint is about 125
  tokens with no reusable widgets and about 350 with three. A list tool works
  only when the agent remembers to look. The workspace audit, not the prompt
  design, owns a workspace that keeps too many.

## Consequences

- An option with a widget is taller. On a phone, four options with a one-row
  player fill about one screen.
- Each widget frame is its own renderer process (ADR 0227). Frames already mount
  only while on screen. One shared cap now limits mounted widget frames across a
  thread, and the frame furthest from view unmounts first. ADR 0422 later
  keeps a frame loaded a while off screen.
- A bad embed in a reply cannot be refused, because a reply is not a card. It
  shows "This widget is not available" in place. A bad embed in an option refuses
  the card, so the agent fixes it.
- A reply streams, and every token replaces the reply's HTML. So an embed mounts
  its frame only once the text is final, and shows its label until then.
- `fold_thread_widgets` no longer ignores a pin with no showing.
- Widget params are visible in the frame URL. They are the agent's words, not
  secrets, and must never carry a credential.
- Plugins still do not ship widgets (`system-knowhow/plugins.md`).
- Open: a per-instance chip picture. The thread "App icons and screenshots in
  apps list and plugins" decides chip pictures for apps, plugins and widgets.

## Alternatives considered

- **A native play button on the card.** One-off: it solves sound and nothing
  else. The user chose the general mechanism.
- **The hash fragment.** Synchronous and already wired for apps, but a hash names
  a place in a page. Mixing "which item" and "which clip" in one channel would
  collide in an app that also takes `navigate_ui`.
- **A host push on the `widget` bridge channel.** Typed and unlimited, but async:
  the widget's first paint has no params.
- **Engine-injected params in the served HTML.** A new server path, and the
  params land in request logs and caches.
- **Plain query keys with a refusal list.** Every new host-owned name would break
  a widget that already used it.
- **A typed `widget` field in the chat tool's schema.** Claude Code still needs the
  embed, so there would be two input forms to teach and test, and more schema
  bytes on every chat request.
- **Options only.** Smaller, but it leaves a rule with holes.
- **Identity by `app_id` alone.** One chip per widget, so the user cannot keep two
  instances.
- **Seeding built-ins into `data/apps`.** Editable per workspace, but an update
  then fights the user's edits, and each workspace's git gets system files.
- **Compiling built-ins into the binary.** No crate in the tree embeds a
  directory, and a widget fix would then need an engine rebuild. The bundle
  directory is the existing pattern.
- **No built-ins.** Each workspace builds its own player first, at varying
  quality.
- **The widget list on demand only.** Flat cost, but the agent forgets the list
  exists unless the prompt names the widgets.
- **The picked option alone keeps its widget once the card settles.** Fewer
  frames, but the user may want to compare again after answering.
