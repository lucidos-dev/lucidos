---
paths:
  - "crates/lucidos-app/src/**/*.ts"
  - "crates/lucidos-app/src/**/*.tsx"
---

# Frontend Conventions (TypeScript & State)

The CSS and component-class half lives in `.claude/rules/frontend-css.md`, which
loads for `.css` and `.tsx`. This file is the TypeScript and state contract and
deliberately no longer loads for a plain CSS edit.

## Frontend Sends Intent, Backend Owns Logic

The frontend expresses **what** the user wants, not **how** the backend should do it. Don't make the frontend extract data from its local state just to pass it back.

## Drafts Are Threads — Per-Draft Selections, Bound at Promotion Time

Compose drafts live in `threadMap` like any other thread (`meta.state === 'composing'`, `focusedThreadId` set). `sendCompose` (compose.ts) flips state to `'active'` BEFORE calling `sendMessage`, so `sendMessage` cannot tell first-send from follow-up by thread state alone: every plausible signal (`threadMap.get` truthy, `focusedThreadId === null`, `meta.state === 'composing'`) is wrong for at least one of optimistic insert, focused draft, or sendCompose's pre-flip.

Lifecycle states: `'composing' | 'active' | 'discarded' | 'archived'` (`ThreadComposeState`).

**Every compose-view dropdown selection is PER-DRAFT, never a bare global signal.** Draft content already is (`composeDrafts[id]`: text, images, `mode`), and so are the dropdowns — **target/scope, coding-agent backend, Lucidos model + reasoning, coding-agent model + reasoning** live in **`composeSelections[id]`** (`store/composeSelections.ts`) as *overrides*, resolved via the `resolve*(threadId)` helpers.

- **Model / agent / reasoning:** `this draft's override ?? the account default` (`currentModel` / `selectedCodingAgent` / `reasoningEffort`) — same shape as `effectiveSendMode`.
- **Scope is special** (no account preference): `resolveScope` falls back to `selectedScope` (localStorage last-used) **ONLY for the no-draft compose view**. An EXISTING draft resolves `override.scope ?? {kind:'lucidos'}`, never the shared `selectedScope`.
- **Hard requirement:** reading a mutable global (`selectedScope`, `codingAgentPending*`) *directly* in a compose surface — or letting an existing draft's scope fall back to `selectedScope` — is a bug, because a change on one draft would leak to every other (the original regression). Any new compose surface (control menu, destination row, drawer draft row) must key on the focused draft id through `resolve*`.

**Persisted in the DB (server-synced) — NO compose interaction writes an account preference.** `composeSelections` is not client-only: each draft's selection is stored in `thread_summaries.compose_selection` alongside its text/images/mode, written by the debounced compose PUT (`compose.ts` `pushNow` includes the selection; pick handlers go through `updateComposeSelection`), fanned out via the `ThreadComposeChanged` SSE (with a `selection` field), and rehydrated on reload (`stageDraftFromApi`) and on peer SSE (`applyRemoteCompose`) through `setComposeSelectionFromServer`. So a stored draft keeps its picks across a refresh and across devices (`docs/plans/2026-07-01-compose-selection-db-persistence.md`).

- **Never** write the account-wide preference (`chat_model` / `chat_reasoning_effort` / `coding_agent_default`) from a compose pick — those are account defaults AND the live fallback every override-less draft reads, so writing one leaks the pick to every existing draft.
- **Scope's `selectedScope` (localStorage) IS written on a scope pick** — it's the last-used seed for the NEXT new draft, not an account preference, and it's leak-safe precisely because no existing draft reads it.
- Coding-agent model/effort have no account default, so `resolveCcModel`/`resolveCcReasoningEffort` are purely the draft override or `null` (no pick → the send omits the field, backend applies its own default). They deliberately do NOT read the active-thread `codingAgentPending*` globals.

**The pending slot + new-draft scope seed.** A draft is created lazily on the first keystroke, so the fresh compose view often has NO focused draft (`focusedThreadId === null`) while the user picks a destination/agent/model. Those picks route through the **pending slot** `pendingComposeSelection`: a `null`/`undefined` threadId makes `getComposeSelectionOverride` / `patchComposeSelection` / `clearComposeSelection` (and therefore every `resolve*`) read and write the pending slot instead of the keyed map. When the draft is created, `ensureFocusedComposeThread` transfers the pending picks onto the draft's own entry AND eager-seeds `scope` from `selectedScope` (`seedComposeSelection({ scope: selectedScope.value, ...pending })`) — the eager seed is required because `resolveScope` for a real draft no longer falls back to `selectedScope`, so the new draft must carry its own scope; a pending scope pick wins over the seed. The seeded selection is persisted by the first keystroke's compose PUT.

Rule for compose surfaces: **reads** go through `resolve*`; a **per-draft/pending write** goes through `updateComposeSelection(threadId-or-null, …)` (patches + schedules the persisting PUT). A bare `setCurrentModel` / `setReasoningEffort` / `setCodingAgentDefault` from a compose surface is the bug (there is no `setCodingAgentDefault` anymore). Scope's write has its own entry point, `applyDestination`, because it ALSO updates the `selectedScope` last-used seed. The control menus derive "compose context" (a focused composing draft OR the fresh no-draft view) vs "active thread" — the CC menu as `!threadId` (active threads pass the active-session id; compose does not), the Lucidos menu via a `composeContext` prop — and route to the per-draft/pending path only in compose context.

