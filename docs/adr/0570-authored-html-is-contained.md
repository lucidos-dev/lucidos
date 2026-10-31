# 0570: Authored HTML is clipped to its own box and carries no form controls or popovers, amending 0114

- **Status**: Accepted
- **Date**: 2026-10-24

## Context

The sanitizer from ADR 0114 kept `style`, `form`, `input`, `button` and the
`popover` attributes. A security review checked what that let rendered
content do on the host origin, in Chromium and WebKit, with synthetic input.

- A form with inline `position: fixed` covered the whole viewport, header and
  Allow button included. Its submit posted a typed key cross-origin and
  navigated the app away.
- The same form with no style at all, and only a host class such as
  `modal-overlay` or `pairing-gate`, did the same.
- A `<button popovertarget>` opened a popover over everything.

The content comes from the model (reachable by prompt injection), from repo
files, from plugin manifests and from release notes.

## Decision

*Authored HTML* gets three rules, applied by one sanitizer policy for every
caller:

1. Every host element that receives sanitizer output carries `.authored-html`,
   whose rule is a `clip-path` inset (host-components.css only). The inset
   reaches past the box by ring room, and on the left by room for the
   numbers of an ordered list.
2. `form`, `button`, `select`, `textarea` and authored `input` are escaped to
   visible text, the 0114 rule for `iframe` and `script`. The renderer's
   nonce-marked task checkbox is the only `input` kept.
3. `popover`, `popovertarget`, `popovertargetaction`, `command` and
   `commandfor` are forbidden attributes.

## Rationale

Each rule closes a hole the others leave open.

- **The clip bounds layout.** It confines the paint and hit-testing of every
  descendant, fixed ones included, and makes a stacking context. That holds
  for an inline style and for a borrowed host class alike, in Chromium and
  WebKit.
- **Containment does not stop a submit.** A form in normal flow, inside its own
  message, still posts a typed secret anywhere. So form controls go.
- **Containment does not clip the top layer.** A popover opens above every
  stacking context. So the popover attributes go.

The renderer's own Copy buttons now join the tree after the sanitizer, on
wrappers carrying the nonce. A forged copy block therefore gets no button.

## Consequences

- Inline `style` and `class` stay everywhere, so slides, rendered diffs and
  markdown files keep their look.
- Authored content may paint up to the room past its box: ring room on each
  side, and `--space-md` on the left. That is the narrowest host gutter, a
  reply's, so the room never passes a pane edge, the header, the composer or
  an option.
- The clip's stacking context paints over an earlier positioned sibling. So
  the rendered diff's tint layer takes a z-index.
- A span site sets a display, so the clip applies to one box.
- A slides list takes the class on its `<ul>`, because an outside list marker
  paints beyond the `<li>`.
- `authored-html-guard.test.ts` fails on a new injection site without the
  class. `authored-html-stays-contained-all.spec.ts` probes the header, the
  composer and the options in every browser project.
- Still open, by choice: a CSP on the host shell, and lookalike content inside
  a message's own box.

## Alternatives considered

- **Strip `style` from every caller.** It unstyles every slide deck and diff,
  and leaves the host-class route fully open.
- **Strip `style` per caller, keeping it for slides and diffs.** It needs a
  per-caller sanitizer config and still misses the class route. It would only
  shrink spoofing inside a message's own box.
- **Containment alone.** The browser proof shows a contained form still posts,
  and a popover still covers the app.
- **`contain: paint`.** The browser proof used it, and it blocks the same
  covers. But it clips at the padding box, which cut ordered-list numbers in
  replies. It also stopped child margins collapsing, which moved slide
  spacing. Room by padding plus a negative margin fights every site that sets
  its own padding.
- **Put the rule on `.markdown-content`.** That class ships to app frames in
  `shared-components.css`, where an app's own layout is its business.
