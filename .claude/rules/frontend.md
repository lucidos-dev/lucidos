---
paths:
  - "crates/lucidos-app/src/**/*.ts"
  - "crates/lucidos-app/src/**/*.tsx"
---

# Frontend Conventions (TypeScript & State)

The CSS half lives in `.claude/rules/frontend-css.md`. This file keeps what you must not break. Reasons and worked examples: [`docs/frontend-conventions.md`](../../docs/frontend-conventions.md).

## Frontend Sends Intent, Backend Owns Logic

The frontend expresses **what** the user wants, not **how** the backend should do it. Don't make the frontend extract data from its local state just to pass it back.

## Drafts Are Threads — Per-Draft Selections, Bound at Promotion Time

A compose draft is a thread in `threadMap` (`meta.state === 'composing'`). `sendCompose` flips it to `'active'` before `sendMessage`, so no thread-state signal tells a first send from a follow-up.

- **Every compose-view dropdown is PER-DRAFT** (scope, coding-agent backend, model and reasoning). The value lives in `composeSelections[id]` as an override, read through `resolve*(threadId)`. Reading a mutable global (`selectedScope`, `codingAgentPending*`) in a compose surface leaks one draft's pick into every other.
- **Writes go through `updateComposeSelection(threadId-or-null, …)`**, and scope through `applyDestination`. A bare `setChatModelSelection` from a compose surface is the bug.
- **No compose pick writes an account preference** (`chat_model`, `chat_reasoning_efforts`, `coding_agent_default`), because each is the fallback of every override-less draft. A backend pick is the one exception (ADR 0247).
- **An existing draft resolves scope as `override.scope ?? {kind:'lucidos'}`.** `selectedScope` only seeds a NEW draft, in `ensureFocusedComposeThread`. Picks made before a draft exists use `pendingComposeSelection`.
- **Selections are server-synced** (ADR 0437). At promotion, `sendCompose` binds them onto the send and clears them.

## Async Data Loading — `Loadable<T>`

Every async-fetched value **must** use `Loadable<T>` from `store/types.ts`:

```typescript
type Loadable<T> =
  | { status: 'not-loaded' }
  | { status: 'loading' }
  | { status: 'loaded'; data: T }
  | { status: 'failed'; error: string; httpCode?: number };
```

- Store signals: `signal<Loadable<T>>({ status: 'not-loaded' })`, never bare arrays.
- Handle **all four states**. `loaded ? data : []` is a bug (masks loading as empty).
- Failed draws `<LoadableError>`, never empty. Never fake errors as `loaded` with empty data.
- Tab data loads on page reload via `store/startup.ts`. Use `ApiError` + `toFailed()`.
- **Never render a loader immediately.** Gate it on `useDelayedLoading(loadable)` or `useDelayedFlag(active, delayMs?)`. Loaders are delay-only, with no minimum-visible floor.
- **Data loading draws a skeleton, always.** No page spinner, no "Loading…" text. `.mini-spinner` is only for an indeterminate working state, or a multi-read list's trailing row. Wrap the skeleton in `<LoadingFade>`. Full-screen surfaces gate it too.
- **The delay gate has NO exception.** Never "hold" a skeleton ahead of it (ADR 0081). Cut the render cost instead.
- **An animated element must SURVIVE the conditional renders around it, so key it.** A remount cannot transition.
- **A loader the gate let through is OPAQUE.** An entrance fade is a second gate.
- **An anchored popover does NOT skeleton-swap.** Render its structure at natural height and dim the value-bearing controls (`NetworkAccessPopover.tsx`). A form's edit lives INSIDE the loaded state, and reopening refetches.
- **Build every skeleton from the row's OWN component**, never a hand-drawn shimmer tree. Use `useSkeleton()`, `<SkText>`, `<SkBlock>` and `<ListSkeletonOf row={() => <MyRow/>} fill? containerClass=… />`.
- **A code-split view draws nothing until its chunk lands**, so eager what must appear at once (ADR 0288).
- **A single VALUE gets no skeleton**: hold its box, right-anchored. **A single CONTROL** wears the real control's box (`<DropdownSkeleton w=…/>`).
- **A surface with several reads shares ONE delay gate.** Derive each slot's pending flag from the first render, never from the effect that starts the read (`backupSlotsPending`).

`skeleton-guard.test.ts` and `loadable-guard.test.ts` (`components/shared/__tests__/`) enforce this, default-deny. An exemption names its rule from a closed tag set, and a stale entry fails.

## A Content Panel Joins the Panel Refresh Contract

**Every content-pane view that shows fetched data registers its refresh with `usePanelRefresh`.** Pull to refresh and the header's Refresh both run it. The contract is `docs/glossary.md` § Panel refresh contract.

