import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  landingReport,
  deadPressReport,
  canceledPressReport,
  faceHitTestReport,
  pressIsWatchable,
  faceName,
  type FaceHitTestFacts,
  type LandingFacts,
  type ProbeViewport,
} from '../deadPressProbe';

// The probe exists because the dead-composer-button report has arrived four
// times and says only "nothing happened". Its whole value is that the NEXT
// report names which half of the stack failed. So these cases pin the verdicts
// apart, and pin the silence on every healthy tap.

const VIEWPORT: ProbeViewport = {
  vvHeight: 460,
  vvOffsetTop: 0,
  innerHeight: 844,
  appHeight: '460px',
  keyboardActive: true,
  pageScrollY: 0,
};

/** An answer Submit as it sits with the keyboard up: a labelled pill low in the
 *  shrunken viewport. */
const FACE_RECT = { left: 300, right: 359, top: 420, bottom: 449 };

function facts(over: Partial<LandingFacts> = {}): LandingFacts {
  return {
    face: 'Submit answer',
    point: { x: 330, y: 434 },
    faceRect: FACE_RECT,
    targetIsFace: true,
    elementAtPoint: 'button.action-btn',
    pointerEventsAtPoint: 'auto',
    viewport: VIEWPORT,
    ...over,
  };
}

describe('landingReport: did the press reach the button it was aimed at', () => {
  it('says nothing when the press landed on the button', () => {
    expect(landingReport(facts())).toBeNull();
  });

  it('says nothing when the press was nowhere near the button', () => {
    // A tap in the transcript, or on another control. Most taps are this.
    expect(landingReport(facts({ point: { x: 40, y: 120 }, targetIsFace: false }))).toBeNull();
  });

  it('says nothing when the button is not on screen', () => {
    expect(landingReport(facts({ faceRect: null, targetIsFace: false }))).toBeNull();
  });

  it('reports the press that was on the button but went elsewhere', () => {
    // The hit-test family: the browser is testing against a layout it is no
    // longer painting, so the pixels under the finger are not what answered.
    const report = landingReport(facts({
      targetIsFace: false,
      elementAtPoint: 'div.thread-content',
    }));
    expect(report).toContain('the tap was on the button');
    expect(report).toContain('div.thread-content');
  });

  it('names the face, so the report says WHICH button died', () => {
    // The previous probe watched one node and could not answer this. The user's
    // fourth report was three other faces.
    const report = landingReport(facts({ face: 'Diff', targetIsFace: false }));
    expect(report).toMatch(/^Diff did not register/);
  });

  it('carries the pointer-events at the point, which names an inert overlay', () => {
    const report = landingReport(facts({
      targetIsFace: false,
      elementAtPoint: 'div.app-shell',
      pointerEventsAtPoint: 'none',
    }));
    expect(report).toContain('pointer-events none');
  });

  it('carries the touch and the button centre, so the offset is readable', () => {
    const report = landingReport(facts({ targetIsFace: false, point: { x: 330, y: 425 } }));
    expect(report).toContain('centre y 435');
    expect(report).toContain('touch y 425');
  });

  it('carries the viewport numbers and the keyboard flag', () => {
    const report = landingReport(facts({
      targetIsFace: false,
      viewport: { ...VIEWPORT, vvOffsetTop: 87, appHeight: '744px', keyboardActive: false },
    }));
    expect(report).toContain('vv 460 +87');
    expect(report).toContain('inner 844');
    expect(report).toContain('app 744px');
    expect(report).toContain('kbd off');
  });

  it('names the absence when nothing at all sits under the finger', () => {
    const report = landingReport(facts({ targetIsFace: false, elementAtPoint: null }));
    expect(report).toContain('nothing');
  });

  it('carries the page scroll offset, which no report used to', () => {
    // A layout viewport scrolled under a fixed shell is the textbook form of
    // this bug on iOS. It is invisible in every other number here.
    const report = landingReport(facts({
      targetIsFace: false,
      viewport: { ...VIEWPORT, pageScrollY: 132 },
    }));
    expect(report).toContain('scroll 132');
  });
});