At promotion, `sendCompose` resolves the draft's selections and binds them onto the send: scope→`meta.repoId`/`codingAgentKind`/`codingAgentFolder` + `codingAgent`, and model/effort/cc-model/cc-effort passed to `sendMessage` via `*Override` options; then it `clearComposeSelection(id)`. `sendMessage` uses those options when present and falls back to the globals when absent — so **raw-new sends (no thread) and active-thread follow-ups keep reading the globals directly** (there's no draft to carry the override). The active-thread control menus are unchanged: the Lucidos picker still writes the global `chat_model` preference, and the coding-agent menu still uses `codingAgentPending*` reconciled per-thread by `loadCommands`; only the *composing* branch of each menu is per-draft (`CodingAgentControlMenu` takes a `composeThreadId` prop for exactly this).

## Async Data Loading — `Loadable<T>`

Every async-fetched value **must** use `Loadable<T>` from `store/types.ts`:

```typescript
type Loadable<T> =
  | { status: 'not-loaded' }
  | { status: 'loading' }
  | { status: 'loaded'; data: T }
  | { status: 'failed'; error: string; httpCode?: number };
```

- Store signals: `signal<Loadable<T>>({ status: 'not-loaded' })`, never bare arrays
- Handle **all four states** — `loaded ? data : []` is a bug (masks loading as empty)
- Failed must look different from empty (error styling); never fake errors as `loaded` with empty data
- Tab data must load on page reload via `useStartup.ts`
- Use `ApiError` + `toFailed()`. Use `useDelayedLoading(loadable)` for loaders — true only after the load has been pending `SPINNER_DELAY_MS` (300ms), so fast loads never flash a loader. Loaders are **delay-only**: a *minimum-visible* floor was tried and rejected — holding a loader means withholding already-loaded content, which feels sluggish. The smooth skeleton **exit** is `<LoadingFade>` instead.

**Never render a bare loading indicator (`<div class="loading-spinner" />`, a skeleton) immediately.** Gate it on `useDelayedLoading(loadable)` (Loadable) or `useDelayedFlag(active, delayMs?)` (boolean) from `hooks/useDelayedLoading.ts` — both delay past `SPINNER_DELAY_MS`. `useDelayedFlag` also backs non-loader fuses (e.g. the 8s "tap to reload" timeout); there it's purely the delay.

- Prefer a **skeleton** (`<ThreadSkeleton/>`, or a self-skeletonizing row via `<ListSkeletonOf row={() => <MyRow/>} />`) for known-shape content (message threads, list rows); keep a plain spinner for inline status indicators and indeterminate "working" states.
- Smooth the skeleton→content handoff with **`<LoadingFade showSkeleton={delayedFlag} skeleton={<ListSkeletonOf row={() => <MyRow/>}/>}>{loaded ? content : null}</LoadingFade>`** (`components/shared/LoadingFade.tsx`) — crossfades the skeleton out as content fades in, without withholding content. ThreadView uses an equivalent fading overlay (`ThreadSkeletonOverlay`) so its scroll container is untouched.
- **Full-screen surfaces gate the skeleton too**, including the workspace picker (`<LoadingFade showSkeleton={useDelayedFlag(listLoading)} …>`). The earlier "no competing content → show the skeleton immediately (ungated)" carve-out was wrong in practice: the picker renders its brand header + footer immediately AND its inline boot splash fades over it for ~0.45s on every open, so the sub-`SPINNER_DELAY_MS` window is never a bare blank panel — an ungated skeleton just *blinked* under the clearing splash on a fast local backend. The delay gate suppresses it on a fast load and shows it only on a genuinely slow one. Still wrap in `<LoadingFade>` for the exit crossfade.
- **A small anchored popover/dropdown that loads its body should NOT skeleton-swap at all.** A skeleton whose height can't exactly match the loaded body makes the popover *resize* on load (no covering chrome to hide the swap). Instead render the body's STRUCTURE immediately at its natural height and defer only the loaded VALUES: dim + inert the value-bearing controls while loading (mark nothing active until the data lands, so no wrong default shows), and render static explanatory text right away so it pins the height. Only opacity/active-state changes then. Canonical example: the gateway picker's Network access popover (`components/picker/NetworkAccessPopover.tsx`, `ws-picker-net-controls[data-state]`), whose body is a pure function of `Loadable<T>` so the load/settle states are unit-testable. Three rules the popover's own bugs produced:
  - **A form's in-progress edit lives INSIDE the loaded state, never in sibling signals.** One `Loadable<{config, draft}>` makes a draft that outlived its config unrepresentable. The popover previously held the config in one signal and the three draft fields in three more, none reset on close, so cancelling an edit and reopening rendered the abandoned click as the active value until the refetch corrected it. Reopening must reset to `loading`, which carries no draft at all.
  - **Reopening refetches; the popover shows what is STORED, not what was last clicked.** A cached-config fast path is what makes a stale draft look settled.
  - **A row whose presence depends on the loaded value stays MOUNTED and animates its height** (`grid-template-rows: 0fr` to `1fr` with `overflow: hidden` on the inner element), rather than being conditionally rendered. Conditional mounting is what turns "the saved value happens to need an extra field" into a jump at settle time. Keep its leading gap as PADDING inside the clipped box, since a margin survives the clip and leaves a phantom gap when the row is shut.