- **Register where the data is owned.** A settings section drawn by a `SettingsView` function takes a row in `SETTINGS_SECTION_REFRESH`. A page with two data owners registers two.
- **Return the promise**, so the header spinner has something to wait on.
- **A re-read keeps what is shown** (`setLoadingIfFresh`, `loadingIfFresh`, `keepLoadedWhileRefetching`). Blank only when the read names a different thing.
- **Pause, never clobber, a draft** (pass `null` while a form holds an unsaved edit). **A new view takes a row** in `store/__tests__/panel-refresh-coverage.test.ts`.

## Every Expand and Collapse Rolls

**Nothing the user expands or collapses appears or vanishes in one frame.**

- **An inline toggle carries `aria-expanded`**, and its content rolls through `<Disclosure open>`, never a bare `{open && …}`. A popover toggle pairs it with `aria-haspopup` and opens through `<Overlay>`. A chevron turns with the roll, on a `transform` transition.
- **A fold is one element, never two swapped.** A mount gated on an open flag rolls too, wherever its toggle lives.
- **A row arriving in a live list rolls in through `<ArrivalList>`.**
- **Whatever animates in animates out, unasked.** Prefer a drawing held with `useLingeringFlag` and a scaled timer (`frontend-css.md` § Every duration is scaled). An overlay open through its exit returns `false` from `onClose` until it has gone.
- **Enforced** by `expand-collapse-rolls-guard.test.ts`.

## No Hidden Errors — Fail Fast, Tell the User

Errors must propagate to the frontend — never silently skip, swallow, or log-only. `console.error` alone is not acceptable.

- **Rust:** Use `?` to propagate. `catch { log; return empty }` is a bug.
- **TypeScript:** Use `showToast(msg, 'error')` or `Loadable` failed state. No fire-and-forget `promise.then(...)` without `.catch()`. Avoid dynamic `import()` for actions (circular deps cause silent failures).
- **The chain:** Backend error → HTTP → `ApiError` → `Loadable` failed → visible to user. No link may drop the error.
- **A user-facing message never carries a raw response body.** `throwIfNotOk` (`api/client/_core.ts`) is the one place a body becomes a `reason`. `showToast` is the second net (`clampToastText`, one-line errors), and neither may be re-widened. A toast's title is explicit: `showToast(message, type, { title })`.
- **Error messages name the entity and the origin.** `App "demo-director" no longer exists`, not `App no longer exists`, plus `(requested by thread "X")` when it was not a direct click.
- **Never conclude "X doesn't exist" from a cached projection.** Re-confirm against the source first.
- **A list whose freshness depends on a refresh event needs EVERY mutation path to emit it.** Check this whenever you touch a mutation site.

## A UI Surface Owns Its Subscription

**All UI state is updated through SSE events.** This is the receiving half, enforced by tests. Reasoning in ADR 0118.

- **A surface that reads state on mount subscribes to the event that mutates it.** Reading a store signal IS the subscription. A surface that fetches its own data uses `useVersionedRefresh(xVersion.value, paused, reload)`.
- **Build a signal only SSE frames move through its gap factory** (`gapRefreshedVersion()`, `gapClearedProgress<T>()` in `store/eventGap.ts`). Never hand-write a gap repair. A signal no frame feeds takes a `NOT_GAP_STATE` row saying why.
- **Never a poll, an interval, a focus or visibility listener, or an invalidate-on-navigation.** `sse-event-coverage.test.ts` scans for all four.
- **A form field over entity state uses `useServerBackedField`.** Seeding a `useState` from an entity is the bug it replaces. A whole-editor draft pauses the re-read instead (`AllowlistEditor`).
- **Every `SystemEvent` is answered or written down**: an arm in `thread-sync.ts` / `entityReferences.ts`, or a `NO_UI_STATE` row.

### Carve-out: best-effort telemetry

A narrow `console.warn` (no toast, no `Loadable` failed) is acceptable only for **best-effort telemetry that runs without user intent and recovers on its own**. Examples: presence pongs, heartbeats, push keepalive, startup probes, tab-close `keepalive` flushes. Every site needs:

1. **A comment** saying WHY a toast is wrong and HOW the user still finds out.
2. **No mutating user intent on the failed line.** A user who clicked is owed a toast.
3. **Self-recovery** on the next attempt, or an escalation that surfaces.

A repeating scheduled path uses `utils/failureCounter.ts`.

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

Any callsite that puts something into the right-hand **content** pane MUST call `revealContentPane()` (`store/actions/pane.ts`). That covers setting `panelOverlay`, switching `activeMenuItem`, and opening a settings subview, app, file, URL, trigger or change. Skipping it is a silent no-op where the pane was not already visible.

