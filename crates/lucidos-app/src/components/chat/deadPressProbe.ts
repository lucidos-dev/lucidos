import { showToast } from '../../store/store';
import { isMobile } from '../../utils/viewport';

/** Reports a tap on the composer's Send that produced nothing.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1 and removed once
 *  the next report names the cause. It exists because the bug behind it has
 *  been reported three times and nobody can reproduce it: it strikes now and
 *  then on an iOS PWA, kills the button wherever the finger presses, and clears
 *  when the keyboard is dismissed. No emulator reproduces it, so the app has to
 *  be the one that says what happened.
 *
 *  Two questions, and the answers point at different halves of the stack. Did
 *  the press even reach the button? If not, paint and hit-testing disagree, and
 *  the viewport numbers say by how much. If it did, both activation paths
 *  declined, which is code we own.
 *
 *  The decisions are pure functions of what was measured, so they test without
 *  a DOM. `installDeadPressProbe` is the shell that measures.
 *
 *  Both reports go out as a `warning`, which `showToast` holds until the user
 *  dismisses it. The reader has to screenshot the numbers, so a toast that
 *  clears itself after a few seconds would report to nobody. */

/** A box in the same client space `elementFromPoint` and a touch's `clientX`
 *  are quoted in. */
export interface ProbeRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** The four numbers that separate a layout fault from an event fault. Carried
 *  in every report, because the reporter is a phone with a screenshot. */
export interface ProbeViewport {
  vvHeight: number;
  vvOffsetTop: number;
  innerHeight: number;
  appHeight: string;
}

export interface LandingFacts {
  /** Where the finger touched down. */
  point: { x: number; y: number };
  /** The Send button's box, or null when it is not on screen. */
  sendRect: ProbeRect | null;
  /** Whether the `touchstart` was dispatched to Send, or inside it. */
  targetIsSend: boolean;
  /** What the browser reports at `point`, for the disagreement case. */
  elementAtPoint: string | null;
  viewport: ProbeViewport;
}