**Skeletons are self-skeletonizing — every `Loadable`-backed list/tree surface ships one, mirroring the real layout by construction.** Don't hand-draw generic shimmer bars or a separate skeleton tree (it drifts the moment the row changes). Build the skeleton from the row's OWN component: give the row optional props + a `useSkeleton()` check, route its text through `<SkText>`, its buttons/chips/icons/dots through `<SkBlock>` (all from `components/shared/Skeleton.tsx`), and gate handlers/optional structure on `!sk` / `(sk || cond)`. Then render N copies through **`<ListSkeletonOf row={() => <MyRow/>} fill? containerClass=… />`** — pass the real list's container class so spacing mirrors; pass `fill` for full-pane lists (it measures the pane + the real row height). The leaves render real content when loaded and `.sk-bar` shimmer inside the provider, so the loaded path is unchanged and the skeleton can't fall out of sync. Trees vary rows by index via the `row(i)` thunk (see `folderTreeSkeletonRow`). A `<ul>/<li>` or brand-skinned surface that can't use `ListSkeletonOf`'s `<div>` wrapper (the picker, the control-panel switcher) still self-skeletonizes the real row markup inside a `<SkeletonProvider>` with the same `Sk*` leaves; re-skin `.sk-bar` with a scoped rule if the surface isn't on the default theme background (e.g. `.ws-picker-row .sk-bar`). Enforced by `components/shared/__tests__/skeleton-guard.test.ts` (source-scan: no reintroduced generic list skeleton; every `<LoadingFade>` paired with a self-skeletonizing skeleton).

**A single CONTROL fed by its own read gets the same treatment, and its skeleton wears the real control's box.** A settings page is often several controls on separate requests, each conditionally rendered on its own flag, so the page steps down as they arrive one at a time. Reserve the space instead: `<DropdownSkeleton w=…/>` (exported from `components/shared/Dropdown.tsx`, beside the thing it stands in for) renders the trigger's OWN `.dropdown-trigger` box and its real chevron, so padding, border, radius, the flex gap and the font metrics all come from one rule and only the *label* width is a guess. A hand-sized `<SkBlock>` passes review and then drifts the next time anyone re-pads the trigger, and a skeleton that drops the chevron is narrower than the control by exactly a chevron, which is the layout shift it was added to prevent. Two things bite when the skeleton lives inside `<LoadingFade>`: the fade wrapper becomes the flex/grid item in the parent row, so any `flex-shrink: 0` (or similar) that reached the control has to reach the wrapper instead (`<LoadingFade class=…>`, `.dropdown-slot`); and a slot whose control legitimately renders nothing once settled must unmount WITH it, or the row's `gap` opens around an empty child. Worked example: `DropdownSlot` in `components/settings/BackupSection.tsx`.

**Pattern in components:**
```tsx
const loadable = myData.value;
const showLoading = useDelayedLoading(loadable);
if (loadable.status === 'failed') return <div class="error">Failed to load</div>;
if (loadable.status !== 'loaded') {
  if (!showLoading) return null;  // No empty container flash
  return <div class="empty">Loading...</div>;
}
if (loadable.data.length === 0) return <div class="empty">No items</div>;
// Render items
```

## No Hidden Errors — Fail Fast, Tell the User

Errors must propagate to the frontend — never silently skip, swallow, or log-only. `console.error` alone is not acceptable.

- **Rust:** Use `?` to propagate. `catch { log; return empty }` and `catch { /* ignore */ }` are bugs.
- **TypeScript:** Use `showToast(msg, 'error')` or `Loadable` failed state. No fire-and-forget `promise.then(...)` without `.catch()`. Avoid dynamic `import()` for actions (circular deps cause silent failures).
- **The chain:** Backend error → HTTP → `ApiError` → `Loadable` failed → visible to user. No link may drop the error.
- **Error messages must name the entity and the origin — never a bare generic.** Say *what* failed (id/name/path — `App "demo-director" no longer exists`, not `App no longer exists`) and, when the action wasn't a direct click, *where it came from* (`… (requested by thread "X")` / `… (requested by an app)`). A generic message with the identity stripped is a swallowed error. Thread the originating context to the error site rather than dropping it. (Regression: a `NavigationRequested` from a sibling thread toasted "App no longer exists" with no id and no source, for an app that existed on disk.)
- **Never conclude "X doesn't exist" from a cached projection — reconfirm against the source of truth first.** Disk-/DB-backed lists (`appsList`, `artifacts`, …) are caches refreshed by SSE events; they go momentarily stale when a sibling thread mutates state. A definitive "gone" verdict (and its toast) must come *after* a re-fetch that re-reads the source (e.g. `openAppById` re-scans disk on a cache miss), not from a `list.some(...)` pre-check against the possibly-stale cache. A stale-cache pre-check that short-circuits the re-fetch is a swallowed error — it reports live entities as deleted.
- **A disk-/DB-backed list whose freshness depends on a refresh event ⇒ EVERY mutation path must emit that event.** The list is loaded by re-scanning the source (e.g. `loadApps()` → `/apps` scans `data/apps/`); the cache only updates live because something emits the `App*`/`Artifact*`/… SSE event that the frontend's `entityReferences` arm reloads on. A mutation path that doesn't emit it leaves every open page showing stale data until a full reload. When you touch a mutation site, check it emits the event the list listens on. (Regression: the chat `write_file` tool emitted `App*` only for `artifacts/` paths, so apps created via raw file writes never refreshed the list.)