- **Its mirror is `revealThreadPane()`**, for any landing on a **thread**. **Both halves are required, and the re-expand gets forgotten.** `focusThread`, `unfocusThread`, `sendMessage`'s raw-new path and `sendSeededPrompt` call it.
- **`{ revealPane: false }` means BOOKKEEPING, not navigation** (stale-pointer cleanup, the post-archive hand-off). A user-intent focus always reveals.
- **User-intent helpers own the call** (`switchMenuItem`, `openApp`, `openFilePreview`, every `handleNavigationRequest` branch). **Pure plumbing does NOT** (`setActiveMenu`).
- **Never call `navigateToPane('content'|'thread')` under an `isMobile()` gate.** It covers only the mobile half.
- **A new entry point joins the mirror** in `store/actions/menu.test.ts`.
- **The nav history row at the cursor describes what is on screen.** Call `pushNavState()` in the same task as the state change, never after an `await`.
- **Add no per-callsite fade.** The navigation cover smooths the swap. A new view owes IDENTITY: a `PanelOverlay` variant showing one of several things resolves in `components/layout/contentViewKey.ts`.

## Pane Resize: Clamped Dividers & Header Sync

Desktop pane resizing follows the *clamped divider* contract (`docs/glossary.md` § Clamped divider; `components/layout/splitHelpers.ts`; ADR 0056).

- **A DRAG never collapses a pane; it only clamps.** Collapse belongs to the toggles, shortcuts and double-clicks. A collapsing drag re-opens the "icons dance between the headers" bug.
- **Every pane minimum is derived and lives in `store/paneMinimums.ts`**, the same for every client (ADR 0058). `splitHelpers.ts` stays pure: the caller measures.
- **A pane that cannot fit its floor yields; it never squeezes** (ADRs 0495, 0498). The Conversation never yields. Read `drawnSplitRatio` and the `drawn*` values, write `splitRatio`, and show the Canvas through `contentShownRatio`.
- **A header region with a `left`/`width` transition joins the `:root[data-pane-resizing]` `transition: none` block** in `styles/panels/shell.css`. Geometry transitions share `var(--duration-slow) ease`.
- **Header regions hide by fading, never by `display: none` or unmount.**
- **A single CONTROL is never two copies, and never fades where something can slide under it** (ADR 0276). A moving control is ONE element whose `left` transitions, with no `opacity`. A disappearing one shrinks in place behind a constant `clip-path`, never `overflow: clip`. Example: `.thread-toggle-slot`.
- **Header titles centre on the PANE, with a DERIVED reserve** (`--header-icon-box`; ADR 0399). Never a `flex: 1` middle or a guessed constant.
- **Keyboard resize and drag share one wall**: the step helpers never collapse.
- **Shell shortcuts must reach the host from an iframe**: through the SDK's `keyboardForward.ts`, `previewFrameBridge.ts` (ADR 0322) or `bridgePreviewIframeShortcuts`.

## Modals & Popovers: Click-Outside Dismiss

**Non-negotiable: any click outside an overlay closes it and does NOT activate anything else.** Every modal, popover, dropdown, palette and sheet dismisses on an outside click AND swallows that click. Escape dismisses too. While it is open **the UI behind is inert**, even for a backdrop-less popover.

**Build every overlay through `<Overlay>`** (`components/shared/Overlay.tsx`). Never hand-roll a `mousedown`/`pointerdown`/`click` close handler, close from a backdrop-only `onClick`, or call `useDismissOnOutside` from a feature component.

**Pass the toggle that opened the overlay as `anchor`, never `null`**, so re-activating it closes through its own handler. When toggle and overlay live in different components, stash the element in a signal (`controlPanelAnchor`, `drawerAnchor`). **A GESTURE-opened overlay has no toggle, so `anchor={null}` is right there**, and a row is never the anchor.

What `<Overlay>` does for you (`makeDismissHandlers`, `store/overlayStack.ts`):

0. **Only the topmost PANEL answers a pointer** (`topPanelOverlay`, never the raw `topOverlay`). Paint order follows `overlayStackDepth`. A modal outranks toasts, except an error or warning.
1. `pointerdown` outside the panel and anchor → `onClose()` and a swallow of the next paired event. The swallow is a one-shot `document` listener that OUTLIVES the unmount.
2. The paired `touchend` is captured at `document` and stopped.
3. The paired `click` is captured and stopped.
4. A `click` with no outside pointerdown (a synthetic `.click()`) takes a click-capture fallback. A click dispatched WITHIN an inside click's dispatch counts as inside.
5. The anchor is exempt, and so is the toast layer (`.toast-container`).
6. Escape dismisses through the LIFO `overlayStack`, standing down while an element is natively fullscreen.
7. **Inert-behind:** `<html>` gets `data-overlay-open`, and CSS sets `.app-shell > * { pointer-events: none }`. It re-enables `[data-overlay-panel]`, `[data-overlay-anchor]` and `[data-overlay-layer]`. It targets `.app-shell`'s **children**, never `.app-shell` itself, which must stay a hit target.
8. **Keyboard focus** (ADR 0335): a dialog contains Tab, an anchored popover wraps it only while focus is inside. A new overlay adds no Tab code.

`onClose` may return `false` to declare a no-op, as `closeDrawer` does during its slide-out. A STEP inside a panel answers Escape through the stack with `useEscapeStep`. Only `<Overlay>` registers a panel into `overlayStack`.