function inside(rect: ProbeRect, x: number, y: number): boolean {
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function viewportSuffix(v: ProbeViewport): string {
  return `vv ${Math.round(v.vvHeight)} +${Math.round(v.vvOffsetTop)}, `
    + `inner ${Math.round(v.innerHeight)}, app ${v.appHeight || 'unset'}`;
}

/** Did the press land on the pixels the button is painted on, yet go somewhere
 *  else? That is the browser hit-testing against a layout it is no longer
 *  painting, and the offset is the evidence. Null when nothing is wrong. */
export function landingReport(f: LandingFacts): string | null {
  if (!f.sendRect || f.targetIsSend) return null;
  if (!inside(f.sendRect, f.point.x, f.point.y)) return null;
  const centreY = Math.round((f.sendRect.top + f.sendRect.bottom) / 2);
  return 'Send did not register: the tap was on the button but the browser sent '
    + `it to ${f.elementAtPoint ?? 'nothing'}. Button centre y ${centreY}, `
    + `touch y ${Math.round(f.point.y)}. ${viewportSuffix(f.viewport)}`;
}

/** The press reached the button and neither path acted. Nothing was suppressed
 *  on `touchend`, and no `click` followed. */
export function silentPressReport(v: ProbeViewport): string {
  return 'Send did not register: the tap reached the button and nothing ran. '
    + viewportSuffix(v);
}

/** How long to wait after `touchend` for a `click` before calling the press
 *  dead. Longer than WebKit's synthetic-click delay, short enough that the
 *  toast still belongs to the tap the user just made. */
const CLICK_GRACE_MS = 600;

/** The one selector the probe watches. The morph button is what was reported,
 *  and watching the whole row would fire on controls nobody complained about. */
const SEND_SELECTOR = '.send-cancel-morph';

/** The morph button's label while it is the constructive Send. The button is
 *  one node across four modes, and only this one has a touch path. */
const SEND_ARIA_LABEL = 'Send message';

/** The face a press is entitled to activate, and the ONLY one the probe
 *  watches. Three exclusions, each a press the app drops on purpose.
 *
 *  A `morph-placeholder` is invisible and inert, holding the row's height. A
 *  disabled face is the settling Stop. A face reading Cancel or Stop has no
 *  touch path at all, so a press sliding off it is a deliberate abort with no
 *  click. That is indistinguishable from the fault being chased.
 *
 *  Without these the probe toasts during ordinary use, and a diagnostic that
 *  cries wolf is one the reader learns to ignore. Structural rather than a DOM
 *  node so it tests without one. */
export function pressIsWatchable(face: {
  disabled: boolean;
  placeholder: boolean;
  ariaLabel: string | null;
}): boolean {
  return !face.disabled && !face.placeholder && face.ariaLabel === SEND_ARIA_LABEL;
}

function readFace(btn: HTMLButtonElement) {
  return {
    disabled: btn.disabled,
    placeholder: btn.classList.contains('morph-placeholder'),
    ariaLabel: btn.getAttribute('aria-label'),
  };
}

function readViewport(): ProbeViewport {
  const vv = window.visualViewport;
  return {
    vvHeight: vv?.height ?? window.innerHeight,
    vvOffsetTop: vv?.offsetTop ?? 0,
    innerHeight: window.innerHeight,
    appHeight: document.documentElement.style.getPropertyValue('--app-height'),
  };
}

function describe(el: Element | null): string | null {
  if (!el) return null;
  const cls = el.classList.item(0);
  return cls ? `${el.tagName.toLowerCase()}.${cls}` : el.tagName.toLowerCase();
}

/** True while the user is typing, which is the only condition this was
 *  reported under. Keeps the probe off every other tap in the app. */
function composerFocused(): boolean {
  const el = document.activeElement as HTMLElement | null;
  return el?.dataset?.role === 'prompt-input';
}

let installed = false;

/** Install the probe. Idempotent, and mobile-only: the report is an iOS PWA
 *  one, and a desktop click path has never been in question. */
export function installDeadPressProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

  // Set from `touchstart` when the press reached an actionable Send, cleared
  // the moment anything activates. A timer reads it to decide the press died.
  let armed = false;
  let graceTimer: ReturnType<typeof setTimeout> | null = null;

  const disarm = () => {
    armed = false;
    if (graceTimer !== null) { clearTimeout(graceTimer); graceTimer = null; }
  };

  // Capture, so an inert or covered target still reports. Passive: the probe
  // observes and never changes what the gesture does.
  document.addEventListener('touchstart', (e) => {
    disarm();
    if (!isMobile() || !composerFocused()) return;
    const btn = document.querySelector<HTMLButtonElement>(SEND_SELECTOR);
    if (!btn || !pressIsWatchable(readFace(btn))) return;
    const touch = e.changedTouches?.[0];
    if (!touch) return;
    const target = e.target as Element | null;
    const targetIsSend = !!target && (target === btn || btn.contains(target));
    const report = landingReport({
      point: { x: touch.clientX, y: touch.clientY },
      sendRect: btn.getBoundingClientRect(),
      targetIsSend,
      elementAtPoint: describe(document.elementFromPoint(touch.clientX, touch.clientY)),
      viewport: readViewport(),
    });
    if (report) { showToast(report, 'warning'); return; }
    if (targetIsSend) armed = true;
  }, { capture: true, passive: true });

  // BUBBLE phase, so the button's own handler has already run and
  // `defaultPrevented` says whether the touch path took the press.
  document.addEventListener('touchend', (e) => {
    if (!armed) return;
    if (e.defaultPrevented) { disarm(); return; }
    // No touch activation. A click may still be coming, so give it the grace
    // window before calling the press dead.
    graceTimer = setTimeout(() => {
      graceTimer = null;
      if (!armed) return;
      armed = false;
      showToast(silentPressReport(readViewport()), 'warning');
    }, CLICK_GRACE_MS);
  }, { passive: true });

  document.addEventListener('click', disarm, { capture: true, passive: true });
  document.addEventListener('touchcancel', disarm, { capture: true, passive: true });
}