### Carve-out: best-effort telemetry

A narrow `console.warn` (without a paired toast / `Loadable` failed) is acceptable for **best-effort telemetry that runs without user intent and recovers on its own**:

- presence pongs, device-visibility heartbeats, push-subscription keepalive
- startup probes that the user did not initiate
- background dev-breadcrumbs whose user-facing error surface lives elsewhere (e.g. a parallel `postAppCapture` that delivers the failure to the LLM)
- tab-close `keepalive` flushes where the document is tearing down and no toast can render

Required at every carve-out site:

1. **A justifying comment** above the `console.warn` saying WHY a toast is wrong and HOW the user still finds out if it matters (e.g. "next push attempt re-triggers the user-facing flow", "engine's deadline-then-default-to-push fallback covers a missed pong", "tab is unloading"). No comment → not a carve-out → fix it.
2. **No mutating user intent on the line that failed.** If the user clicked a button they are owed a toast — even for "cancel" or "dismiss". The carve-out is for code that runs whether the user did anything or not.
3. **Self-recovery.** The next probe / heartbeat / push attempt must either succeed silently or escalate via a different path that does surface to the user.

For paths that run on a schedule and could fail repeatedly (polling), prefer the tracked failure counter (`utils/failureCounter.ts`): silent below a threshold, single toast at threshold N, reset on success.

## No Silent Defaults

`COALESCE(x, 'chat')` masks bugs. Fall back to `'unknown'`, not plausible values.

## UI State Labels

Requesting (sent, waiting) → Working (steps arriving) → Waiting (CC idle) → Canceled (user stop) / Aborted (system)

## Notifications

`send_notification` tool: all contexts, args `title`+`message`, creates DB + SSE + web push. Scheduler auto-creates error notifications for failed tasks. LLM uses `send_notification` only for noteworthy results.

## Interruption Semantics

`completed: Some(true)` = normal. `Some(false)` = interrupted. `None` = in progress.
Frontend live: `wasInterrupted: !wasIdle`. Reload: uses `completed === false` from backend.

## Circuit Breakers

Generic breaker: same tool+target **failed** 3+ times in a row → warn the model; 5+ → force-break with error. It gates on consecutive *failure*, not consecutive call, so productive repetition (e.g. distinct `psql` queries) never trips. `read_file`/`list_files` keep their own content-deterministic breakers (identical re-reads block regardless of success). All bounded by `MAX_ITERATIONS`.

## Navigation That Lands Content Must Call `revealContentPane()`

Any callsite that puts something into the right-hand **content** pane — sets `panelOverlay`, switches `activeMenuItem`, opens a settings subview, opens an app / file / URL / trigger / change — MUST call `revealContentPane()` from `store/actions/pane.ts`. Mobile users get swiped to the content pane; desktop users get the **Content pane group activated** (`focusedPane = 'content'`), so keyboard Tab routes into the navigated view via `handlePaneTab`, plus a collapsed split re-expanded. Skipping it produces a silent no-op on whichever surface wasn't already visible — the user's tap "did nothing" — or, on desktop, lands the view but leaves Tab stuck on the previously-focused pane.

