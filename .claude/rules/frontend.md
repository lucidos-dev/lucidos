---
globs:
  - "crates/lucidos-app/src/**/*.ts"
  - "crates/lucidos-app/src/**/*.tsx"
  - "crates/lucidos-app/src/**/*.css"
---

# Frontend Conventions (TypeScript & CSS)

## Frontend Sends Intent, Backend Owns Logic

The frontend expresses **what** the user wants, not **how** the backend should do it. Don't make the frontend extract data from its local state just to pass it back.

## Drafts Are Threads — Bind Global Selections at Promotion Time

Compose drafts live in `threadMap` like any other thread (`meta.state === 'composing'`, `focusedThreadId` set). `sendCompose` (compose.ts) flips state to `'active'` BEFORE calling `sendMessage` — so `sendMessage` cannot tell first-send from follow-up by thread state alone. Every plausible signal (`threadMap.get` truthy, `focusedThreadId === null`, `meta.state === 'composing'`) gives a wrong answer for at least one of: optimistic insert, focused draft, or sendCompose's pre-flip.

Bind global compose-view selections (e.g. `selectedRepoId.value`) onto `meta.*` at promotion time inside `sendCompose`. Follow-ups via `sendMessage` then just read `meta.*`. Raw-new sends straight to `sendMessage` (no thread) read `selectedRepoId.value` directly — there's no thread to carry the binding.

