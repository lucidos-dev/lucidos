---
name: Lucidos JavaScript SDK, lucidos.ui: UI Control (continued)
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.ui: UI Control (continued)

### Starting a fresh chat with a prefilled prompt

`lucidos.ui.startThread()` opens a new chat thread. A `prompt` lands in the compose textarea **prefilled**: the user reviews, edits and clicks Send. It is never auto-submitted, so the user controls what is sent on their behalf.

```js
// "Set this up for me" button: pops a fresh chat with a ready-to-send prompt.
document.querySelector('#setup-trigger').addEventListener('click', () => {
  lucidos.ui.startThread({
    prompt: 'Create a daily 9am trigger that summarizes my unread email.',
  });
});
```

Call it with no arguments to open a blank fresh chat, like the "new thread" shortcut.

#### Preselecting the coding agent and folder

`coding_agent` and `folder` aim the new chat at a coding agent, so the user opens a ready-to-send coding-agent chat instead of picking dropdowns first.

```js
// "Work on PR" button: a coding-agent chat on the PR's repository.
lucidos.ui.startThread({
  prompt: 'Work on PR #123 on branch feature/x',
  coding_agent: 'claude-code',
  folder: 'example-repo',
});
```

| Field | Value | Effect |
|-------|-------|--------|
| `coding_agent` | `'claude-code'` or `'codex'` | Preselects that backend. |
| `folder` | A registered repository's name or id, or `data/apps/<id>` for an installed app | Preselects that coding target. A name matches exactly first, then in any case. |

- **Either field aims the chat at a coding agent.** `folder` alone keeps the backend the chat would have used. `coding_agent` alone keeps the target the chat would have used.
- **A value that names nothing never breaks the chat.** The chat still opens with the prompt. The dropdown stays at its default, and a warning toast names the value and who asked. An unknown agent, an unregistered or ambiguous repository name, and an app that is not installed all behave this way. Name a repository by id to avoid ambiguity.
- **Nothing is sent.** The user still clicks Send, and can change either dropdown first.
- **Only this chat changes.** The picks apply to the new draft, never to another existing draft. A folder pick also becomes the starting folder of the next new chat, as a manual pick does.
- **There is no `branch` or `model`.** Write the branch in the prompt text.
- **The `navigate_ui` tool takes the same two fields.** It also tells the agent in its result when `folder` matches no repository and no app, so the agent can correct itself.

### Confirmation dialogs

`lucidos.ui.confirm` shows a modal rendered by the Lucidos shell, outside your app iframe, so it inherits the user's theme and sits above all app content. Use it instead of `window.confirm()`.

```ts
interface ConfirmOptions {
  /** Optional heading. Renders above the message. */
  title?: string;
  /** Required. Plain text. A BLANK line (\n\n) starts a new paragraph; a
   *  single \n collapses to a space, the way HTML collapses source wrapping. */
  message: string;
  /** Default: "Confirm". */
  okLabel?: string;
  /** Default: "Cancel". */
  cancelLabel?: string;
  /** Style the OK button as destructive (red). Default: false. */
  danger?: boolean;
}
```

Resolves `true` on OK click or Enter, and `false` on Cancel, Esc or backdrop click. A second `confirm` while one is visible resolves the first `false` and replaces it.

Only the Lucidos shell that hosts the frame sees or answers a dialog. The SDK posts every `ui` request to the shell's origin and ignores a reply from any other page. A page that frames your app from another origin reads no dialog and cannot answer one.

**Example:**

```js
const ok = await lucidos.ui.confirm({
  title: 'Delete node?',
  message: 'Delete "Reduce CPAC by 50%" and its 3 descendants?',
  okLabel: 'Delete',
  danger: true,
});
if (!ok) return;
// proceed with deletion
```

### Toasts

`lucidos.ui.toast` shows a transient status banner rendered by the Lucidos shell,
above all app content and themed. It is **fire-and-forget**, with nothing to
await. Use it for success/error feedback instead of hand-rolling a banner.