The desktop focus half is **signal-only**, matching the pointer-down `focusPane`: it sets `focusedPane` but does not move DOM focus, so the first Tab pulls focus in (`paneTabTarget`'s `activeIndex < 0` branch) without yanking focus mid-navigation.

Its mirror is **`revealThreadPane()`** (also `store/actions/pane.ts`): navigation landing on a **thread** (an existing thread, the empty compose view, or a brand-new thread spawned from another pane) re-expands a collapsed split (`splitRatio <= 0`, the Content pane group maximized) and re-activates the **Threads** pane group (`focusedPane = 'thread'`); mobile swipes to the thread pane. The *focus* half is normally taken only from the cross-group case (`focusedPane === 'content'`), so an existing `'drawer'`/`'thread'` focus is left alone and drawer ↑/↓ browsing isn't disturbed. A collapse it just undid is the exception and takes the focus unconditionally: the pane was invisible (and a collapsed thread pane hides the drawer with it), so a marker pointing into it is stale by construction. **Both halves are required, and the re-expand is the one that gets forgotten** (it shipped missing until 2026-08-05): with the thread pane collapsed, a thread link clicked from the content pane, e.g. a change toast, a Search Everywhere hit, a New-thread button, moved `focusedThreadId` behind a zero-width pane, so nothing on screen changed and the click read as a no-op. Mobile never saw it, because panes there are navigated rather than collapsed, which is exactly why a desktop-only regression like this hides. Three callers: `focusThread` (navigate to an existing thread), `unfocusThread` (the compose view: New-thread buttons, the new-chat shortcut, a new-chat NavigationRequested), and `sendMessage`'s raw-new-thread path (`isNewThread`, e.g. the new-app form submitting from the content pane). A raw-new send must reveal the thread pane the same way `focusThread` does. Without it, a thread created off the thread pane (the new-app flow) stays invisible.

**The opt-out is `{ revealPane: false }`, on both `focusThread` and `unfocusThread`, and it means BOOKKEEPING rather than navigation.** A focus change the user didn't ask for must not move the visible pane. Two callers:

- **Stale-pointer cleanup.** `ThreadView` clears a `focusedThreadId` whose thread left the map during *render*, and on mobile every pane is mounted at once (`MobileSwipeContainer`), so a reveal there would swipe a user on the content pane to the thread pane mid-render.
- **The post-archive hand-off** (`handleArchiveThread`). Archiving moves the focus to the next visible row (or drops it, on the last one) so the thread pane isn't left pointing at a thread that just left the list. On mobile the thread drawer is its own pane, so revealing swiped a user archiving row after row out of the thread drawer on every tap. Both branches pass `revealPane: false`, as does the re-focus after a rejected archive.

A user-intent focus/unfocus still reveals: the opt-out is never the default, and never a way to make a tap on a thread row quieter.

The rule has a sharp shape:

- **User-intent helpers own the call.** `switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`, `openApp`/`openAppById`, `openFilePreview`, `openUrl`, `openRepoFilePreview`, `openEncodedRepoFilePreview` (transitively, via `openFilePreview`), `navigateToTrigger`, and every `handleNavigationRequest` branch that lands content all call `revealContentPane()` at the end.
- **Pure plumbing does NOT.** `setActiveMenu` (`store/actions/menu.ts`) is signal-state plumbing used by multiple flows; it does NOT swipe panes. Same for any future helper that only mutates store signals without expressing user intent. The earlier `setActiveMenu` carried a conditional `navigateToPane('content')` gated on `item !== prev && mobileView === 'thread'` — the gate silently dropped the swipe when the user re-tapped the current item or wasn't on the chat pane. That conditional is gone; callers are explicit.
- **Don't reach for `navigateToPane('content'|'thread')` directly under an `isMobile()` gate.** That covers only the mobile half — it leaves desktop users with a collapsed split and no visible feedback. Content navigation uses `revealContentPane`; thread navigation uses `revealThreadPane`; both handle the desktop focused-pane-group half. A bare `navigateToPane('thread')` paired after `unfocusThread`/`focusThread` is the smell that the reveal belongs centralized in the helper (those manual pairings were removed when `revealThreadPane` was extracted).

When you add a new navigation entry point, add it to the test mirror in `crates/lucidos-app/src/store/actions/menu.test.ts` (or the equivalent suite) so the regression is pinned.

**The view swap itself is already smoothed, so do NOT add a per-callsite fade.** `revealContentPane()` is about *which pane the user is looking at*; the crossfade between the outgoing and incoming views is the **content-pane navigation cover** (`docs/glossary.md`), which `ContentPane` mounts centrally on every change of the **content view key**, whatever caused it. A new navigation entry point inherits it for free. What a new *view* owes is identity, not animation: if it is a `PanelOverlay` variant whose payload picks out one of several things (which file, which notification, which inline form), resolve it in `components/layout/contentViewKey.ts` so two of them are told apart. A variant that returns its bare type while displaying more than one thing is the bug this replaced, and it breaks the scroll memory in the same stroke.

## Pane Resize: Clamped Dividers & Header Sync

Desktop pane resizing follows the *clamped divider* contract (`docs/glossary.md` § Clamped divider; implementation in `components/layout/splitHelpers.ts`; ADR 0056): a divider drag is clamped to the pane minimums as it moves, so it stops at the wall while the pointer keeps going, and nothing corrects it on release. What the user drops is what persists. Three rules keep it honest:

- **A DRAG never collapses a pane; it only clamps.** Collapse belongs to the toggles (`toggleThreadPane` / `toggleContentPane` / `toggleThreads`), the shortcuts (`⌘⇧1`, `⌘⇧↵`) and the double-clicks. That pairing is what makes the clamp safe: the collapse-state attributes (`data-thread-collapsed` / `data-content-collapsed` / `data-thread-drawer-open`) flip at a ratio of exactly 0 or 1, a clamped drag cannot reach either, so a mid-drag flip is *unreachable* rather than merely postponed. Those flips are what swap header icon groups between hosts while the pointer wiggles across a pane edge, the "icons dance between the headers" bug. **Reintroducing a drag that can collapse re-opens it**, which is why the earlier deferred snap (a free drag corrected ~400ms after release) existed at all, and why it is not the thing to go back to: read ADR 0056 before proposing either.
- **Every pane minimum is derived, and they live in `store/paneMinimums.ts`.** The drawer's floor, the Conversation pane's and the Canvas pane's are all computed from the root font size, because everything they size is rem-authored and a px constant is only right at the one UI scale it was measured at. Reading one is a DOM read, so `splitHelpers.ts` stays pure: the caller measures (`splitBounds()`), the helpers compute. They are one module because they are weighed against each other, and from 150% ui-scale on a 1280px screen they stop summing, which is what `clampToRange`'s empty-range branch is for. **None of the three varies by client**: the drawer's floor pays the packaged build's traffic-lights lead in the browser too, so a workspace stops the drawer at the same width wherever it is opened (ADR 0058), and `data-titlebar-overlay` decides how the header row is laid out, never how narrow a pane may get.
- **New header regions join the resize kill list.** Any absolutely-positioned `.app-header` section with a `left`/`width` transition MUST be added to the `:root[data-pane-resizing]` `transition: none` block in `styles/panels/shell.css`, or it visibly lags behind the panes during a drag. Header sections and panes must share `var(--duration-slow) ease` for their geometry transitions so snaps arrive together.
- **Header regions hide by fading, never by `display: none` or unmount.** A collapse/toggle hides header elements via `opacity: 0; visibility: hidden; pointer-events: none` riding the same `var(--duration-slow)` transitions as the geometry — `display: none`, a removed conditional render, or a keyframe keyed on the state attribute pops the element out at the *start* of the pane animation while everything else is still sliding. Same rule for pane *content*: keep it mounted through the exit animation (`useLingeringFlag` in `hooks/useDelayedLoading.ts` — see the thread drawer's list).
- **A single CONTROL travels instead; it never fades, and it is never two copies.** The bullet above is about a whole *region* leaving with its pane, where the fade is uniform and the regions are adjacent by construction, so none of them can end up over another. A lone control that exists in two states is the opposite case, and both of the obvious shapes are bugs the user has reported: crossfading **two mounted copies** shows two half-transparent icons converging on one x (the drawer toggle, until 2026-08-09), and fading **one copy in place** leaves a dimmed ghost in a slot another control is sliding into (the same toggle on a Conversation-pane collapse, where the Canvas hamburger lands exactly there). So a control with two positions is ONE element whose `left` transitions between them, carrying no `opacity` anywhere: see `.thread-toggle-slot` in `styles/panels/shell.css`, pinned by `styles/__tests__/header-drawer-toggle-travel.test.ts` and `e2e/header-drawer-toggle-travel-desktop.spec.ts`. **Every one of its positions must sit on the same geometry track its neighbours ride** (for the toggle, `calc(var(--co) + var(--ddo))`), because `left` interpolates between two resolved lengths and each value therefore serves the animation in BOTH directions. Parking the toggle off-track at a negative `left` to send it out on a pane collapse read correctly going out and put it inside the drawer row for 92% of the way back. A control that has to disappear rather than move shrinks (`width: 0` under an `overflow: clip` scoped to that state, so no focus ring is ever clipped), since a full-width box cannot both clear the row and clear whatever slides into the slot it vacates. Giving it a geometry transition is what puts it under the kill-list bullet above, so the two land in the same change: the toggle's `left` depends on `--co`, which a drawer-divider drag rewrites on every pointermove.

Header overlap is prevented structurally: the content side (`.content-header-elements`) is a flex region pinned between the split divider and the header's right padding — icons and the centered title can shrink/clip but never overlap. Don't reintroduce absolutely-positioned children with hardcoded width reservations inside it.

**Keyboard resize and drag share one wall.** The Narrow/Widen pane shortcuts (`stepThreadPaneWidth` / `stepThreadDrawerWidth` in `store/actions/pane.ts`; pure math in `computeStepRatio` / `computeDrawerStepWidth`) move a divider by `KEYBOARD_RESIZE_STEP_PX` per press, clamped to the same pane minimums a drag is, and likewise never collapse. The keyboard path clamped first and the drag caught up (ADR 0056), so a new resize entry point takes the clamp from the same helpers rather than growing a third rule. The two differ in one way only: a keystroke against an already-collapsed pane is a no-op (the pane is a settled state), where a drag on its divider re-expands it to the minimum.

**Maximize a *pane group*; focus the Conversation drawer.** Two desktop shortcuts express the Conversation↔Canvas back-and-forth (`docs/glossary.md` §§ Pane group, Split): `⌘⇧↵` (`maximizePaneGroup` → `toggleMaximizeFocusedPaneGroup` in `store/actions/pane.ts`) toggles the focused *pane group* full-width via `setSplitRatio(1|0)` and restores the remembered ratio on a second press; `⌘⇧1` (`toggleThreadDrawer` → `focusOrToggleThreadDrawer`) is a three-stage open+focus / focus / close that moves DOM focus into the drawer so its existing ↑/↓/Enter list-nav lights up (re-expanding the Conversation side first if collapsed). Both are desktop-only (`isMobile()` guard) and rebindable. The drawer **icon** stays a pure show/hide (`toggleThreads`) — only the shortcut is focus-aware, mirroring the `focusPane` (pointer) vs `focusPaneAndControl` (keyboard) split.

**Shell shortcuts must reach the host even when focus is inside a content-pane iframe.** A keydown fired inside an `<iframe>` never bubbles to the host `document`, so the global handler (`useKeyboardShortcuts.ts`) never matches it, never `preventDefault()`s, and the chord falls through to the browser's default — e.g. focus a file/HTML preview, press `⌘⇧↵`, and Chrome opened its page context menu instead of maximizing.

- **App** iframes dodge the dead-shortcut half via the SDK forwarding their chords up over postMessage (`packages/lucidos-sdk/src/keyboardForward.ts` → `dispatchForwardedChord`), but that path can't suppress the browser default (no synchronous event to cancel).
- **Preview** iframes (file/HTML/diff/PDF — `FilePreviewInline`/`RepoFilePreview`) run no SDK, so they're bridged directly. They're same-origin (`about:srcdoc` inherits the host origin; engine-served files are same-origin), so `bridgePreviewIframeShortcuts` (`components/files/previewIframeShortcuts.ts`, wired via the iframe `onLoad`) attaches a capture-phase `keydown` listener on the iframe's own `contentDocument` and runs `dispatchPreviewIframeShortcut` — matching the same registry, `preventDefault()`ing the host-shortcut chord (killing Chrome's default), and reconciling `focusedPane = 'content'` first (the keydown lives in the content pane, but the host never saw the pointer cross the iframe boundary). Non-shortcut keys (plain Enter on a link, typing) are untouched; a cross-origin preview throws on `contentDocument` access and no-ops.

## Modals & Popovers: Click-Outside Dismiss

**Non-negotiable, app-wide principle: any click outside an overlay closes it and does NOT activate anything else.** Every modal, popover, dropdown, anchored panel, command palette, bottom sheet — anything overlaid on other UI — dismisses on an outside click AND swallows that click, so the button / chat row / link under the cursor never also fires. Two contracts in one, both always required: the user meant "get rid of this thing", not "get rid of it *and* press whatever was behind it". Escape dismisses too. And while it's open **the UI behind is inert** — no hover highlight, no activation (the hover analog of the swallow). Even a backdrop-less popover (a dropdown, the control panel) makes the UI behind inert.

Why both halves are required: a CSS-only "click closes me" via backdrop element solves dismiss but leaves the underlying click free to fire a different button; a bare `pointerdown` listener that calls `onClose` without swallowing the paired event does the same. Get this wrong and a user dismissing a settings popover can accidentally send a chat message, open a different app, start a new thread, or trigger a destructive action.

**Build every overlay through the one central `<Overlay>` component** (`components/shared/Overlay.tsx`). It bakes the whole contract in, so an individual overlay can't drop or mis-wire it — the exact failure that shipped the SearchEverywhere "second tap reopens" bug (it passed `anchor={null}`). Do NOT hand-roll a `document.addEventListener('mousedown'|'pointerdown'|'click', …)` close handler, do NOT use a CSS/backdrop-only `onClick` to dismiss, and do NOT call `useDismissOnOutside` directly from a feature component — `<Overlay>` is the surface; `useDismissOnOutside` + `overlayStack` are its internal mechanisms and should have no other callers.

Centered / sheet modal (full-screen backdrop container):

```tsx
const open = useSignal(false);
return (
  <>
    <button onClick={() => (open.value = true)}>Open</button>
    <Overlay open={open.value} onClose={() => (open.value = false)}
             overlayClass="my-modal-overlay" panelClass="my-modal">
      …content…
    </Overlay>
  </>
);
```

Toggle-opened overlay (popover / palette) — pass the toggle as `anchor`. Anchored popovers also compute their position with `useAnchoredPosition` and pass it via `panelStyle` + `backdrop={false}`:

```tsx
const [anchor, setAnchor] = useState<HTMLElement | null>(null);
const open = useSignal(false);
return (
  <>
    <button ref={setAnchor} onClick={() => (open.value = !open.value)}>Open</button>
    <Overlay open={open.value} onClose={() => (open.value = false)} anchor={anchor} panelClass="my-popover">
      …content…
    </Overlay>
  </>
);
```

**Always pass the toggle that opened the overlay as `anchor`. Never `null` for a toggle-opened overlay.** The anchor is exempt from the outside-pointerdown dismiss, so re-activating the toggle closes via the toggle's OWN handler rather than being raced by the dismiss. With `anchor={null}`, on touch the outside `pointerdown` closes the overlay and then the toggle's `touchend` re-flips it open — it never closes (the SearchEverywhere bug). When toggle and overlay live in different components, stash the element in a signal at click time and pass `signal.value` — `controlPanelAnchor`, `drawerAnchor`, `searchEverywhereAnchor` are the precedents.

What `<Overlay>` does for you (mechanism: `makeDismissHandlers` / `store/overlayStack.ts`):

1. `pointerdown` outside the panel + anchor → `onClose()` and arm "swallow the next paired event" (the `touchend`/`click` the browser is about to dispatch). **The swallow is armed on a one-shot `document` listener that OUTLIVES the overlay's unmount** (`installPairedSwallow`): closing re-renders and tears the overlay's own listeners down in the microtask checkpoint *between* the pointerdown task and the next event task — i.e. BEFORE the gesture's paired `touchend`/`click` fires — so a swallow living only on the overlay's listeners would already be gone (the original compose-on-first-tap bug survived a first attempt for exactly this reason). The one-shot self-disarms on the first swallowed event (or `touchcancel`/`pointercancel`, or a short fuse) so a later unrelated tap is never eaten.
2. The paired `touchend` outside the panel + anchor is captured at `document` in the **capture phase** (so it precedes the target button's own bubble-phase `onTouchEnd`) and `stopPropagation`+`preventDefault`'d. This covers touch buttons that run their action on `onTouchEnd` and `preventDefault()` the synthetic click (the iOS keyboard-nudge pattern in `composeHandlers`): without it the outside pointerdown dismisses the overlay but the button still fires its action on the same tap (the compose-on-first-tap bug), and — because the button cancels the synthetic click — no `click` ever arrives. The `touchend`'s `preventDefault` cancels the synthetic click. Anchor / inside-panel touches are never swallowed. (The overlay's own open-gated `touchend` handler also swallows, for the rarer same-task case where it hasn't been torn down yet — complementary, not redundant.)
3. The paired `click` (the mouse case — on touch the `touchend` already consumed the one-shot) is captured at `document` and `stopPropagation`+`preventDefault`'d — nothing downstream fires.
4. A `click` arriving **without** a preceding outside pointerdown (synthetic `HTMLElement.click()` from a keyboard shortcut, an e2e driver, or any programmatic source) falls into a click-capture fallback: same `isOutsidePointerTarget` check, same `onClose()` + swallow. Without this branch the contract silently breaks for any caller driving dismiss via synthetic clicks (`e2e/overlay-dismiss-swallow.spec.ts`, over the drawer row's overflow menu, is the canary; it used to be the thread filter, which is a pane panel now and no longer an overlay at all).
5. The anchor is exempted (re-activating it must toggle via the caller's `onClick` / `onTouchEnd`).
6. Escape dismisses via the central LIFO `overlayStack`: one capture-phase dispatcher (`useKeyboardShortcuts`) pops the top overlay, so stacked overlays close newest-first and per-instance Escape listeners never race. (It `stopPropagation`s, which also shadows the hook's own Escape so `onClose` fires once.) **One exception, and it is what makes that `stopPropagation` load-bearing rather than tidy: while an element is NATIVELY fullscreen the dispatcher stands down** (`dispatchEscape` returns `'fullscreen'`), because the browser takes that Escape to exit fullscreen and nothing in a keydown handler can stop it. Dismissing as well would make one keypress do two things. It still `stopPropagation`s, without `preventDefault`, precisely so the hook's own bubble-phase Escape cannot close the overlay behind the stand-down's back while the UA's fullscreen exit goes ahead. Pseudo-fullscreen is NOT covered: it registers on this same stack before the overlay, so LIFO already closes the overlay first and leaves fullscreen alone.
7. **Inert-behind**: while any `<Overlay>` is open, `<html>` gets `data-overlay-open` (ref-counted across stacked overlays) and CSS sets `.app-shell > * { pointer-events: none }`. Three things are re-enabled (`pointer-events: auto`): the overlay panels (`[data-overlay-panel]`), **the toggle that opened the overlay** (`[data-overlay-anchor]`, set by `<Overlay>` on its `anchor`), and the *overlay layer*'s mount (`[data-overlay-layer]`). The third exists because the layer does not always render outside `.app-shell`: while an app is natively fullscreen, `OverlayLayer` portals the whole overlay group into a mount inside the fullscreen app panel, which is the only subtree the browser paints, and that mount is deep inside `.app-shell` and would otherwise inherit the inert. The anchor re-enable is load-bearing, not cosmetic: the anchor lives inside `.app-shell`, so without it re-activating the toggle can't fire its own handler (rule 5's exemption would be dead, because the click would route through the outside-dismiss path instead, and a careful pointer / Playwright can't land on the inert toggle at all). The inert targets `.app-shell`'s **children**, never `.app-shell` itself, since `.app-shell` must stay a real hit target so an outside click LANDS ON IT and dismisses (a backdrop-less popover has no scrim, so a click on `.app-shell { pointer-events: none }` would fall through to `#app` and the outside-click path couldn't resolve a target, which is the regression that broke `message-route-panel`/`cc-slash-menu` clicks). `Toast` and the modal scrims render OUTSIDE `.app-shell` so they stay live. `pointer-events` is used rather than `inert` (and inherits down from the inert child) precisely because a descendant (the panel, the anchor) can override an inert ancestor, which the `inert` attribute can't. `force`-tap inert (non-anchor) targets in e2e: a real finger lands on `.app-shell` the same way.

`onClose` may return `false` to declare the call was a no-op — e.g. the overlay is already on its way out via a close animation and the originating signal is still `true`. Both the pointerdown path and the click-capture fallback honour it: a `false` return leaves the suppressor disarmed (and skips the inline swallow in the fallback), so the user's tap on a sibling button still reaches its handler. Returning `void` / `true` keeps the default dismiss+swallow. `closeDrawer` is the canonical user: during the 200ms slide-out it returns `false` so the hook stops eating neighbor taps mid-animation.

The older `ModalOverlay` (backdrop-`onClick`) component has been **deleted**: it dismissed but did not swallow, and couldn't serve click-through overlays. **Every** overlay panel now renders through `<Overlay>`: `useDismissOnOutside` has exactly one caller (`<Overlay>`), and `<Overlay>` is the only thing that registers a *panel* into the `overlayStack`. (The stack also takes **panel-less Escape registrants**, for a surface that should answer Escape without being an overlay: `ContentHeaderActions` pushes `pseudo-fullscreen` so Escape exits fullscreen, and `store/threadFilterPanel.ts` pushes `thread-filter-panel` so Escape closes the thread filter, which is a view inside the drawer pane rather than something floating over the app. `useKeyboardShortcuts` drives Escape against the stack; all of these are consumers/registrants of the Escape registry, not overlay panels.) Don't reintroduce a hand-rolled dismiss listener or a backdrop-only `onClick` close. Contract logic is unit-tested via `makeDismissHandlers` (`hooks/useAnchoredPopover.test.ts`) and the `<Overlay>` tripwires (`components/shared/__tests__/overlay-contract.test.ts`); behavior is covered end-to-end by `e2e/search-everywhere-close-mobile.spec.ts` (re-tapping the anchor closes) and `e2e/overlay-compose-dismiss-mobile.spec.ts` (a touch tap on a sibling `touchend` button dismisses without firing the action).

