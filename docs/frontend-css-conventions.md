# Frontend CSS conventions: the reasons

`.claude/rules/frontend-css.md` lists the CSS and component conventions as short bullets. This page keeps the reasoning and the scars behind the long ones. Each section names the rule bullet it explains. ADRs hold the decisions, and this page holds what has no ADR.

## Every duration is scaled

The `--duration-*` tokens carry the animation speed slider into CSS only through `var(--duration-scale)`. A new token declared as a bare literal is the one animation that ignores the setting.

A TS timer that outlives a CSS transition holds an animation class on, or keeps content mounted through its own fade. Slack is a fixed safety margin and is not animation, so it is added outside `scaledDurationMs`. An unscaled timer fires partway into the transition it exists to outlive. At 0.1x the drawer body blanks mid-slide, and a maximizing pane snaps the rest of the way.

An indefinite animation is an activity indicator and not a transition. It keeps a literal duration.

## An icon that belongs to a label

A `<button>` or an `<svg>` is an atomic inline, and line breaking allows a break in front of one. A label squeezed by the control beside it drops its trailing icon onto a line of its own. Under the text and next to nothing, it reads as a stray glyph.

`white-space: nowrap` is not the fix. The break sits in the parent's text, so only the parent's own value governs it. Nowrapping the whole label is worse, because a long label must still wrap.

The fix is a plain inline wrapper whose `::before` is a U+2060 WORD JOINER. The joiner forbids a break on both sides. It also raises the label's min-content width to include the icon, so a flex row cannot shrink it off the line either.

- Use generated content, never a text node. A text node lands in `textContent`, in the accessible name of a wrapping `<label>`, and in a copied selection. An exact-text assertion then fails on an invisible character.
- The wrapper must be `display: inline`. An `inline-flex` or `inline-block` wrapper is an atomic inline itself and hides the joiner.
- Give it `line-height: 0` and it measures exactly what the bare icon did.

`.explainer-slot` is the worked example, so every *explainer* has it already.

## Dialog messages

`showConfirm` and `showPrompt` take `message: string`, and so do their SDK twins `lucidos.ui.confirm` and `lucidos.ui.prompt`. Multi-paragraph copy can only live inside that one string.

`DialogMessage` emits one `.confirm-message` paragraph per blank-line-separated block. A single `\n` collapses to a space, so source-wrapped concatenated copy stays one paragraph. `white-space: pre-line` would turn every incidental newline in an app-supplied message into a break. The paragraph rule is part of the app-facing SDK contract, so a change to it updates `system-knowhow/js-sdk.md`.

## The left-accent stripe

A coloured vertical `border-left` is an overused template look, and the project rejects it. The regression this bans: `.system-notice` once carried `border-left: … var(--accent)` on "New engine version available".

## Prose in a list row

The base `.list-row-details` is `display: flex`, and its `0.75rem` gap is the separator between metadata fields. It blockifies every inline child. Put a sentence in it and each `<strong>` or `<code>` becomes its own flex item.

- The gap opens holes mid-sentence.
- The row gap double-spaces the wrapped lines.
- Punctuation right after an element is stranded at the start of the next line. Mobile Access once rendered a lone "." under its tailnet address.

The prose modifier restores block flow. The same trap applies to an explicit `·` between fields, which the gap double-spaces. It also applies to `.list-row-details` on a `<span>` inside a line of text, where the flex box breaks the line.

## Prose fields and autocorrect

`installNoAutofill` stamps `autocomplete`, `autocorrect` and `autocapitalize` = `off` on every input, current and future. It kills WebKit's saved-value dropdown and its white-to-dark flash. That suits the roughly 100 config fields (paths, ids, model ids, API keys). It is wrong for a prose field.

A prose field spreads `PROSE_TEXT_ATTRS`, a marker (`data-prose`) and not a value. The stamp then leaves `autocapitalize` at the browser default. `autocorrect` follows the device's Autocorrect switch. Unset, it is on, on every client. The store pushes the value into the stamp through `setProseAutocorrect`, which re-stamps every mounted field. `autocomplete="off"` is still stamped.

A device that keeps hitting the UIKit bug turns autocorrect off. While UIKit holds a correction, it keeps the tap on a button below the text. That killed the composer's Send for a month (ADR 0262).

### Why the marker, and never `autocorrect="off"` in JSX

Spreading `autocorrect="on"` also works, but it routes through Preact's property path, and that path inverts the `off` direction. `autocorrect` is not in Preact's exclusion list, so `autocorrect={x}` becomes `el.autocorrect = x`. The IDL attribute is a boolean. The non-empty string `"off"` coerces to `true` and reflects back as `autocorrect="on"`. The attribute is then present, so `setIfAbsent` skips the field forever. That shipped as a real bug in `AllowlistEditor`, from 9 June to 29 July.

A `data-*` attribute is never an IDL property, so the marker always lands through `setAttribute`. A source-scan over every `.tsx` in `noAutofill.test.ts` enforces it.