```ts
type ToastType = 'success' | 'info' | 'warning' | 'error';

interface ToastOptions {
  /** A bold line over the message. Only the title is bold; without one the
   *  toast is its message alone. A non-string title throws a TypeError. */
  title?: string;
  /** Auto-dismiss after this many ms. Omit for the host default: errors,
   *  warnings and keyed spinning toasts stay until dismissed or replaced;
   *  success/info auto-close, with or without a key. */
  durationMs?: number;
  /** false = hide the close (X) on persistent toasts. Timed toasts always
   *  show the X so the user can dismiss them early. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast (message/type/etc.) instead of stacking a new
   *  one, e.g. an 'Opening…' toast becoming 'Opened'. */
  key?: string;
  /** true = show an indeterminate "work in progress" spinner in place of the
   *  severity icon. Pair it with a `key`, so a later keyed toast can replace
   *  the spinner with the outcome. Indeterminate only: no percentage. */
  spinning?: boolean;
}
```

`type` defaults to `'info'`, and an unknown value degrades to `'info'`. Only this
serializable subset is exposed. The host's toast action buttons take `onClick`
callbacks, which can't cross the app-iframe boundary.

**A tap on an app's toast does nothing**, so it stays up until its X or its
timer. A toast never takes keyboard focus. The user reaches it with the Focus
newest toast shortcut.

**A repeat counts instead of stacking.** Raise an unkeyed toast whose type,
title and message match one still up, and that card shows `×2`, `×3` and so on.
Its timer restarts. Use a `key` when a later toast should replace the words.

**The title is explicit, and the message is plain text.** Pass `opts.title` for
a bold line over the message. A newline in the message is just a line break: the
host reads no heading or list out of it, and shows a `"• "` line as written.

**A toast is a summary, and the host bounds it.** A message longer than 2000
characters is truncated with an ellipsis. An `'error'` toast shows its title and
message as ONE line each, truncated at 200 characters, so a newline becomes a
space. Keep an error to a sentence, and put the detail somewhere the user can
come back to.

**Example:**

```js
lucidos.ui.toast('Saved', 'success');
lucidos.ui.toast('Could not reach the server', 'error');
lucidos.ui.toast('Working on it…', 'info', { durationMs: 2000 });

// A bold title over a longer message:
lucidos.ui.toast('3 habits due today\n• Read\n• Walk', 'info', { title: 'Habit Tracker' });

// Collapse a two-step status into one toast that updates in place:
lucidos.ui.toast('Opening from Drive…', 'info', { key: 'drive-open' });
lucidos.ui.toast('Opened "Q3 deck"', 'success', { key: 'drive-open' });
```

#### Long-running work: a spinner you can take back down

`spinning: true` swaps the severity icon for a small indeterminate spinner, so a
keyed toast can narrate work with no honest percentage. Its counterpart,
`lucidos.ui.dismissToast(key)`, takes that toast back down, for work that
finishes with nothing left to say.

`dismissToast` is fire-and-forget like `toast`, and **a key matching nothing is a
no-op**, never an error. The user may have closed the toast, or its `durationMs`
expired, so "already gone" is normal. It reaches toasts by key only, so a
`toast()` raised without one can't be dismissed this way.

Start the spinner on the user's action, and let the event that reports the work
finished decide how it ends:

```js
document.querySelector('#reindex').addEventListener('click', async () => {
  lucidos.ui.toast('Reindexing your notes…', 'info', {
    key: 'reindex',
    spinning: true,
    dismissable: false,   // work is under way; there's nothing to cancel by closing
  });
  await lucidos.events.emit('ReindexRequested', {});
});

lucidos.sse.on('ReindexCompleted', (data) => {
  if (data.changed === 0) {
    lucidos.ui.dismissToast('reindex');   // nothing worth reporting, just clear it
  } else {
    lucidos.ui.toast(`Reindexed ${data.changed} notes`, 'success', { key: 'reindex' });
  }
});
lucidos.sse.on('ReindexFailed', (data) => {
  lucidos.ui.toast(`Reindex failed: ${data.error}`, 'error', { key: 'reindex' });
});
```

One `key` across every arm makes the spinner *become* the outcome in place,
instead of stacking a second toast. Give a `spinning` toast an end on every path
(a keyed replacement or a `dismissToast`), or the spinner stays forever.

### Prompts

