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
 */

const CSS_VAR = '--scrollbar-gutter-width';

/** The transcript's own overflow declarations, verbatim (`.thread-content` in
 *  styles/chat/input-messages.css). The probe is a clone of the thing it is
 *  measuring, so the two cannot answer differently, whichever of the pair a
 *  given engine actually honours. Keep in sync with that rule. */
const PROBE_OVERFLOW = 'overflow-x:hidden;overflow-y:scroll;scrollbar-gutter:stable;';

const PROBE_BASE =
  'position:absolute;top:-9999px;left:-9999px;width:100px;height:100px;visibility:hidden;';

/**
 * Measure the gutter, in px, by laying out an off-screen clone of the transcript's
 * scroll container and reporting what its scrollbar took out of the content box.
 * 0 where scrollbars are overlay.
 *
 * The answer is measured rather than read off the `::-webkit-scrollbar` width in
 * components.css because that rule is authored in `rem` (so the answer moves with
 * the UI scale) and because a platform may ignore it entirely. It is measured
 * with the transcript's exact declarations rather than a "typical" scroll
 * container because engines disagree about which of them reserves anything:
 * Chromium honours `scrollbar-gutter: stable` on a container that does not
 * overflow while WebKit only reserves once a scrollbar is actually drawn, and
 * Chromium's `--hide-scrollbars` (headless, and therefore the e2e suite) zeroes
 * the scrollbar but not the stable gutter. Cloning the declarations sidesteps
 * every one of those: whatever the engine does to the transcript, it does to the
 * probe.
 */
export function measureScrollbarGutter(doc: Document = document): number {
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
 * Measure and publish. Called once at boot and again whenever the UI scale
 * changes (`applyUiScale`), since a rem-sized scrollbar changes width with the
 * root font size.
 */
export function publishScrollbarGutter(doc: Document = document): number {
  const gutter = measureScrollbarGutter(doc);
  doc.documentElement.style.setProperty(CSS_VAR, `${gutter}px`);
  return gutter;
}