// The question the finger cannot influence, and the one the landing check above
// could not ask. That check needs the touch point INSIDE a painted rect, so a
// coordinate space out of step with layout missed every rect and said nothing.
describe('faceHitTestReport: is the face reachable where it is drawn', () => {
  function hit(over: Partial<FaceHitTestFacts> = {}): FaceHitTestFacts {
    return {
      face: 'Send message',
      centre: { x: 330, y: 434 },
      answeredWithFace: true,
      elementAtCentre: 'button.action-btn',
      pointerEventsAtCentre: 'auto',
      viewport: VIEWPORT,
      ...over,
    };
  }

  it('says nothing when the browser answers with the face', () => {
    expect(faceHitTestReport(hit())).toBeNull();
  });

  it('says nothing when it answers with something inside the face', () => {
    // The icon inside the button. The caller resolves that through
    // `face.contains`, so a descendant is the face answering.
    expect(faceHitTestReport(hit({ elementAtCentre: 'svg.icon' }))).toBeNull();
  });

  it('reports an ancestor answering, which is the face taking no pointer', () => {
    const report = faceHitTestReport(hit({
      answeredWithFace: false,
      elementAtCentre: 'div.prompt-actions-row',
      pointerEventsAtCentre: 'none',
    }));
    expect(report).toContain('not reachable where it is drawn');
    expect(report).toContain('div.prompt-actions-row');
    expect(report).toContain('pointer-events none');
  });

  it('reports an unrelated element, which is a stale hit-test or a cover', () => {
    const report = faceHitTestReport(hit({
      answeredWithFace: false,
      elementAtCentre: 'div.thread-content',
    }));
    expect(report).toMatch(/^Send message is not reachable/);
    expect(report).toContain('(330, 434)');
  });

  it('carries the viewport numbers, the keyboard flag and the scroll', () => {
    const report = faceHitTestReport(hit({
      answeredWithFace: false,
      viewport: { ...VIEWPORT, pageScrollY: 87 },
    }));
    expect(report).toContain('vv 460 +0');
    expect(report).toContain('kbd on');
    expect(report).toContain('scroll 87');
  });

  it('names the absence when the page answers with nothing', () => {
    const report = faceHitTestReport(hit({ answeredWithFace: false, elementAtCentre: null }));
    expect(report).toContain('nothing');
  });
});

// The decisive question, and the reason this round exists. Apple's Handling
// Events page says a page change during the tap cascade stops the rest of it.
// A `touchend` goes to the element the press STARTED on, so a replaced node
// kills the touch path and the click path together.
describe('deadPressReport: the press arrived and no path took it', () => {
  const BASE = {
    face: 'Submit answer',
    movedPx: 0,
    connectedAtLift: true,
    rowMutations: 0,
    outcome: null,
    viewport: VIEWPORT,
  };

  it('stays silent on a press somebody claimed, whichever of them took it', () => {
    // `served` is the button's own touch path running its action. `swallowed`
    // is the overlay contract eating the paired event of a dismissing tap,
    // which is legitimate: Send can sit under a popover. Neither is a fault,
    // and both are indistinguishable from a dead press without the claim.
    expect(deadPressReport({ ...BASE, outcome: 'served' })).toBeNull();
    expect(deadPressReport({ ...BASE, outcome: 'swallowed' })).toBeNull();
    expect(deadPressReport({ ...BASE, outcome: 'served', rowMutations: 4 })).toBeNull();
  });

  it('stays silent on a press that slid off the button', () => {
    // Every click-only face (Stop, Cancel, the banner actions) produces no
    // click when the press slides off, by design. Reporting that would toast
    // through ordinary use. The fault chased here is a STATIONARY tap.
    expect(deadPressReport({ ...BASE, movedPx: 40 })).toBeNull();
    expect(deadPressReport({ ...BASE, movedPx: 40, connectedAtLift: false })).toBeNull();
  });

  it('shares the tap gate threshold with the cancelled-gesture report', () => {
    expect(deadPressReport({ ...BASE, movedPx: 8 })).not.toBeNull();
    expect(deadPressReport({ ...BASE, movedPx: 9 })).toBeNull();
  });

  it('names a replaced button outright, never as a hit-test miss', () => {
    const report = deadPressReport({ ...BASE, connectedAtLift: false, rowMutations: 4 });
    expect(report).toContain('replaced in the page while your finger was on it');
    expect(report).toContain('4 row changes');
    expect(report).not.toContain('hit');
  });

  it('reports a surviving button whose row churned, the same cause one step weaker', () => {
    const report = deadPressReport({ ...BASE, rowMutations: 2 });
    expect(report).toContain('the button survived');
    expect(report).toContain('changed 2 times');
  });

  it('falls back to both-paths-declined on a stable node', () => {
    const report = deadPressReport(BASE);
    expect(report).toContain('reached the button and nothing ran');
  });

  it('carries the viewport numbers in every shape', () => {
    for (const shape of [
      BASE,
      { ...BASE, rowMutations: 3 },
      { ...BASE, connectedAtLift: false },
    ]) {
      expect(deadPressReport(shape)).toContain('vv 460 +0');
      expect(deadPressReport(shape)).toContain('kbd on');
    }
  });

  it('survives a viewport that never published --app-height', () => {
    // Desktop, or a boot before MobileSwipeContainer's first write. An empty
    // string in the middle of the line reads as a truncated report.
    const report = deadPressReport({ ...BASE, viewport: { ...VIEWPORT, appHeight: '' } });
    expect(report).toContain('app unset');
  });
});