### Mobile keyboards

`autocapitalize` is mobile-only and iOS reads it at focus time. Desktop browsers ignore it. Verify on a device, never in a desktop browser or the responsive simulator.

iOS computes the keyboard's shift state from editing deltas. A programmatic `el.value = ''` bypasses the editing pipeline. The keyboard can then keep a stale lowercase state until a real delta, such as a backspace, forces recomputation. That is a WebKit bug ([ionic#23217](https://github.com/ionic-team/ionic-framework/issues/23217), [ionic#22744](https://github.com/ionic-team/ionic-framework/issues/22744)).

If a surface clears a focused field programmatically and the keyboard misbehaves, clear it through a real editing operation. Do not blur and refocus: iOS refuses to reopen the keyboard outside a user gesture.

The regression behind all this: the 2026-06-25 autofill fix bundled `autocorrect` and `autocapitalize` into a suppression that only needed `autocomplete`. The chat prompt lost autocorrect. `spellcheck` stayed on, so typos stayed underlined and were never fixed.

## Border widths stay px

A border width is snapped to a whole unit before layout. Chromium snaps to a CSS px and WebKit to a device px. A rem value that lands on a fraction at a scaled root is silently rounded. That is harmless alone. It breaks any `calc()` that subtracts the border, since the calc sees the specified value and layout uses the snapped one.

So a border above `1px` keeps a px literal and a comment saying what depends on it not rounding. `--prompt-box-border-width` in `base.css` is the worked example: the composer's inner padding derives from it.

## `1rem` is a heading

`--font-size-xl` is exactly `1rem` and is labelled "section headings". Body text is `--font-size-md` at `0.8125rem` (13px). On the rest of the web `1rem` is body, so the scale's zero point sits a step and a half off the platform's. Every guess calibrated elsewhere lands high.

That one fact explains three failure shapes:

- A surface that names no size.
- A mockup that rebuilds "a sensible scale" around a 16px body and comes out one rung high on every step.
- An eyeballed literal that lands above the step it wanted.

The root font-size is `var(--user-ui-scale)`. It is deliberately not a scale step. It is the multiplier every `rem` in the app rides, geometry included, so it cannot be re-anchored.

### The default sizes in `base.css`

Two defaults are separate: `body { font-size: var(--font-size-md) }`, and `input, textarea, select, button { font-family: inherit; font-size: inherit }`. The second exists because a control inherits nothing from `body`: the UA stylesheet applies the `font` shorthand to it. The scattered per-component `font: inherit` declarations each stood in for it.

- The `font` shorthand also resets `font-weight` and `font-feature-settings`. `steps.css` and `skills.css` each carry the scar.
- `font-size: inherit` on `html` would override the ui-scale.

`text-defaults-guard.test.ts` checks the source. `e2e/type-scale-all.spec.ts` checks the rendered result, the only check that resolves the cascade. The history of all three deletions is in `docs/code-review-priors.md`.

### Pinned sizes and design skills

A value pinned to a computed-px threshold needs a comment. Example: the mobile prompt textarea's `0.9rem` stays at or above 16px on the 18px mobile root, so focusing it does not trigger iOS Safari's input zoom. The nearest token would fall below 16px.

A generic frontend design skill tells you to make the type treatment expressive and memorable. It is calibrated to a 16px body and knows nothing about this app.

## Content-pane spacing

The spacing scale is mirrored into the engine's `api/sdk_iframe.css`, since app iframes never load `base.css`. A shared class like `.list-row` renders in both, and shipped apps key off these exact values. Hence the two blocks stay identical and no step is redefined.

Every content-pane view routed into `.content-pane-body` shares the `--space-lg` horizontal gutter. Switching views then keeps one clean left edge, with the top-left toggle directly above the row content.

The carve-outs:

- Full-bleed views are edge to edge by design: the app-ui iframe, file, url and repo previews, and the diff content. The gutter is not moved onto `.content-pane-body` either, or it would inset list-row hover backgrounds.
- The Files tree is an indented tree, coherent at its own `0.5rem`.
- Card-like elements are coloured `bg-tertiary` boxes such as `.thread-queue-policy`. Their padding is internal and not a gutter.

There is no source-scan test, since classifying "structural" against "inline" padding is too brittle.

## The scroll gutter of a column view

`.content-pane-body` is one scroller for every view. Without a reserved gutter, a view that grows past one screen gains a classic scrollbar, and the whole column slides sideways. It uses the same `overflow-y: scroll` plus `scrollbar-gutter: stable` pair as `.thread-content`, for the same engine split (`panels/shell.css`).

## One content edge in the thread pane

The composer must line up with the message text, question cards and user bubbles it docks under. Four parts hold that edge.

