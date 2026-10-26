---
paths:
  - "crates/lucidos-app/src/**/*.css"
  - "crates/lucidos-app/src/**/*.tsx"
  - "crates/lucidos-engine/src/api/sdk_iframe.css"
  - "crates/lucidos-engine/src/api/sdk_iframe_shapes.css"
  - "crates/lucidos-engine/src/api/sdk_iframe_control_theme_parts.css"
---

# Frontend CSS & Component Conventions

This half of the frontend rules carries the design tokens, the three-file component split and the class contract. `frontend.md` carries the TypeScript and state contract. Reasons and scars: [`docs/frontend-css-conventions.md`](../../docs/frontend-css-conventions.md).

Scoped to `.css` **and** `.tsx` on purpose: a component applies these classes from TSX. A plain `.ts` edit gets only `frontend.md`.

## CSS & Component Rules

### Design tokens: never hardcode a value a token already names

The token families live in `styles/global/` (`:root` in `global.css`, the type scale in `base.css`). Read the file for current values. Never copy values into a rule.

| Family | Tokens | Rule |
|---|---|---|
| Color | `--bg-{primary,secondary,tertiary}`, `--border-color`, `--text-{primary,secondary,muted,on-accent,strong}`, `--accent`, `--accent-{green,yellow,red}` | Never hardcode a color. Two `#fff` / `rgba(255,255,255,*)` exceptions: a translucent overlay over image content, and a surface whose colour is functional (the pairing QR's white field). The second takes a comment saying why. |
| Type scale | `--font-size-{3xs,2xs,xs,sm,md,lg,xl,2xl,3xl,display}` | `font-size` takes a token, never a raw `rem`. `em`, percentages, `inherit` and `var(--user-ui-scale)` stay literal, as does a value pinned to a computed-px threshold, with a comment saying why. |
| Z-index | `--z-{float,dropdown,sticky,drawer,control-panel,popover,app-fullscreen,modal,toast,tooltip}` | Raw 1 to 10 is fine inside a component's own stacking context. Anything higher takes a token. |
| Duration | `--duration-{fast,normal,slow,emphasis}` | `emphasis` is for a cue the user must register. Try `normal` or `slow` first. |
| Icon size | `--icon-size-{sm,md,lg}` | No inline `width`/`height` on an SVG inside `.icon-btn`. |
| Shadow | `--shadow-{sm,md,lg,up}` | Never hardcode a `box-shadow`. |
| Surface | `--radius-{control,surface,round}`, `--surface-bg`, `--scrim`, `--surface-inset` | Every corner reads a radius token (`round` for circles and pills), scaled with `calc()` if needed. Spinners and badges are exempt (`radius-token-guard.test.ts`). Every toast, popover and dialog is a `.surface` with a `SurfaceHead`, and a menu a `.surface-box` (ADR 0290). Only a blocking surface paints `--scrim`. Text in a head, body or foot sits on `--surface-inset`. A surface opens at its trigger, as a *header palette*, or as a blocking dialog (ADR 0299, `surface-system-guard.test.ts`). |
| Syntax | `--syntax-{key,string,number,keyword,comment,control}` | Highlighting only. |


### Every duration is scaled by the animation-speed slider

Each `--duration-*` token is its 1x literal times `var(--duration-scale)`. That product is the only thing carrying the slider into CSS.

- **A new duration token is `calc(<literal> * var(--duration-scale))`.** A bare literal is the one animation that ignores the setting (`duration-scale-guard.test.ts`).
- **A TS timer that outlives a CSS transition** passes its 1x base through `scaledDurationMs` and adds slack outside the call: `scaledDurationMs(PANE_TRANSITION_MS) + 100`. List it in `store/__tests__/duration-scale.test.ts`.
- **An indefinite animation (spinner, shimmer) does NOT scale.** It keeps a literal duration and no token.

### Component rules

- **One concept, one component, one wording.** Reuse what draws the nearest neighbour, never a one-off look. A transcript marker is an event card (`eventRowBody`), and every session start says "New session". A new look changes the shared component.
- **Every icon-only button needs an `aria-label`.**
- **Explain a control with `<Explainer>` ⓘ, not a hint line.**
- **An icon that belongs to a label never wraps away from it.** Put the icon in a plain `display: inline` wrapper whose `::before` is `content: '\2060'` (U+2060 WORD JOINER). Generated content, never a text node. `.explainer-slot` is the example.
- **An icon toggle renders through `ToggleIconButton`** (`aria-pressed`, a `tone`). A new look is a new tone, never a local rule (`toggle-icon-single-source.test.ts`).
- **Tab title**: `(count) Lucidos`.
- **No system dialogs**: use `showToast(msg, type)` and `await showConfirm(msg, okLabel, { variant })`. `variant` is required: `'danger'` only for an action that loses something for good, else `'default'`.
- **A dialog message is ONE string, and a blank line is a paragraph.** Render it through `<DialogMessage message={…} />` (`components/shared/DialogMessage.tsx`). Never hand-roll a `<p class="confirm-message">` or use `white-space: pre-line`. A change to the paragraph rule updates `system-knowhow/js-sdk.md` § Confirmation dialogs and § Prompts in the same commit.
- **No native tooltips**: use `data-tooltip="text"`. Host and apps share `installTooltips()` (`packages/lucidos-sdk/src/tooltip.ts`).
- **A tap paints no background.** Wrap every `:hover` rule in `@media (hover: hover)` or scope it under `body:not(.is-touch)`. No `:active` fill (a scale is fine). Never restate the tap-highlight reset in `base.css`. A cursor class set on `mouseenter` takes the same gate. Pinned by `touch-paints-no-press.test.ts`.
- **No left-accent stripe on a card, callout, banner or notice, strictly forbidden.** That covers a coloured `border-left`, and a faux bar via `::before` or an inset `box-shadow`. Use bold text, a raised `--bg-tertiary` surface, spacing or an inline icon.
- **Component CSS is split three ways by audience:**
  - **Reusable (host + apps)** goes in `styles/global/shared-components.css`: the `.action-btn` family, `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.pill-bar`, `.toggle-switch`, `.mini-spinner`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`, `h1`–`h6`, the dropdown classes. The engine `include_str!`s this file into `/api/v1/sdk-iframe.css` (`sdk.rs`), so a change ships to host and apps at once. **Never copy these rules into `sdk_iframe.css`.** A component apps also need moves here, never an app-only copy (`exported-components-single-source.test.ts`). `surface.css` and `text-input.css` are served the same way. `iframe-css-no-shared-component-restyle.test.ts` bans a colour, background, border, radius, padding, font-size or box-shadow on their classes in `sdk_iframe.css`.
  - **Host chrome only (never served to apps)** goes in `styles/global/host-components.css`: the custom `<Dropdown>` positioning, `.nav-history-*`, `.send-cancel-*`, `.icon-btn` header variants. `global.css` imports it after `shared-components.css`.
  - **Iframe only** goes in `sdk_iframe.css`, or `sdk_iframe_shapes.css` for widget shapes.
  - **Structural host chrome** (`#app`, `body`, the `:root` token blocks) stays in `base.css`. The `#tooltip` rules live in `shared-components.css` (`tooltip-single-source.test.ts`).
  - A new app-facing class in `shared-components.css` also goes in the component-class table in `system-knowhow/js-sdk.md`.
- **List rows**: `.list-row` / `.list-row-info` / `.list-row-actions`. `.list-row-details` is a row of FIELDS (`display: flex`). A sentence takes `class="list-row-details list-row-details-prose"` (`list-row-prose-guard.test.ts`).
- **Action buttons**: `.action-btn` plus an additive variant.
  - `.action-btn` (blue): neutral. Edit, Open, Restart, Prev/Next, Retry.
  - `.action-btn-confirm` (green): positive. Apply, Accept, Confirm.
  - `.action-btn-danger` (red): destructive. Delete, Discard, Remove, Stop.
  - `.action-btn-secondary` (outlined): the lower-emphasis choice beside a primary. Cancel, Later, Not now.
  - A toast never draws a lone neutral action as a button: the whole card is the tap (`docs/glossary.md` § Toast tap).
- **A row of action buttons packs right, primary last.** Use `justify-content: flex-end`. `button-rows-pack-right.test.ts` fails on a left-packed row.
- **Auto-expanding textareas**: `AutoTextarea` (`components/shared/AutoTextarea.tsx`).
- **A prose field spreads `{...PROSE_TEXT_ATTRS}`.** `installNoAutofill` (`utils/noAutofill.ts`) stamps `autocomplete`/`autocorrect`/`autocapitalize` = `off` on every input. A prose field is one the user writes sentences in, such as the chat prompt or a thread title. The marker (`data-prose`) leaves `autocapitalize` at the browser default and lets `autocorrect` follow the device's switch (ADR 0262).
  - `AutoTextarea` is a prose field already. Use a bare `<textarea>` for a code or config editor. Add a new prose field to `PROSE_FIELDS` in `utils/noAutofill.test.ts`.
  - **Never write `autocorrect="off"` in JSX.** Preact turns it into a boolean `true`. A source-scan in `noAutofill.test.ts` enforces this.
  - **Verify `autocapitalize` on a real iOS device.** Never clear a focused field with `el.value = ''`, and never blur and refocus.
- **All sizes `rem`**: px / 16. Exempt: border widths (any width), `0px`, `env()`, `@media`, `box-shadow`. A border above `1px` keeps a px literal and a comment saying what depends on it not rounding (`--prompt-box-border-width` in `base.css`).
- **`1rem` IS A HEADING in this app.** `--font-size-xl` is `1rem` ("section headings"). Body text is `--font-size-md` at `0.8125rem`. An app or HTML artifact body is `--font-size-sm` (ADR 0319). When text reads too big, look for a MISSING declaration first. The root font-size is `var(--user-ui-scale)`, never a scale step.
- **A surface that names no `font-size` gets the body step from `base.css`. Do not remove its two defaults:** `body { font-size: var(--font-size-md) }` and `input, textarea, select, button { font-family: inherit; font-size: inherit }`.
  - **Never use the `font` shorthand** to give a control its font, and **never add `html` to the control rule.**

  Pinned by `text-defaults-guard.test.ts` and `e2e/type-scale-all.spec.ts`.
- **`font-size` takes a `--font-size-*` token, never a raw `rem`.** A new size gets a token in *both* `:root` sources (`base.css` and `api/sdk_iframe.css`).
- **A general-purpose design skill does not override this scale.** Take the type scale from `base.css`.
- **Content-pane structural padding uses a `--space-*` token and one 1rem gutter.**
  - **The scale** is `--space-{xs,sm,md,lg,xl}`, in `base.css` and mirrored in `api/sdk_iframe.css`. Keep the two blocks identical. Never redefine a step.
  - **Every content-pane view shares the `--space-lg` horizontal gutter** on its structural padding: the toolbar, list rows, empty states and headers. `/harden` flags a raw `padding: N rem` gutter.
  - **Carve-outs, each with a comment at the site:** full-bleed views (app-ui iframe, previews, diffs), the Files tree (`.file-item`), card-like elements (boxes, pills, chips, badges), inline controls, computed-px values, `1px` borders, `0` and `env()`.
- **A content-pane column view reserves the scroll gutter while it fits.** `.content-pane-body` uses `overflow-y: scroll` plus `scrollbar-gutter: stable`, as `.thread-content` does. A full-bleed view is excluded. Pinned by `content-pane-scroll-gutter.test.ts` and `e2e/settings-column-holds-still-desktop.spec.ts`.
- **Every content-pane view gets the *end space* below its last element, and no view sets its own** (`docs/glossary.md` § End space). One default-deny spacer in `panels/content.css` provides it. Never pin a column view to the pane's height. Never style a view root's `::after`. A new full-bleed view takes `content-view-full-bleed` and a row in `content-pane-end-space.test.ts`.
- **Thread-pane surfaces share ONE content edge, scroll gutter included.** The transcript (`.thread-content`) and the composer (`.prompt-area` → `.prompt-input-container`) resolve to the same left and right insets. Four parts:
  1. **The shared `--thread-pane-gutter`** on both sides of both surfaces.
  2. **A `--turn-body-inset` padding** on the composer column and on the compose-empty `.welcome-hero`.
  3. **`--scrollbar-gutter-width` added to the RIGHT inset of every NON-scrolling sibling.** `.thread-content` reserves it with `overflow-y: scroll` **and** `scrollbar-gutter: stable`.
  4. **The two NESTED surfaces share one text column.** A question or permission card (`.question-body`) and the composer box (`.prompt-box`) both put content on `--turn-surface-inset`, minus their OWN border. The composer's padding lives on `.prompt-row .prompt-textarea`. `--prompt-box-border-width` stays `2px`.

  A new docked surface joins parts 1 to 3, a nested one part 4 too. Guards: `e2e/prompt-transcript-alignment-all.spec.ts` and the tripwire in `utils/scrollbarGutter.test.ts`.
- **A fade over a scroll container must exclude the scroll gutter.** The scrollbar is chrome and stays fully opaque.
  - **A mask** gets a second opaque layer sized `var(--scrollbar-gutter-width) 100%` pinned `right top`. Narrow the fade layer to `calc(100% - var(--scrollbar-gutter-width))`.
  - **An overlay** insets its right edge by the same var, and is a gradient, never a `box-shadow`.
  - Guard: `transcript-fade-scroll-gutter-guard.test.ts`. Examples: `chat/input-messages.css`, `.prompt-area::before` in `panels/content.css`.
- **No `<select>`**: use `Dropdown` (`components/shared/Dropdown.tsx`).
- **Every search or filter box is a `SearchField`** (`components/shared/SearchField.tsx`). A host recolours it through `--search-field-{bg,fg,muted}`. Never restyle the input.
- **No `id` on dual-rendered components.** The header chrome has per-layout copies (`ControlPanel` in `AppHeader` and `MobileAppHeader`), and they swap at the breakpoint. Use `data-role="name"` + `querySelectorAll`, or `getVisiblePromptInput()` in `promptFocus.ts`.
- **A heavy-mount child with a copy in each layout skips the inactive one**: iframes, players, WebGL canvases, anything that fetches on mount. The parent forwards `layout: 'desktop' | 'mobile'`. The child renders only when `layout === (viewportIsMobile.value ? 'mobile' : 'desktop')` (`AppUiInline.tsx`).
- **No `getElementById()`** except `#app`. Use `querySelector`.

### Focus rings: a box that clips leaves ring room

`--focus-ring` is an outward `box-shadow`, which adds nothing to scrollable overflow. An ancestor that scrolls or clips cuts the ring where a control sits flush with its edge.

- **Fix the clipping box, never the control.** Give it *ring room* (`docs/glossary.md`): padding of `--focus-ring-width` on the flush edge, handed back with an equal negative margin or taken from a neighbour. `.pill-bar` and the surface body are the examples.
- **A clip with no job goes.**
- **A header row clips through `clip-path: inset(...)`**, never `overflow: clip`. The packaged macOS webview makes a `clip` box a scroll container.
- **Inset is the last resort**, for a control with no room outside it.
- **The ring stays a literal**, since a theme replaces it whole. `focus-ring-width.test.ts` holds every definition to `--focus-ring-width`.

`e2e/focus-ring-not-clipped.spec.ts` fails on a cut edge. Put a new surface in its visit list.

### Reduced motion keys on `data-motion`, never on the media query

The *Motion* setting and the OS switch resolve to one value, published as `data-motion` on `<html>` (`docs/glossary.md` § Reduced motion (resolved)).

- **Write calm rules as `:root[data-motion="reduce"] .x { … }`.** Script reads `isReducedMotion()` from `utils/motion.ts`. A `@media (prefers-reduced-motion)` block ignores a user who picked Reduce or Full in the app.
- **Reduced motion collapses `--duration-scale`**, so a tokenized one-shot needs no rule. An indefinite or literal-duration animation needs `animation: none` for its own selector.
- **An end-event listener needs a way to finish without its event**: a scaled timer, or a bypass that reads `isReducedMotion()`.

`reduced-motion-guard.test.ts` and `animation-end-fallbacks.test.ts` enforce all three.