// The family the previous plan deferred. A gesture the system takes runs no
// path and produces no click, which is indistinguishable from the fault being
// chased unless the app says so.
describe('canceledPressReport: the system took the gesture', () => {
  const BASE = { face: 'Cancel', movedPx: 0, viewport: VIEWPORT };

  it('reports a stationary press the system still cancelled', () => {
    const report = canceledPressReport(BASE);
    expect(report).toContain('the system cancelled the touch after 0px');
    expect(report).toMatch(/^Cancel did not register/);
  });

  it('stays silent on a press that moved, which is a scroll', () => {
    // A cancelled scroll is the platform working. Toasting on it is how a
    // diagnostic teaches the reader to ignore it.
    expect(canceledPressReport({ ...BASE, movedPx: 40 })).toBeNull();
  });

  it('shares the tap gate threshold rather than inventing a second one', () => {
    // 8px, from `tapGesture`. Exactly at the threshold is still a tap.
    expect(canceledPressReport({ ...BASE, movedPx: 8 })).not.toBeNull();
    expect(canceledPressReport({ ...BASE, movedPx: 9 })).toBeNull();
  });
});

// A diagnostic that cries wolf gets ignored. What is excluded is a press the
// app drops on purpose, and NOT a face the previous probe simply did not name.
describe('pressIsWatchable: every actionable face in the row', () => {
  const LIVE = { disabled: false, placeholder: false };

  it('watches an enabled face', () => {
    expect(pressIsWatchable(LIVE)).toBe(true);
  });

  it('ignores the invisible placeholder that holds the row height', () => {
    expect(pressIsWatchable({ ...LIVE, placeholder: true })).toBe(false);
  });

  it('ignores a disabled face, which is a settling Stop or a busy Apply', () => {
    expect(pressIsWatchable({ ...LIVE, disabled: true })).toBe(false);
  });
});

// The second output channel. A toast reports to whoever is looking at the
// screen and keeps it, which is how five episodes produced nothing to work
// from. The breadcrumb lands in engine.log and can be read back later.
describe('the breadcrumb channel', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../deadPressProbe.ts'), 'utf-8');
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');

  it('writes one line per watched press, under one category', () => {
    expect(code).toContain(`postClientLog('composer-press'`);
  });

  it('records every verdict a press can end on', () => {
    for (const verdict of ['dead', 'clicked', 'canceled', 'missed']) {
      expect(code).toContain(`'${verdict}'`);
    }
    // 'served' and 'swallowed' come from `takePressOutcome`, not from a
    // literal here, and reach the line through `outcome ?? 'dead'`.
    expect(code).toContain(`outcome ?? 'dead'`);
  });

  it('carries nothing the user typed', () => {
    // A log line is not a place for the draft (.claude/rules/no-private-data.md).
    // The module must not even be able to reach one.
    expect(code).not.toContain('getDraft');
    expect(code).not.toContain('composeDrafts');
    expect(code).not.toMatch(/\.value\b/);
  });
});

