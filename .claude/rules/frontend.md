---
globs:
  - "crates/lucidos-app/src/**/*.ts"
  - "crates/lucidos-app/src/**/*.tsx"
  - "crates/lucidos-app/src/**/*.css"
---

# Frontend Conventions (TypeScript & CSS)

## Frontend Sends Intent, Backend Owns Logic

The frontend expresses **what** the user wants, not **how** the backend should do it. Don't make the frontend extract data from its local state just to pass it back.

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
- **No `getElementById()`**: Banned except `#app`. Use `querySelector`/`querySelectorAll`
