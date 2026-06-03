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
- Use `ApiError` + `toFailed()`. Use `useDelayedLoading(loadable)` for spinners (300ms delay)
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

Tool called 3+ times on same target → force-break with error.

## Navigation That Lands Content Must Call `revealContentPane()`

Any callsite that puts something into the right-hand **content** pane — sets `panelOverlay`, switches `activeMenuItem`, opens a settings subview, opens an app / file / URL / trigger / change — MUST call `revealContentPane()` from `store/actions/pane.ts`. This is the canonical helper: mobile users get swiped to the content pane; desktop users with a collapsed split get the split re-expanded. Skipping it produces a silent no-op on whichever surface didn't already happen to be visible — the user's tap "did nothing".

The rule has a sharp shape:

- **User-intent helpers own the call.** `switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`, `openApp`/`openAppById`, `openFilePreview`, `openUrl`, `openRepoFilePreview`, `navigateToTrigger`, and every `handleNavigationRequest` branch that lands content all call `revealContentPane()` at the end.
- **Pure plumbing does NOT.** `setActiveMenu` (in `store/actions/menu.ts`) is signal-state plumbing used by multiple flows; it does NOT swipe panes. Same for any future helper that only mutates store signals without expressing user intent. The earlier `setActiveMenu` carried a conditional `navigateToPane('content')` gated on `item !== prev && mobileView === 'thread'` — the gate silently dropped the swipe when the user re-tapped the current item or wasn't on the chat pane. That conditional is gone; callers are explicit.
- **Don't reach for `navigateToPane('content')` directly under an `isMobile()` gate.** That pattern only covers the mobile half — it leaves desktop users with a collapsed split with no visible feedback. `revealContentPane` handles both. The one exception is `focusThread`, which navigates to the *thread* pane (different pane, different helper).

When you add a new navigation entry point, add it to the test mirror in `crates/lucidos-app/src/store/actions/menu.test.ts` (or the equivalent suite) so the regression is pinned.

## Modals & Popovers: Click-Outside Dismiss

**Every modal, popover, dropdown, anchored panel, command menu — anything overlaid on top of other UI — MUST dismiss on outside click AND swallow that click.** A click outside the modal closes it and does NOT also trigger the underlying button, chat row, link, or any other handler. Two contracts in one — both are required.

Always use `useDismissOnOutside` from `hooks/useAnchoredPopover.ts`. Do not hand-roll a `document.addEventListener('mousedown', ...)` close handler inside a component — those forget to swallow the paired `click` and fall out of compliance the moment someone clicks on a sibling action button.

```tsx
const panelRef = useRef<HTMLDivElement>(null);
const [anchor, setAnchor] = useState<HTMLElement | null>(null);
const open = useSignal(false);

useDismissOnOutside(open.value, panelRef, anchor, () => (open.value = false));

return (
  <>
    <button ref={setAnchor} onClick={() => (open.value = !open.value)}>…</button>
    {open.value && <div ref={panelRef} role="dialog">…</div>}
  </>
);
```

How the contract works (see `makeDismissHandlers` in the same file for the full pure-function version):

1. `pointerdown` outside the panel + anchor → `onDismiss()` and arm "swallow next click".
2. The next `click` is captured at `document` in the capture phase and `stopPropagation`+`preventDefault`'d — nothing downstream fires.
3. A `click` that arrives **without** a preceding outside pointerdown (synthetic `HTMLElement.click()` from a keyboard shortcut, an e2e test driver, or any other programmatic source) falls into a click-capture fallback: same `isOutsidePointerTarget` check, same `onDismiss()` + swallow. Without this branch the contract silently breaks for any caller that drives dismiss via synthetic clicks (the thread-filter dropdown e2e tests are the canary).
4. The anchor is exempted (re-clicking it must toggle via the caller's `onClick`).
5. `Escape` always dismisses (no click-swallow side effect).

`onDismiss` may return `false` to declare the call was a no-op — e.g. the popover is already on its way out via a close animation and the originating signal is still `true`. Both the pointerdown path and the click-capture fallback honour the return value: a `false` return leaves the suppressor disarmed (and skips the inline swallow in the fallback), so the user's tap on a sibling button still reaches its handler. Returning `void` / `true` keeps the default dismiss+swallow. `closeDrawer` is the canonical user of this: during the 200ms slide-out it returns `false` so the hook stops eating neighbor taps mid-animation.

Why both halves: a CSS-only "click closes me" via backdrop element solves dismiss but leaves the underlying click free to fire a different button. A bare `pointerdown` listener that calls `onDismiss` but doesn't swallow the click does the same. The user expects "I clicked away from this thing" to be a single atomic action — they did NOT mean to also fire whatever was under their cursor. Get this wrong and a user dismissing a settings popover can accidentally send a chat message, open a different app, or trigger a destructive action.

Tests live in `hooks/useAnchoredPopover.test.ts` — drive `makeDismissHandlers` directly without jsdom.

## CSS & Component Rules

- **Tab title**: `(count) Lucidos` (count first for narrow tabs)
- **No system dialogs**: Use `showToast(msg, type)` / `await showConfirm(msg, okLabel)`
- **No native tooltips**: Use `data-tooltip="text"`. Desktop-only.
- **List rows**: `.list-row` / `.list-row-info` / `.list-row-actions` from `global.css`
- **Action buttons**: `.action-btn` from `global.css`. Variants are additive: `class="action-btn action-btn-confirm"`.
  - `.action-btn` — default (blue). Neutral: Edit, Open, Restart, Prev/Next, Retry.
  - `.action-btn-confirm` — green. Positive: Apply, Accept, Confirm.
  - `.action-btn-danger` — red. Destructive: Delete, Discard, Remove, Cancel.
- **Auto-expanding textareas**: `AutoTextarea` from `components/shared/AutoTextarea.tsx`. Enter submits, Shift+Enter newline.
- **All sizes `rem`**: Divide px by 16 (4px→0.25rem, 8px→0.5rem, 16px→1rem). Exceptions: `1px` borders, `0px` env(), `@media`, `box-shadow`.
- **No `<select>`**: Use `Dropdown` from `components/shared/Dropdown.tsx`
- **No `id` on dual-rendered components**: Both `SplitLayout` (desktop) and `MobileSwipeContainer` (mobile) render simultaneously. Use `data-role="name"` + `querySelectorAll`. Cross-component: `getVisiblePromptInput()` in `promptFocus.ts`. **Debug hint:** if something works on desktop but fails on mobile (or vice versa), check whether you're hitting the wrong layout's copy — inspect `getBoundingClientRect()` for 0x0 dimensions.
- **Heavy-mount children must skip the inactive layout**: Iframes, video/audio players, big WebGL canvases, anything that fetches on mount — both layout copies will trigger the work, doubling network and resource cost (e.g., before the fix, opening an app fetched `/api/v1/sdk-prefs.js` twice). Pattern: `ContentPane` takes `layout: 'desktop' | 'mobile'` and forwards it to the heavy child; child gates the render with `layout === (viewportIsMobile.value ? 'mobile' : 'desktop')`. `viewportIsMobile` is the reactive signal in `utils/viewport.ts`. Example: `AppUiInline.tsx`.
- **No `getElementById()`**: Banned except `#app`. Use `querySelector`/`querySelectorAll`