`lucidos.ui.prompt` shows a single-field text-input modal rendered by the Lucidos
shell, themed and above all app content: the text-input sibling of `confirm`.
Use it instead of `window.prompt()`.

```ts
interface PromptOptions {
  /** Required. The question/instruction shown above the input. Plain text,
   *  with the same paragraph rule as `confirm`: a blank line (\n\n) starts a
   *  new paragraph, a single \n collapses to a space. */
  message: string;
  /** Optional heading rendered above the message. */
  title?: string;
  /** Prefilled input value. */
  defaultValue?: string;
  /** Placeholder shown when the input is empty. */
  placeholder?: string;
  /** OK button label. Default "OK". */
  okLabel?: string;
  /** Cancel button label. Default "Cancel". */
  cancelLabel?: string;
  /** Render a multi-line textarea instead of a single-line input. Default false. */
  multiline?: boolean;
}
```

Resolves the entered string on OK click or Enter, and `null` on Cancel, Esc or
backdrop click. A `multiline` prompt uses Enter for newlines, so submit with the
OK button. A second `prompt` while one is visible resolves the first `null` and
replaces it.

**Example:**

```js
const name = await lucidos.ui.prompt({
  title: 'Rename board',
  message: 'New name for this board:',
  defaultValue: 'Untitled',
});
if (name === null) return; // user cancelled
// proceed with `name`
```

### Tooltips

Any element with `data-tooltip` gets a themed Lucidos tooltip, with nothing to
call. `sdk.js` installs one delegated listener on the document, so an element
you add later is covered too. Never hand-roll a tooltip in an app.

```html
<button class="icon-btn" data-tooltip="Delete this row">🗑</button>
```

| Attribute | Effect |
|---|---|
| `data-tooltip` | The tooltip text. For the normal case this is the whole contract. Text after a U+001F character draws as a key chip on its own row, for a keyboard shortcut (`"Refresh\u001f⌘R"` from script). |
| `data-tooltip-title` | A bold heading above the text. |
| `data-tooltip-rows` | A JSON array of `{label, value, tone?}`, rendered as a two-column grid. Use it for a small property list. `tone` paints a leading dot and takes `running`, `changes`, `waiting` or `failed`. |
| `data-tooltip-below` | Always place the tooltip below the element, never above. |
| `data-tooltip-follow-cursor` | Anchor on the pointer instead of the element's border. Worth it only on a very tall element, where the border is far from the hand. |
| `data-tooltip-tap` | On a touch device, reveal on a single tap as well as on a long press. |

Placement: above the element by default, flipped below when there is no room
above. Horizontally centred on the element and clamped inside the viewport, with
the arrow kept on the anchor. Pointer scrolling repositions the tooltip.
It hides on mouseout, mousedown and when the frame loses focus.
A touch swipe dismisses it.
A popup toggle (`aria-haspopup` with `aria-expanded="true"`) shows none, so it
never covers the menu it opened.

**A pointer waits 300ms before the tooltip appears**, so crossing a toolbar does
not trail one behind the cursor.

**Touch: press and hold for 450ms.** The tooltip appears under the finger and
clears itself two seconds after you lift. Moving the finger cancels it, so a
scroll or a swipe never reveals one. The tap that ends the long press is
swallowed, so revealing a tooltip never also activates what sits under it.

**A redundant tooltip is dropped.** When the text repeats the element's own
visible text, and that text is not truncated, nothing shows. A truncated label
keeps its tooltip, which is what makes a clipped file name readable.

#### Turning it off

The layer stands down on its own whenever your page owns a `#tooltip` element,
so an app with a hand-rolled tooltip never shows two.

To turn it off deliberately, set the attribute in markup:

```html
<html data-lucidos-tooltips="off">
```

or call this once at startup:

```js
lucidos.ui.disableTooltips();
```

The attribute is read on `<html>` or `<body>`, and it applies before any script
runs. `disableTooltips()` sets the same attribute and drops the tooltip node.
Both last for the life of the page.

### lucidos.ui.ModelPicker: model, effort and provider

Uses the same stepped picker as Settings and chat, inside the app's own frame.
Available through `window.lucidos` and `@lucidos/sdk`. The shared
`.model-selection-field`, `.model-selection-value`, `.dropdown-menu` and
`.control-list` / `.control-option` classes supply its theme and geometry.

