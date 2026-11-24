---
name: Building an App, When an app is the right answer
description: Part of Building an App. Its index lists the sections it covers.
---

One part of `system-knowhow/building-an-app`, which lists every part.

## When an app is the right answer

An app is a persistent UI the user opens repeatedly. Push back on "make me an app" if something simpler fits:

| User says | Better answer |
|---|---|
| "Notify me when my package ships" | Trigger, not app |
| "Summarize today's emails" | Just answer, or save to `artifacts/` |
| "Track my morning habits" | App: repeated UI interaction |
| "Show me a chart of last week's runs" | If one-off, a widget in this thread. If they'll keep coming back, app. |
| "Compare these fares across three dates" | Widget: easier to scan and click than to read |
| "Show me flights to Stockholm", and they will pick one | Widget: a pick among data items, never question-card options |

When unsure, ask one question, then act: "Do you want to open this regularly, or is it a one-time thing?"

## Widgets: a small answer inline in the thread

A *widget* is an app that lives in the thread that made it. It is not in the apps list. `create_app` with `kind: "widget"` makes one and records this thread as its origin. Embed it in your reply, `![Name](app:<id>)`, and it shows there as an *inline widget*: the widget's bare *widget box*, centred in the turn.

A right-click or a long press on it opens its menu: its size first, then Open in Canvas, Add to dashboard and the rest. The box is exactly the box its dashboard tile draws at that size, so what the thread shows is what a dashboard shows. A widget the user wants in view goes on one of the two *dashboards*, each with its own tiles. The desktop dashboard stays in view in a rail beside everything, and the phone one is a sheet the user opens over any thread.

**When to choose one.**
- A comparison or a dataset that is easier to scan and click than to read: a fare comparison, a week's trend, a chart with a toggle.
- **A pick among data items**: flights, hotels, time slots, products. Build a widget the user picks in. Never list the items as question-card options; those are for deciding what you do next.
- Not for prose, and not for something the user will open every day (that is an app).

The system prompt says whether automatic widgets are on. When they are off, make a widget only when the user asks for one. For a pick, offer one as a question option instead. "Just text" means no widget.

**Build it exactly like an app.** The same scaffold, theme rules and SDK apply. Data still comes through `lucidos.*`, and a widget keeps it current the same way: one `render()`, called on each stream open and each matching file event (see *Live data* below). Five differences:

