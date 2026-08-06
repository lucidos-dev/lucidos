/**
 * Publishes the width the transcript's scrollbar takes out of its content box as
 * `--scrollbar-gutter-width` (declared with a 0px default in
 * styles/global/base.css).
 *
 * WHY a measured value instead of a constant: the transcript scrolls and the
 * composer below it does not, so on a platform with classic (layout-consuming)
 * scrollbars the transcript's centered turns sit half a scrollbar to the LEFT of
 * anything centered in a non-scrolling sibling, and its right content edge sits a
 * whole scrollbar further in. Nothing in CSS exposes that width to the sibling,
 * so the alignment has to be measured once and handed to CSS. Every surface that
 * must line up with the transcript (`.prompt-area`, the compose-empty welcome)
 * adds this to its own right inset, and `.thread-content` reserves the matching
 * space unconditionally (`overflow-y: scroll` + `scrollbar-gutter: stable`).
 *
 * It is 0 wherever scrollbars are overlay (iOS, and macOS unless the system is
 * set to always show scroll bars), where the transcript reserves nothing either.
 *
 * WHY the LIVE transcript is measured, with the probe only as a fallback: a
 * detached clone is a MODEL of the transcript, and on real iOS the model is
 * wrong. There the probe reports our `::-webkit-scrollbar` width (0.5rem = 9px
 * at the 18px mobile root) while the transcript itself reserves nothing, so the
 * composer subtracted a gutter that was not there and its right edge sat 9px
 * inside the question cards it docks under. No emulator reproduces the split
 * (Playwright's WebKit answers 9 for both), so the fix is not to build a better
 * model. It is to ask the element itself. The probe survives only for the boot
 * publish, before any transcript is mounted.
 */

const CSS_VAR = '--scrollbar-gutter-width';

/** The transcript's scroll container. Both `ThreadView` branches render it, and
 *  so does the compose-empty welcome, which is excluded below since it is not a
 *  scroll container and reserves nothing. */
const TRANSCRIPT_SELECTOR = '.thread-content';

/** The transcript's own overflow declarations, verbatim (`.thread-content` in
 *  styles/chat/input-messages.css). The fallback probe is a clone of the thing
 *  it stands in for, so the two cannot answer differently on the engines where
 *  cloning IS enough. Keep in sync with that rule. */
const PROBE_OVERFLOW = 'overflow-x:hidden;overflow-y:scroll;scrollbar-gutter:stable;';

const PROBE_BASE =
  'position:absolute;top:-9999px;left:-9999px;width:100px;height:100px;visibility:hidden;';

/**
 * What a laid-out scroll container takes out of its own content box, in px:
 * `offsetWidth - clientWidth` is the scrollbar gutter plus the horizontal
 * borders, so the borders come back off (`.thread-content` has none today, but
 * a border added later would otherwise read as gutter and push the composer in
 * by that much).
 */
function reservedWidth(el: HTMLElement, style: CSSStyleDeclaration): number {
  const borders = parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth);
  const reserved = el.offsetWidth - el.clientWidth - (Number.isFinite(borders) ? borders : 0);
  return Number.isFinite(reserved) && reserved > 0 ? reserved : 0;
}

/**
 * Measure the LIVE transcript, or null when there is none to measure (boot
 * before the first render, the compose view, a layout-less test environment).
 * Null is "no answer", distinct from a measured 0.
 */
function measureLiveTranscript(doc: Document): number | null {
  const view = doc.defaultView;
  if (!view || typeof view.getComputedStyle !== 'function') return null;
  if (typeof doc.querySelectorAll !== 'function') return null;
  const els = doc.querySelectorAll<HTMLElement>(TRANSCRIPT_SELECTOR);
  for (let i = 0; i < els.length; i++) {
    const el = els[i];
    // A 0-width copy is an inactive layout's, not the one on screen. Not
    // `isElementVisible` (components/chat/scrollState.ts): this module is
    // imported by main.tsx before anything mounts, so it must not depend on the
    // component/store layer, and it takes `doc` injected so the unit tests can
    // run without a layout engine. A transcript clipped by an ancestor also
    // still reserves its gutter, which is all this asks.
    if (!(el.offsetWidth > 0)) continue;
    const style = view.getComputedStyle(el);
    // The compose-empty welcome reuses `.thread-content` with `overflow:
    // visible` (styles/panels/shell.css) and reserves nothing. Letting it answer
    // would drop the compensation while the composer is still the same element
    // that is about to dock under a real transcript, so the composer would slide
    // sideways on the way into a thread. Only a scroll container answers.
    if (style.overflowY !== 'scroll' && style.overflowY !== 'auto') continue;
    return reservedWidth(el, style);
  }
  return null;
}

/**
 * Fallback for the boot publish: lay out an off-screen clone of the transcript's
 * scroll container and report what its scrollbar took out of the content box.
 *
 * The answer is measured rather than read off the `::-webkit-scrollbar` width in
 * components.css because that rule is authored in `rem` (so the answer moves with
 * the UI scale) and because a platform may ignore it entirely. It is measured
 * with the transcript's exact declarations rather than a "typical" scroll
 * container because engines disagree about which of them reserves anything:
 * Chromium honours `scrollbar-gutter: stable` on a container that does not
 * overflow while WebKit only reserves once a scrollbar is actually drawn, and
 * Chromium's `--hide-scrollbars` (headless, and therefore the e2e suite) zeroes
 * the scrollbar but not the stable gutter. Cloning the declarations covers every
 * one of those. What it does NOT cover is a platform that treats the clone and
 * the real scroller differently at all (iOS does), which is why this runs only
 * until a transcript exists to ask directly.
 */
function measureProbe(doc: Document): number {
  // Capability check rather than a try/catch: the unit-test environment runs on
  // a hand-rolled `document` stub with no layout engine (src/test-setup.ts), and
  // "there is nothing here to measure" is a legitimate zero, not an error being
  // swallowed. A real document always clears this.
  const host = doc.body ?? doc.documentElement;
  const el = doc.createElement('div');
  if (!host || typeof host.appendChild !== 'function' || !el || !el.style) return 0;
  el.style.cssText = PROBE_BASE + PROBE_OVERFLOW;
  host.appendChild(el);
  // A layout-less environment reports undefined for both, so this is NaN there.
  const reserved = el.offsetWidth - el.clientWidth;
  el.remove();
  return Number.isFinite(reserved) && reserved > 0 ? reserved : 0;
}

/**
 * The width the composer has to subtract from its right inset: what the live
 * transcript actually reserved, or the probe's estimate while there is no
 * transcript on screen.
 */
export function measureScrollbarGutter(doc: Document = document): number {
  return measureLiveTranscript(doc) ?? measureProbe(doc);
}

/**
 * Measure and publish. Called at boot (probe), whenever the UI scale changes
 * (`applyUiScale`, since a rem-sized scrollbar changes width with the root font
 * size), and when the transcript mounts (`ThreadView`), which is the first
 * moment the real answer is available.
 */
export function publishScrollbarGutter(doc: Document = document): number {
  const gutter = measureScrollbarGutter(doc);
  doc.documentElement.style.setProperty(CSS_VAR, `${gutter}px`);
  return gutter;
}