```js
const picker = lucidos.ui.ModelPicker.create({
  value: { model: 'your-registry-model', reasoning_effort: null, provider: null },
  filter: { callableOnly: true },
  onChange: value => console.log(value),
  onError: error => console.error(error),
});
document.querySelector('.model-field').append(picker.element);
await picker.ready;
```

`value` contains a registry `model`, nullable `reasoning_effort` and nullable
`provider`. A null provider resolves within the filtered routes; the returned
value names the actual provider. A null effort keeps the provider default.
Unsupported efforts clamp using the host picker rules. Explicit unavailable
models or providers reject rather than selecting another route.

Options also accept `disabled`, `label`, and `filter.providers` / `filter.models`
(string arrays). `callableOnly` restricts to supported builtin proxy protocols;
it does not hide routes that need credentials. Unknown setup state is shown as
not set up. Retired models are excluded and chat-ladder models appear first.

The instance exposes `element`, `ready`, `refresh()`, `getValue()`, `setValue(value)`,
`setDisabled(boolean)`, `open()`, `close()` and `destroy()`. Await `ready` before
opening. `refresh()` explicitly reloads the registry and rejects on failure;
load errors disable the field and call `onError`. Programmatic `setValue` never
calls `onChange`; while loading it is validated when that load completes.

Destroy removes the picker and its listeners. Escape steps back before closing;
only the final choice commits. The app owns persistence of the selected value.

### lucidos.ui.Select: themed dropdown

Replaces a native `<select>` (whose popup the OS draws and CSS can't reach) with a themed dropdown. Supports keyboard nav, type-to-select, and light and dark mode.

It renders the host's own `.dropdown-trigger` / `.dropdown-option` classes, plus `.surface-box` for the menu's box, so it matches the dropdown in Settings.

#### Types

```ts
interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface SelectCreateOptions {
  options: SelectOption[];
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  onChange?: (value: string, option: SelectOption | undefined) => void;
}

interface SelectInstance {
  element: HTMLElement;          // insert into the DOM
  getValue(): string | undefined;
  setValue(value: string | undefined): void;
  setOptions(options: SelectOption[]): void;
  setDisabled(disabled: boolean): void;
  open(): void;
  close(): void;
  destroy(): void;               // removes listeners + detaches the element
}
```

#### Keyboard

| Key | Action |
|---|---|
| `ArrowDown` / `ArrowUp` | Open the menu, then move focus through options |
| `Home` / `End` | Jump to first / last option (when open) |
| `Enter` / `Space` | Open the menu, or commit the focused option |
| `Escape` | Close the menu without changing the value |
| Letter keys | Jump to the next option whose label starts with the typed prefix (multi-character buffer, resets after 500 ms) |
| `Tab` | Close the menu and move focus to the next focusable element |

#### Programmatic usage

```js
const sel = lucidos.ui.Select.create({
  options: [
    { value: 'apple', label: 'Apple' },
    { value: 'banana', label: 'Banana' },
    { value: 'cherry', label: 'Cherry' },
  ],
  value: 'apple',
  placeholder: 'Pick a fruit…',
  onChange: (v) => console.log('picked', v),
});
document.querySelector('#my-container').appendChild(sel.element);

// Later, mutate it from outside:
sel.setValue('banana');
sel.setOptions([{ value: 'd', label: 'Durian' }]);
sel.setDisabled(true);

// Tear down when the host UI unmounts:
sel.destroy();
```

#### Declarative usage: enhance existing `<select>` elements

`enhanceSelects()` walks `root` (default `document`) and replaces every
`<select class="lucidos-select">` it finds. The native element stays in the DOM,
hidden. Its `value` mirrors the user's selection and `change` events still fire
on it, so existing form code keeps working. It skips already-enhanced selects,
so call it again after adding new ones.

```html
<select class="lucidos-select" data-placeholder="Choose…">
  <option value="todo">To do</option>
  <option value="doing">In progress</option>
  <option value="done">Done</option>
</select>
<script>
  lucidos.ui.enhanceSelects();
</script>
```