- **Small: a fixed box, no scroll area inside.** Put the answer where the smallest declared size shows it. One focused view, at most two actions, no tabs, no navigation, no settings screen. If it needs more, build an app.
- **Hold a phone height budget.** By default show about 5 list rows and one control row. Put the sort and the filter in that one row, and never stack two toolbars. Put the rest of the rows behind a "Show more" button.
- **Named by what it does**, in two or three words: "Flight picker", "Fare grid", "Run chart". The menus, the dashboard's Add and the apps panel show the name. So the trip, dates and other specifics go inside the widget and in its description.
- **It draws into a widget box.** Follow the box contract below, in a thread and on a dashboard alike. See `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Sizing a widget.
- **Its manifest says so.** `create_app` writes `"kind": "widget"` and `"origin_thread_id"`. Keep both, and `"reusable"`, `"params"`, `"sizes"` and `"transparent"`, when you rewrite `manifest.json`. A widget a plugin ships names `"origin_plugin_id"` instead and is always reusable: see `system-knowhow/plugins/shipping-widgets.md` § Shipping widgets.

**How a widget looks.** A widget is one strong shape on calm ground. The widget box around it already draws the frame, so the widget draws none.

- **Lead with one shape, not a table.** Pick the shape that carries the answer: a `.ring` for one score, a `.bar-run` for a short series, a `.choice-row` for a pick, or one large number. Markup and options are in `system-knowhow/js-sdk/setup-2.md` § Widget shapes. Keep a table for an app, or behind "Show more".
- **No title, or one short line.** The reply around it says what it is. Open with the answer itself, in one short sentence. Never write a heading that repeats the name.
- **No panel around the widget, and no border inside it.** The body is the ground. Separate the parts with space, never with a hairline or a box in a box: `body { padding: var(--space-lg); display: grid; gap: var(--space-xl); }`.
- **Theme tokens only. The accent marks the one thing that matters**, such as the suggested day or the picked choice. Paint the rest with `--text-*` and `--bg-*`.
- **Soft shapes on the theme's corners.** Use `--radius-round` for choices, pills and bars, and `--radius-surface` for a larger tile. A square theme then squares them too.
- **Tap targets are choices.** Give each data item a `.choice`, and set `aria-pressed` on the pick. Add at most one `.action-btn`, for the one action that follows the pick.
- **Each shape carries its own fill**, so it reads on any ground. Never rely on the page background to give a shape its edge.
- **Show only data you have.** A reading for a day still to come is a guess. Show a guess as a suggestion with its reason ("clear calendar, readiness up four days"), never as a value in a ring or a bar.
- **A calm tap.** The picked choice fills, and a ring or bar slides to its new value, on the `--duration-*` tokens. Nothing bounces. Under `data-motion="reduce"` it all stands still (§ Motion that explains).
- **Small type, one large value.** Body text is `--font-size-sm`, and a label over a part is `--font-size-xs` in `--text-muted`. Only the one key value goes up, to `--font-size-2xl` or `--font-size-3xl`.

**Params: one widget, many places.** A widget can take *widget params*, a JSON object given for each place it shows. A sound player takes the clip; a chart takes the series. Declare the names in the manifest, and read the object with `lucidos.params()` (`system-knowhow/js-sdk/lucidos-triggers-scheduled-tasks.md` § lucidos.params):

```json
{
  "name": "Sound player",
  "kind": "widget",
  "reusable": true,
  "params": {
    "clip": {"description": "A workspace path or an https URL", "required": true}
  }
}
```

```js
const { clip } = lucidos.params();   // {} when the widget was given none
```

The host carries them in the frame URL, so they are visible there. Never put a credential in a param. Params over 2 KB are refused: put large data in a file and pass its path.

**Sizes: how big its box may be.** A widget's card and its dashboard tile take one of four *widget sizes*: `small` (one square cell), `medium` (two cells wide, one high), `large` (two by two) or `xl` (four cells wide, two high). A widget that declares none takes small, medium and large and starts medium, so draw for each. `xl` is opt-in: an XL box is 42.75rem wide, so a widget takes it only by listing it. Declare `sizes` to drop a size the widget cannot draw well or to add `xl`, default first, with `create_app`'s `sizes` or in the manifest. Only a widget takes sizes, and each name may appear once.

```json
{ "name": "Clock", "kind": "widget", "sizes": ["small", "medium"], "transparent": true }
```

**Transparent: a widget with its own shape.** A tile draws a rounded surface behind the widget. Set `"transparent": true` for a widget that draws its own shape, such as a round clock: its tile and page are then see-through. Give the shape a fill of its own.

**The widget box.** Eight rules, one contract for a thread card and a dashboard tile:

1. **A widget draws into a widget box.** The box is fixed at its size. The host sets it, and content never sizes it.
2. **The same box everywhere a size applies:** the thread card and a dashboard tile. What the thread shows is what a dashboard shows.
3. **Read the size, never measure the page for it.** Style under `html[data-widget-size="small"]` and its siblings, or read `lucidos.widget.size()` and listen on `lucidos.widget.onSize()`. The document's viewport is the box, so `100%`, `100vh` and container units all mean the box.
4. **Fit by dropping content, never by shrinking it.** Type stays on the theme tokens, in rem, so the UI scale reaches it. A smaller size shows less: the one value at small, the pick and its alternatives at medium, the full answer at large, the full answer beside its context at xl.
5. **Never scroll inside the box.** Content past its bottom or right edge is clipped under a fade, and the widget menu's Open in Canvas shows the widget full size. Where a dashboard or a thread is narrower than an XL box, the host clamps the tile or card and scrolls the box sideways. The widget still sees `xl` and its true 42.75rem width, and never scrolls itself. A sideways drag on a clamped XL widget pans the box, so the widget gets no pane swipe there.
6. **Declare only the sizes you draw well.** A size the widget no longer declares, as after an update, falls back. The card or tile shows the nearest smaller declared size, else the nearest larger.
7. **Two places have no size.** In Canvas the widget is a full page. In a question option it is as tall as its content, under the option's label. There `lucidos.widget.size()` returns `null`.
8. **The widget fit check tells you what each box cuts off.** After you create a widget or write its `.html`, `.css`, `.js` or `manifest.json`, the tool result carries a `[WIDGET FIT]` note. An open Lucidos window renders every declared size on the desk tile and on a 360 px phone column. It uses the largest UI scale any device uses.
   - A pass is one line: `fits small and large on the desk tile and the phone column`. Anything else names the size, the surface, the frame px and the cut text. Fix each line, then run `widgets(action="fit")` again.
   - It counts text past an edge of the box, text an `overflow: hidden` ancestor cuts, text an ellipsis cuts, and media past an edge.
   - Never cut data text in a widget with an ellipsis. Show less instead, and let the rest wrap.
   - `not checked` is never a pass. It means no window could render the widget, as in a trigger run with no window open.
   - A widget with a required param is checked without its params, so its note says `not checked with params`. Fix what it names. To see its real content, embed it with params in a reply.
   - A local-token automation browser cannot answer as your paired device. A launcher with a verified origin for its source thread can register a fit-only renderer at `POST /api/v1/widgets/fit-session`, then explicitly select it on `POST /api/v1/widgets/fit`. Its one-check credential grants no owner authority; keep it in launcher memory and attach it only to the top-frame fit report. Never put it in widget frames, URLs or saved evidence. Ordinary paired windows need no session.

Sizes are names, never pixels. A cell is one size in rem, and a narrow phone column draws the cells smaller, as a narrow dashboard does.

**A tap leads into the work.** A tile is a way in: tapping a row opens its thread or app with `lucidos.ui.navigate` (`system-knowhow/js-sdk/lucidos-threads-thread-management.md` § lucidos.ui). Keep a tile live from data the workspace holds: a trigger fetches, writes to `data/` and emits an event, and the widget re-reads on that event. A tile that polls on a timer keeps a renderer busy for nothing, so stop timers when `lucidos.widget.onVisibility` says it is hidden.

**Embed a widget wherever a picture draws.** A *widget embed*, `![label](app:<id>?params={...})`, draws the widget live: in a question option's description or preview, in a card's `message`, and in a reply. The alt text is its label. Only the leading `!` draws it, exactly as for a picture. The params are a JSON object, written raw or percent-encoded.

- **In an option**, the engine lifts the embed into the option, and the card draws the widget under the option's label. One embed per option. A broken embed, an unknown or non-widget id, another thread's widget that is not reusable, or params the manifest does not take refuse the card. Fix the embed and ask again.
- **In a reply**, a bad embed cannot be refused, so it reads "This widget is not available" in place. Under that it gives the reason an option would get, such as `Widget 'habit-tracker' takes no param 'date'.` Pass only the params the widget's manifest declares. While the reply streams it shows its label; the widget mounts once the text is final.
- **A card starts at the widget's default size.** The user's size pick in its menu sticks to that instance in the thread, on every device and after a reload.

**Built-in widgets ship with Lucidos.** A *built-in widget* is a reusable widget under an id starting `lucidos-`, such as `lucidos-sound-player` or `lucidos-video-player` (each takes param `clip`: a workspace path or an https URL). It is read-only: no tool, route or CLI command can change, delete or stop reusing it, and `create_app` refuses a new `lucidos-` id. Read its files at `apps/<id>/` to build your own from it. Prefer a built-in over building the same thing again.

**Only one sound plays at a time.** A clip that starts in one app or widget pauses the others. So a voice sample per option never talks over another (`system-knowhow/js-sdk/setup.md` § One sound at a time).

**What the user can do with it**, from the inline widget's menu: right-click it, or long press on a phone. Each action is also yours:

| Menu action | Tool |
|---|---|
| Open in Canvas, full size | `navigate_ui` to the widget's id |
| Make reusable, offering it to every thread | `widgets(action="make_reusable")` |
| Show a widget in this thread | Embed it in your reply: `![Name](app:<id>)` |
| Pick a card's size in the menu's Size row. Only a size the user named in this turn | `widgets(action="size")` |
| Add to dashboard, from any thread: a tile at the card's size on the phone or desktop dashboard. A widget that belongs to its thread is made reusable first | `widgets(action="add_to_dashboard")`, with `dashboard` when the user names one, and `size` when the user named one |
| Remove from dashboard: that dashboard only | `widgets(action="remove_from_dashboard")` |
| Arrange a dashboard: order and size. Only when the user asks in this turn | `widgets(action="arrange")`, after `widgets(action="dashboard")` |
| Make app | Build a new app with `create_app`, reading the widget's files. The widget stays as it is. |

An embed, a card size and a dashboard add take params too: one set of params is one *widget instance*, with its own card and card size. `![Marin](app:lucidos-sound-player?params={"clip":"artifacts/voices/marin.mp3"})` shows that player with that clip.

The four dashboard actions take an optional `dashboard`, `phone` or `desktop`. Leave it out and they use the *last used device*'s dashboard. A turn with no device, such as a trigger run, must name one.

Deleting the thread deletes its own widgets. A reusable widget, and an app made from a widget, stay.

**Put an app on a dashboard as a shortcut.** Any app, not only a widget, can sit on a dashboard as an *app shortcut*. It shows the app's icon and name on the rail with no border, and a tap opens the app in the Canvas. The user adds one from the dashboard's Add, which lists apps under the widgets. An app row's Add to dashboard in the Apps panel does the same.

You add one with `widgets(action="add_to_dashboard")` and the app's id. A shortcut is a quarter tile: four fill one small cell. It never runs the app in the tile, takes no `params`, and cannot be resized. `widgets(action="dashboard")` lists each tile with its `kind`, `widget` or `app`. A deleted app's shortcut drops off.

## Questions to settle with the user before creating

Ask at most two questions before scaffolding. Pick the ones the request leaves open:

- **What's the smallest version that's useful?** (Surfaces scope.)
- **What data does it need to remember between visits?** (Surfaces storage shape.)

Don't design on paper. Scaffold the smallest thing that shows the idea (`create_app` with a working `index.html` + `manifest.json`), show it, then iterate.