describe('faceName: what the report calls the button', () => {
  it('prefers the accessible name, since an icon-only face has no text', () => {
    expect(faceName({ ariaLabel: 'Send message', text: '' })).toBe('Send message');
  });

  it('falls back to the visible label', () => {
    expect(faceName({ ariaLabel: null, text: ' Diff ' })).toBe('Diff');
  });

  it('never reports an empty name, which reads as a truncated toast', () => {
    expect(faceName({ ariaLabel: '  ', text: '' })).toBe('A composer button');
  });
});

// The probe observes and never changes what a gesture does. A diagnostic that
// consumes a press becomes the bug it was added to chase.
describe('the probe consumes no gesture', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../deadPressProbe.ts'), 'utf-8');
  // Comments stripped, because the module DOCUMENTS what it refuses to call.
  // Scanning the prose would fail on the very sentence explaining the rule.
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');

  it('never calls preventDefault or stopPropagation', () => {
    expect(code).not.toMatch(/preventDefault|stopPropagation/);
  });

  it('registers every listener passive', () => {
    const calls = (code.match(/addEventListener\(/g) ?? []).length;
    const passive = (code.match(/passive: true/g) ?? []).length;
    expect(calls).toBeGreaterThan(0);
    expect(passive).toBe(calls);
  });

  it('watches the row rather than one named face', () => {
    // The miss this round: the probe queried `.send-cancel-morph`, and the row
    // was in answer mode, where that node is not rendered at all.
    expect(code).toContain(`'.prompt-actions-row'`);
    expect(code).not.toContain('send-cancel-morph');
  });

  it('reads isConnected at the lift, which is the decisive question', () => {
    expect(code).toMatch(/press\.el\.isConnected/);
  });

  it('settles on the pressed face, or on the node that replaced it', () => {
    // Settling on ANY click let one landing elsewhere cancel the report for a
    // press that really did die. A neighbouring face answering says nothing
    // about this one, and `isConnected` separates that from a node Preact
    // swapped under the finger.
    expect(code).toMatch(/onPressedFace = press\.el\.contains\(target\)/);
    expect(code).toMatch(/onReplacement = !press\.el\.isConnected && !!target\.closest\(ROW_SELECTOR\)/);
    expect(code).toMatch(/if \(!onPressedFace && !onReplacement\) return/);
  });

  it('hears a touchend nothing else let through', () => {
    // The arm has to be CAPTURE phase. A bubble listener on `document` is
    // skipped once anything upstream stops propagation in capture, and the
    // overlay contract's paired swallow does exactly that. That silence is
    // what hid the sixth episode.
    const arm = code.match(/addEventListener\('touchend',[\s\S]*?\}, \{([^}]*)\}\)/);
    expect(arm).not.toBeNull();
    expect(arm?.[1]).toContain('capture: true');
  });

  it('asks who took the press instead of reading defaultPrevented', () => {
    // The touch path cancels the default BEFORE running its action, so the
    // flag never told a press that ran from one that was eaten.
    expect(code).toContain('takePressOutcome');
    expect(code).not.toContain('defaultPrevented');
  });

  it('reads the outcome in a window measured from THIS press', () => {
    // A second composer touch inside the first one's grace window supersedes
    // it without consuming its claim. A fixed window would let the second
    // press read the first one's, and go quiet on a genuinely dead press.
    expect(code).toContain('takePressOutcome(Date.now() - press.armedAt)');
  });

  it('watches the row by where it is, not by what has focus', () => {
    // iOS can hold the keyboard up after focus has left the textarea, and the
    // old focus gate excluded exactly that state.
    expect(code).not.toContain('activeElement');
    expect(code).toContain('watchableRow');
  });

  it('stands the reachability check down while an overlay is open', () => {
    // That is the one time something is MEANT to cover the row.
    expect(code).toContain(`'data-overlay-open'`);
  });

  it('latches the reachability report, so one state is one toast', () => {
    // It runs on every touch while composing, and its toast holds until
    // dismissed. Unlatched, a wedged row buries the screen in copies.
    expect(code).toMatch(/reportedUnreachable\.has\(name\)\) continue/);
    expect(code).toMatch(/reportedUnreachable\.delete\(name\)/);
  });
});