Lifecycle states: `'composing' | 'active' | 'discarded' | 'archived'` (`ThreadComposeState`).

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
- Failed must look different from empty (error styling)
- Tab data must load on page reload via `useStartup.ts`
- Use `ApiError` + `toFailed()`. Use `useDelayedLoading(loadable)` for loaders — it returns true only after the load has been pending `SPINNER_DELAY_MS` (300ms), so fast loads never flash a loader. (Loaders are **delay-only**: a *minimum-visible* floor was tried and rejected — to actually hold a loader it must withhold already-loaded content, which feels sluggish. The smooth skeleton **exit** is handled by `<LoadingFade>` instead, see below.)
- **Never render a bare loading indicator (`<div class="loading-spinner" />`, a skeleton) immediately.** Gate it on `useDelayedLoading(loadable)` (Loadable) or `useDelayedFlag(active, delayMs?)` (boolean) from `hooks/useDelayedLoading.ts` — both delay the loader past `SPINNER_DELAY_MS`. Prefer a **skeleton** (`<ThreadSkeleton/>`, `<ListSkeleton/>`) for known-shape content (message threads, list rows); keep a plain spinner for inline status indicators and indeterminate "working" states. To smooth the skeleton→content handoff (so a shown skeleton doesn't hard-snap), wrap the loaded content in **`<LoadingFade showSkeleton={delayedFlag} skeleton={<ListSkeleton/>}>{loaded ? content : null}</LoadingFade>`** (`components/shared/LoadingFade.tsx`) — it crossfades the skeleton out as content fades in, without withholding content; ThreadView uses an equivalent fading overlay (`ThreadSkeletonOverlay`) so its scroll container is untouched. `useDelayedFlag` also backs non-loader fuses (e.g. the 8s "tap to reload" timeout); there it's purely the delay. **Full-screen surfaces gate the skeleton too** — including the workspace picker (`<LoadingFade showSkeleton={useDelayedFlag(listLoading)} …>`). The earlier "no competing content → show the skeleton immediately (ungated)" carve-out was wrong in practice: the picker renders its brand header + footer immediately AND its inline boot splash fades over it for ~0.45s on every open, so the sub-`SPINNER_DELAY_MS` window is never a bare blank panel — an ungated skeleton just *blinked* under the clearing splash on a fast local backend. The delay gate suppresses the skeleton on a fast load (the chrome/splash covers the brief empty→content transition) and shows it only on a genuinely slow one. Still wrap it in `<LoadingFade>` for the exit crossfade.
- Never fake errors as `loaded` with empty data

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
- **Error messages must name the entity and the origin — never a bare generic.** A user-facing error has to say *what* failed (the id/name/path — `App "demo-director" no longer exists`, not `App no longer exists`) and, when the action wasn't a direct click, *where it came from* (`… (requested by thread "X")` / `… (requested by an app)`). A generic message with the identity stripped out is a swallowed error: the user can't tell what's missing or who asked. Thread the originating context to the error site rather than dropping it. (Regression that motivated this: a `NavigationRequested` from a sibling thread toasted "App no longer exists" with no id and no source, for an app that existed on disk.)
- **Never conclude "X doesn't exist" from a cached projection — reconfirm against the source of truth first.** Disk- or DB-backed lists (`appsList`, `artifacts`, …) are caches refreshed by SSE events; they go momentarily stale when a sibling thread mutates state. A definitive "gone" verdict (and its toast) must come *after* a re-fetch that re-reads the source (e.g. `openAppById` re-scans disk on a cache miss), not from a `list.some(...)` pre-check against the possibly-stale cache. A stale-cache pre-check that short-circuits the re-fetch is a swallowed error — it reports live entities as deleted.
- **A disk-/DB-backed list whose freshness depends on a refresh event ⇒ EVERY mutation path must emit that event.** The list is loaded by re-scanning the source (e.g. `loadApps()` → `/apps` scans `data/apps/`); the cache only updates live because something emits the `App*`/`Artifact*`/… SSE event that the frontend's `entityReferences` arm reloads on. If you add a code path that mutates the underlying store (a new tool, endpoint, or write site) and *don't* emit the matching refresh event, every open page silently shows stale data until a full reload. When you touch a mutation site, check it emits the event the list listens on. (Regression: the chat `write_file` tool emitted `App*` only for `artifacts/` paths, so apps created via raw file writes never refreshed the list.)

### Carve-out: best-effort telemetry

A narrow `console.warn` (without a paired toast / `Loadable` failed) is acceptable for **best-effort telemetry that runs without user intent and recovers on its own**:

- presence pongs, device-visibility heartbeats, push-subscription keepalive
- startup probes that the user did not initiate
- background dev-breadcrumbs whose user-facing error surface lives elsewhere (e.g. a parallel `postAppCapture` that delivers the failure to the LLM)
- tab-close `keepalive` flushes where the document is tearing down and no toast can render

Required at every carve-out site:

1. **A justifying comment** above the `console.warn` saying WHY a toast is wrong and HOW the user still finds out if it matters (e.g. "next push attempt re-triggers the user-facing flow", "engine's deadline-then-default-to-push fallback covers a missed pong", "tab is unloading"). No comment → not a carve-out → fix it.
2. **No mutating user intent on the line that failed.** If the user clicked a button, they are owed a toast — even if the action is "cancel" or "dismiss". The carve-out is for code that runs whether the user did anything or not.
3. **Self-recovery.** The next probe / heartbeat / push attempt must either succeed silently or escalate via a different path that does surface to the user.

For paths that run on a schedule and could fail repeatedly (polling), prefer the tracked failure counter (`utils/failureCounter.ts`): silent below a threshold, single toast at threshold N, reset on success. That keeps the noise floor low without losing the signal entirely.

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

Any callsite that puts something into the right-hand **content** pane — sets `panelOverlay`, switches `activeMenuItem`, opens a settings subview, opens an app / file / URL / trigger / change — MUST call `revealContentPane()` from `store/actions/pane.ts`. This is the canonical helper: mobile users get swiped to the content pane; desktop users get the **Content pane group activated** (`focusedPane = 'content'`) — so keyboard Tab routes into the navigated view via `handlePaneTab` — plus a collapsed split re-expanded. Skipping it produces a silent no-op on whichever surface didn't already happen to be visible — the user's tap "did nothing" — or, on desktop, lands the view but leaves Tab stuck on the previously-focused pane.

The desktop focus half is **signal-only**, matching the pointer-down `focusPane`: it sets `focusedPane` but does not move DOM focus, so the first Tab pulls focus in (`paneTabTarget`'s `activeIndex < 0` branch) without yanking focus mid-navigation. Its mirror is **`revealThreadPane()`** (also in `store/actions/pane.ts`): navigation that lands on a **thread** — an existing thread, the empty compose view, or a brand-new thread spawned from another pane — re-activates the **Threads** pane group (`focusedPane = 'thread'`) — but only from the cross-group case (`focusedPane === 'content'`), so an existing `'drawer'`/`'thread'` focus is left alone and drawer ↑/↓ browsing isn't disturbed; mobile swipes to the thread pane. The three callers are `focusThread` (navigate to an existing thread), `unfocusThread` (the compose view — New-thread buttons, the new-chat shortcut, archiving the last review, a new-chat NavigationRequested), and `sendMessage`'s raw-new-thread path (`isNewThread`, e.g. the new-app form submitting from the content pane). A raw-new send must reveal the thread pane the same way `focusThread` does — without it, a thread created off the thread pane (the new-app flow) stays invisible. **The one opt-out is `unfocusThread({ revealPane: false })`** — stale-pointer CLEANUP, not navigation. `ThreadView` clears a `focusedThreadId` whose thread left the map during *render*, and on mobile every pane is mounted at once (`MobileSwipeContainer`), so a reveal there would swipe a user on the content pane to the thread pane mid-render. Cleanup must not move the visible pane; user-intent unfocus must.

The rule has a sharp shape:

- **User-intent helpers own the call.** `switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`, `openApp`/`openAppById`, `openFilePreview`, `openUrl`, `openRepoFilePreview`, `navigateToTrigger`, and every `handleNavigationRequest` branch that lands content all call `revealContentPane()` at the end.
- **Pure plumbing does NOT.** `setActiveMenu` (in `store/actions/menu.ts`) is signal-state plumbing used by multiple flows; it does NOT swipe panes. Same for any future helper that only mutates store signals without expressing user intent. The earlier `setActiveMenu` carried a conditional `navigateToPane('content')` gated on `item !== prev && mobileView === 'thread'` — the gate silently dropped the swipe when the user re-tapped the current item or wasn't on the chat pane. That conditional is gone; callers are explicit.
- **Don't reach for `navigateToPane('content'|'thread')` directly under an `isMobile()` gate.** That pattern only covers the mobile half — it leaves desktop users with a collapsed split with no visible feedback. Content navigation uses `revealContentPane`; thread navigation uses `revealThreadPane` — both handle the desktop focused-pane-group half too. A bare `navigateToPane('thread')` paired after `unfocusThread`/`focusThread` is the smell that the reveal belongs centralized in the helper (those manual pairings were removed when `revealThreadPane` was extracted).

When you add a new navigation entry point, add it to the test mirror in `crates/lucidos-app/src/store/actions/menu.test.ts` (or the equivalent suite) so the regression is pinned.

## Pane Resize — Deferred Snap & Header Sync

Desktop pane resizing follows the *deferred snap* contract (see `docs/glossary.md` § Deferred snap; implementation in `components/layout/splitHelpers.ts`): a divider drag lands wherever released, and a below-minimum pane snaps to its minimum or to hidden ~400ms later. Three rules keep it honest:

- **Never clamp or collapse mid-drag.** Minimum widths and collapse-state flips (`data-thread-collapsed` / `data-content-collapsed` / `data-thread-drawer-open`) belong to the post-release snap. Mid-drag flips swap header icon groups between hosts while the pointer wiggles across a pane edge — the "icons dance between the headers" bug.
- **New header regions join the resize kill list.** Any absolutely-positioned `.app-header` section with a `left`/`width` transition MUST be added to the `:root[data-pane-resizing]` `transition: none` block in `styles/panels/shell.css`, or it will visibly lag behind the panes during a drag. Header sections and panes must share `var(--duration-slow) ease` for their geometry transitions so snaps arrive together.
- **Header regions hide by fading, never by `display: none` or unmount.** A collapse/toggle hides header elements via `opacity: 0; visibility: hidden; pointer-events: none` riding the same `var(--duration-slow)` transitions as the geometry — `display: none`, a removed conditional render, or a keyframe keyed on the state attribute pops the element out at the *start* of the pane animation while everything else is still sliding. Same rule for pane *content*: keep it mounted through the exit animation (`useLingeringFlag` in `hooks/useDelayedLoading.ts` — see the thread drawer's list).

Header overlap is prevented structurally: the content side (`.content-header-elements`) is a flex region pinned between the split divider and the header's right padding — icons and the centered title can shrink/clip but never overlap. Don't reintroduce absolutely-positioned children with hardcoded width reservations inside it.

**Keyboard resize clamps immediately — no deferred snap.** The Narrow/Widen pane shortcuts (`stepThreadPaneWidth` / `stepThreadDrawerWidth` in `store/actions/pane.ts`; pure math in `computeStepRatio` / `computeDrawerStepWidth`) move a divider by `KEYBOARD_RESIZE_STEP_PX` per press, clamped to the pane minimums on the spot, and never collapse a pane — collapse stays with the toggles (`toggleThreadPane` / `toggleContentPane` / `toggleThreads`) and the drag-release snap. A step cancels any pending snap so a stale drag correction can't overwrite the explicit keystroke. Deferred snap is a *drag* contract (the correction must not fight the pointer); a discrete keystroke has no mid-gesture state, so immediate clamping is correct there.

**Maximize a *pane group*; focus the Conversation drawer.** Two desktop shortcuts express the Conversation↔Canvas back-and-forth (see `docs/glossary.md` §§ Pane group, Split): `⌘⇧↵` (`maximizePaneGroup` → `toggleMaximizeFocusedPaneGroup` in `store/actions/pane.ts`) toggles the focused *pane group* full-width via `setSplitRatio(1|0)` and restores the remembered ratio on a second press; `⌘⇧1` (`toggleThreadDrawer` → `focusOrToggleThreadDrawer`) is a three-stage open+focus / focus / close that moves DOM focus into the drawer so its existing ↑/↓/Enter list-nav lights up (re-expanding the Conversation side first if collapsed). Both are desktop-only (`isMobile()` guard) and rebindable. The drawer **icon** stays a pure show/hide (`toggleThreads`) — only the shortcut is focus-aware, mirroring the `focusPane` (pointer) vs `focusPaneAndControl` (keyboard) split.

## Modals & Popovers: Click-Outside Dismiss

**Non-negotiable, app-wide principle: any click outside an overlay closes it and does NOT activate anything else.** Every modal, popover, dropdown, anchored panel, command palette, bottom sheet — anything overlaid on top of other UI — dismisses on an outside click AND swallows that click, so the button / chat row / link under the cursor never also fires. The user meant "get rid of this thing", not "get rid of it *and* press whatever was behind it". Two contracts in one, both always required. Escape dismisses too. And while it's open, **the UI behind it is inert** — no hover highlight, no activation (the hover analog of the swallow): the overlay is the focused thing, so the stuff behind it must not react. Even a backdrop-less popover (a dropdown, the control panel) makes the UI behind go inert.

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

**Always pass the toggle that opened the overlay as `anchor`. Never `null` for a toggle-opened overlay.** The anchor is exempt from the outside-pointerdown dismiss, so re-activating the toggle closes via the toggle's OWN handler rather than being raced by the dismiss. With `anchor={null}`, on touch the outside `pointerdown` closes the overlay and then the toggle's `touchend` re-flips it open — it never closes (the SearchEverywhere bug). When the toggle and the overlay live in different components, stash the element in a signal at click time and pass `signal.value` — `controlPanelAnchor`, `drawerAnchor`, `searchEverywhereAnchor` are the precedents.

What `<Overlay>` does for you (see `makeDismissHandlers` / `store/overlayStack.ts` for the mechanism):

1. `pointerdown` outside the panel + anchor → `onClose()` and arm "swallow the next paired event" (the `touchend`/`click` the browser is about to dispatch). **The swallow is armed on a one-shot `document` listener that OUTLIVES the overlay's unmount** (`installPairedSwallow`): closing the overlay re-renders and tears the overlay's own listeners down in the microtask checkpoint *between* the pointerdown task and the next event task — i.e. BEFORE the gesture's paired `touchend`/`click` fires — so a swallow that lived only on the overlay's listeners would already be gone (the original compose-on-first-tap bug survived a first attempt for exactly this reason). The one-shot self-disarms on the first swallowed event (or `touchcancel`/`pointercancel`, or a short fuse) so a later unrelated tap is never eaten.
2. The paired `touchend` outside the panel + anchor is captured at `document` in the **capture phase** (so it precedes the target button's own bubble-phase `onTouchEnd`) and `stopPropagation`+`preventDefault`'d. This covers touch buttons that run their action on `onTouchEnd` and `preventDefault()` the synthetic click (the iOS keyboard-nudge pattern in `composeHandlers`): without it the outside pointerdown dismisses the overlay but the button still fires its action on the same tap (the compose-on-first-tap bug), and — because the button cancels the synthetic click — no `click` ever arrives. The `touchend`'s `preventDefault` cancels the synthetic click. Anchor / inside-panel touches are never swallowed. (The overlay's own open-gated `touchend` handler also swallows, for the rarer same-task case where it hasn't been torn down yet — complementary to the one-shot, not redundant.)
3. The paired `click` (the mouse case — on touch the `touchend` already consumed the one-shot) is captured at `document` and `stopPropagation`+`preventDefault`'d — nothing downstream fires.
4. A `click` that arrives **without** a preceding outside pointerdown (synthetic `HTMLElement.click()` from a keyboard shortcut, an e2e test driver, or any other programmatic source) falls into a click-capture fallback: same `isOutsidePointerTarget` check, same `onClose()` + swallow. Without this branch the contract silently breaks for any caller that drives dismiss via synthetic clicks (the thread-filter dropdown e2e tests are the canary).
5. The anchor is exempted (re-activating it must toggle via the caller's `onClick` / `onTouchEnd`).
6. Escape dismisses via the central LIFO `overlayStack` — one capture-phase dispatcher (`useKeyboardShortcuts`) pops the top overlay, so stacked overlays close newest-first and per-instance Escape listeners never race. (It `stopPropagation`s, which also shadows the hook's own Escape so `onClose` fires once.)
7. **Inert-behind** — while any `<Overlay>` is open, `<html>` gets `data-overlay-open` (ref-counted across stacked overlays) and CSS sets `.app-shell > * { pointer-events: none }`, so every element behind the overlay stops hover-highlighting and can't activate. Two things are re-enabled (`pointer-events: auto`): the overlay panels (`[data-overlay-panel]`) and **the toggle that opened the overlay** (`[data-overlay-anchor]`, set by `<Overlay>` on its `anchor`). The anchor re-enable is load-bearing, not cosmetic — the anchor lives inside `.app-shell`, so without it re-activating the toggle can't fire its own handler (rule 5's anchor exemption would be dead: the click would route through the outside-dismiss path instead, and a careful pointer / Playwright can't land on the inert toggle at all). The inert targets `.app-shell`'s **children**, never `.app-shell` itself — `.app-shell` must stay a real hit target so an outside click LANDS ON IT and dismisses (a backdrop-less popover has no scrim, so a click on `.app-shell { pointer-events: none }` would fall through to `#app` and the outside-click path couldn't resolve a target — this is exactly the regression that broke `message-route-panel`/`cc-slash-menu` clicks). `Toast` and the modal scrims render OUTSIDE `.app-shell` so they stay live. This is why even backdrop-less popovers (dropdowns, the control panel) make the UI behind inert. `pointer-events` is used rather than `inert` (and inherits down from the inert child) precisely because a descendant (the panel, the anchor) can override an inert ancestor, which the `inert` attribute can't. `force`-tap inert (non-anchor) targets in e2e — a real finger lands on `.app-shell` the same way.

`onClose` may return `false` to declare the call was a no-op — e.g. the overlay is already on its way out via a close animation and the originating signal is still `true`. Both the pointerdown path and the click-capture fallback honour the return value: a `false` return leaves the suppressor disarmed (and skips the inline swallow in the fallback), so the user's tap on a sibling button still reaches its handler. Returning `void` / `true` keeps the default dismiss+swallow. `closeDrawer` is the canonical user of this: during the 200ms slide-out it returns `false` so the hook stops eating neighbor taps mid-animation.

Why both halves: a CSS-only "click closes me" via backdrop element solves dismiss but leaves the underlying click free to fire a different button. A bare `pointerdown` listener that calls `onClose` but doesn't swallow the paired event does the same. The "paired event" is `click` on mouse but `touchend` on touch — a touch button that runs its action on `onTouchEnd` and `preventDefault()`s the synthetic click (the `composeHandlers` iOS keyboard-nudge pattern) never dispatches a `click`, so the contract swallows the `touchend` too (capture phase, before the target's bubble-phase handler). The user expects "I clicked/tapped away from this thing" to be a single atomic action — they did NOT mean to also fire whatever was under their cursor/finger. Get this wrong and a user dismissing a settings popover can accidentally send a chat message, open a different app, start a new thread, or trigger a destructive action.

The older `ModalOverlay` (backdrop-`onClick`) component has been **deleted** — it dismissed but did not swallow, and couldn't serve click-through overlays. **Every** overlay panel now renders through `<Overlay>`: `useDismissOnOutside` has exactly one caller (`<Overlay>`), and `<Overlay>` is the only thing that registers a *panel* into the `overlayStack`. (The stack also takes a panel-less Escape registrant — `ContentHeaderActions` pushes `pseudo-fullscreen` so Escape exits fullscreen — and `useKeyboardShortcuts` drives Escape against it; both are consumers/registrants of the Escape registry, not overlay panels.) Don't reintroduce a hand-rolled dismiss listener or a backdrop-only `onClick` close. Contract logic is unit-tested via `makeDismissHandlers` (`hooks/useAnchoredPopover.test.ts`) and the `<Overlay>` tripwires (`components/shared/__tests__/overlay-contract.test.ts`); the behavior is covered end-to-end by `e2e/search-everywhere-close-mobile.spec.ts` (re-tapping the anchor closes) and `e2e/overlay-compose-dismiss-mobile.spec.ts` (a touch tap on a sibling `touchend` button dismisses without firing the action).

## CSS & Component Rules

- **Tab title**: `(count) Lucidos` (count first for narrow tabs)
- **No system dialogs**: Use `showToast(msg, type)` / `await showConfirm(msg, okLabel)`
- **No native tooltips**: Use `data-tooltip="text"`. Desktop-only.
- **Component CSS is split three ways by audience — put each rule in the right file:**
  - **Reusable (host + apps)** → `styles/global/shared-components.css` (the `.action-btn` family, `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`, `h1`–`h6`). This is a SINGLE SOURCE OF TRUTH: the engine `include_str!`s this exact file and appends it to `/api/v1/sdk-iframe.css` (`crates/lucidos-engine/src/api/sdk.rs`), so a class added/changed here ships to the host AND every app iframe at once. **Never copy these rules into `sdk_iframe.css` — edit the shared file.**
  - **Host-chrome only (host bundle, NEVER served to apps)** → `styles/global/host-components.css` (the custom `<Dropdown>` + `.nav-history-*`, `.send-cancel-*` morph, `.icon-btn.header-icon`/`.filter-active`/`.pinned` variants, `.list-row.flip-animating`). Imported by `global.css` AFTER `shared-components.css` so source-order overrides of a shared base class still win.
  - **Iframe-only (apps, not the host)** → the engine's `crates/lucidos-engine/src/api/sdk_iframe.css` (e.g. `.action-btn-secondary`, the `lucidos.ui.Select` `.lucidos-select` styles) — keeps them out of the host bundle as dead code.
  - Structural host chrome (`#tooltip`, `#app`, `body`, the `:root`/theme token blocks) stays in `base.css`.
  - When you add an app-facing class to `shared-components.css`, also add it to the component-class table in `system-knowhow/js-sdk.md` (it's the app-author-facing contract).
- **List rows**: `.list-row` / `.list-row-info` / `.list-row-actions` (in `shared-components.css`)
- **Action buttons**: `.action-btn` (in `shared-components.css`). Variants are additive: `class="action-btn action-btn-confirm"`.
  - `.action-btn` — default (blue). Neutral: Edit, Open, Restart, Prev/Next, Retry.
  - `.action-btn-confirm` — green. Positive: Apply, Accept, Confirm.
  - `.action-btn-danger` — red. Destructive: Delete, Discard, Remove, Cancel.
- **Auto-expanding textareas**: `AutoTextarea` from `components/shared/AutoTextarea.tsx`. Enter submits, Shift+Enter newline.
- **All sizes `rem`**: Divide px by 16 (4px→0.25rem, 8px→0.5rem, 16px→1rem). Exceptions: `1px` borders, `0px` env(), `@media`, `box-shadow`.
- **No `<select>`**: Use `Dropdown` from `components/shared/Dropdown.tsx`
- **No `id` on dual-rendered components**: `App.tsx` mounts only the visible layout's pane tree (`SplitLayout` on desktop, `MobileSwipeContainer` on mobile — dual-mounting the panes was removed because every signal write fanned out to both subtrees). But per-layout copies still exist in the header chrome (`ControlPanel` renders in both `AppHeader` and `MobileAppHeader`), and the mounted layout swaps at runtime when the viewport crosses the breakpoint — so `id` attributes remain unsafe. Use `data-role="name"` + `querySelectorAll`. Cross-component: `getVisiblePromptInput()` in `promptFocus.ts`. **Debug hint:** if something works on desktop but fails on mobile (or vice versa), check whether you're hitting the wrong layout's copy — inspect `getBoundingClientRect()` for 0x0 dimensions.
- **Heavy-mount children that render in both layouts must skip the inactive one**: Iframes, video/audio players, big WebGL canvases, anything that fetches on mount — if a component has a copy in each layout, both copies trigger the work, doubling network and resource cost (e.g., before the pane single-mount fix, opening an app fetched `/api/v1/sdk-prefs.js` twice). Pane children mount once now; the rule still binds for chrome with per-layout copies. Pattern: the parent takes `layout: 'desktop' | 'mobile'` and forwards it to the heavy child; child gates the render with `layout === (viewportIsMobile.value ? 'mobile' : 'desktop')`. `viewportIsMobile` is the reactive signal in `utils/viewport.ts`. Example: `AppUiInline.tsx` (gate retained from the dual-mount era; harmless now that `ContentPane` mounts once).
- **No `getElementById()`**: Banned except `#app`. Use `querySelector`/`querySelectorAll`