1. `--thread-pane-gutter` is shared on both sides of both surfaces. An asymmetric value throws the centered column off the transcript's centre line above the width cap.
2. `--turn-body-inset` reproduces the inset a turn puts between its box and its visible content. All three surfaces then share one edge at any pane width.
3. On a classic-scrollbar platform the scrollbar takes its width out of the transcript's content box alone. Nothing in CSS hands that to a sibling. So every non-scrolling sibling adds `--scrollbar-gutter-width` to its right inset.
4. The two nested surfaces share one text column, because the eye reads the text and not the box.

### The gutter measurement

Chromium honours `scrollbar-gutter` on a container that does not overflow. WebKit reserves only once the scrollbar is drawn. So `.thread-content` uses `overflow-y: scroll` and `scrollbar-gutter: stable` together.

`utils/scrollbarGutter.ts` measures `offsetWidth - clientWidth` of the live element, borders discounted. It runs at boot, on every UI-scale change, and when the transcript mounts. It measures the real element because the detached clone is wrong on real iOS. The clone reports our `::-webkit-scrollbar` width (`0.5rem`, 9px at the 18px mobile root), while the transcript reserves nothing. The composer then subtracted a gutter that was not there, and its right edge sat 9px inside the question cards above it.

No emulator reproduces that split. Playwright's WebKit answers 9 for both. So the guard is a unit test over the measurement's element preference and not a browser e2e. The probe survives only as the boot fallback, before any transcript exists.

The compose-empty welcome is deliberately not measured. It reuses `.thread-content` with `overflow: visible`, and its answer of 0 would slide the composer sideways on the way into a thread.

### Nested surfaces

A question or permission card inside a turn (`.question-body`) and the composer box (`.prompt-box`) match more than their outer edges. Both put content on `--turn-surface-inset` by subtracting their own border. Two traps:

- The composer's inner padding lives on `.prompt-row .prompt-textarea` (`chat/input-messages.css`). It is not on the `.prompt-box` shell, which the trigger form's Intent field shares.
- `--prompt-box-border-width` is a literal `2px`. A rem value would round by an amount the `calc()` cannot see, and put the text off the column at a scaled root.

`e2e/prompt-transcript-alignment-all.spec.ts` checks that the outer edges match and that the published var equals what the transcript reserved. A spliced-in `.question-body` probe's content inset must also equal the composer's.

## End space

One default-deny spacer in `panels/content.css` gives every content-pane view its end space. The one exception is the phone's flowing file preview, which re-adds the spacer in `mobile.css`.

## A fade over a scroll container

The scrollbar is chrome and stays at full opacity whatever dissolves underneath it. There are two shapes, each with its own trap.

A mask on the container applies to the element's whole rendering, scrollbar included, so a full-width mask fades the thumb. The fix is a second, fully opaque layer sized `var(--scrollbar-gutter-width) 100%` and pinned `right top`, with the fade layer narrowed to `calc(100% - var(--scrollbar-gutter-width))`. The mask origin is the border box. The gutter sits between the border edge and the padding box, so the strip lands exactly on the scrollbar.

An overlay painted over the container by a sibling insets its right edge by the same var. It must be a gradient. A `box-shadow` feathers past the box it is cast from on every side, so no inset gives it a clean edge.

Both are no-ops wherever scrollbars are overlay, because the var is `0px`. That is why neither regression reproduces on a default-setting Mac or on a phone. It is also why the guard is a source scan. The transcript's two fades are the worked examples: the desktop top mask on `.thread-content`, and the bottom dissolve into the composer on `.prompt-area::before`.

## Dual-rendered components

`App.tsx` mounts only the visible layout's pane tree: `SplitLayout` on desktop, `MobileSwipeContainer` on mobile. Dual-mounting the panes was removed because every signal write fanned out to both subtrees. Per-layout copies still exist in the header chrome, and the mounted layout swaps at runtime when the viewport crosses the breakpoint. So `id` attributes stay unsafe.

If something works on desktop but fails on mobile, or the reverse, suspect the wrong layout's copy. Inspect `getBoundingClientRect()` for 0x0 dimensions.

### Heavy-mount children

A component with a copy in each layout triggers its work twice, which doubles network and resource cost. Before the pane single-mount fix, opening an app fetched `/api/v1/sdk-prefs.js` twice. Pane children mount once now, and the rule still binds for chrome with per-layout copies. `AppUiInline.tsx` keeps the gate from the dual-mount era. It is harmless now that `ContentPane` mounts once.

## Focus rings

`--focus-ring` is an outward `box-shadow`, and a shadow adds nothing to scrollable overflow. Any ancestor that scrolls or clips cuts the ring where a control sits flush with its edge. That is how the Rename dialog's input lost the bottom edge of its ring.

- A header row clips through `clip-path: inset(...)` and never `overflow: clip`. The packaged macOS webview makes a `clip` box a scroll container.
- A clip with no job goes. `.list-row-info` lost its `overflow: hidden`, since `min-width: 0` already lets it shrink.
- An inset is the last resort, for a control with no room outside it, such as a chevron pinned to a clip edge.
