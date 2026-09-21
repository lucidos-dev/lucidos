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
  noLiftReport,
  pressWasAlone,
  pressIsWatchable,
  faceExclusion,
  underFingerReason,
  distanceOutside,
  nearestFaceMiss,
  missVector,
  screenOffset,
  morphStateOf,
  faceName,
  shouldNudgeUntouched,
  shouldReportSilence,
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
    alone: true,
    viewport: VIEWPORT,
  };

  it('stays silent on a press that shared the glass', () => {
    // WebKit synthesises no click for a multi-finger gesture, so this press
    // produces exactly what a dead one does and is not dead at all. The
    // seventeenth report's Archive toast is the false alarm that costs.
    expect(deadPressReport({ ...BASE, alone: false })).toBeNull();
    expect(deadPressReport({ ...BASE, alone: false, connectedAtLift: false })).toBeNull();
  });

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
  const BASE = { face: 'Cancel', movedPx: 0, alone: true, viewport: VIEWPORT };

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

  it('stays silent on a press that shared the glass, which is a pinch', () => {
    expect(canceledPressReport({ ...BASE, alone: false })).toBeNull();
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

  it('names the placeholder ahead of disabled, since that mode is both', () => {
    // Placeholder mode renders `disabled` as well, so the two overlap. The
    // specific answer is the one a report can act on.
    expect(faceExclusion({ disabled: true, placeholder: true })).toBe('placeholder');
    expect(faceExclusion({ disabled: true, placeholder: false })).toBe('disabled');
    expect(faceExclusion({ disabled: false, placeholder: false })).toBe('watchable');
  });

  it('ignores a disabled face, which is a settling Stop or a busy Apply', () => {
    expect(pressIsWatchable({ ...LIVE, disabled: true })).toBe(false);
  });
});

// The second output channel. A toast reports to whoever is looking at the
// screen and keeps it, which is how five episodes produced nothing to work
// from. The breadcrumb lands in engine.log and can be read back later.
describe('underFingerReason: why no watchable face took the press', () => {
  // Every `missed` line in the tenth report's ledger carried no face at all, so
  // three different situations wrote the same line. Only one of them is a bug.

  it('names an excluded action face, which is the one that matters', () => {
    expect(underFingerReason({ actionFace: 'disabled', otherButton: false }))
      .toBe('disabled-face');
    expect(underFingerReason({ actionFace: 'placeholder', otherButton: false }))
      .toBe('placeholder-face');
  });

  it('tells a control button apart from an action face', () => {
    // The row also holds `.icon-btn` controls, which this module never watched
    // and never should. A tap on one is ordinary use.
    expect(underFingerReason({ actionFace: null, otherButton: true }))
      .toBe('other-button');
  });

  it('says nothing sat there at all, for a tap on empty row space', () => {
    expect(underFingerReason({ actionFace: null, otherButton: false }))
      .toBe('nothing');
  });

  it('prefers the action face when a control button is under it too', () => {
    expect(underFingerReason({ actionFace: 'disabled', otherButton: true }))
      .toBe('disabled-face');
  });

  it('calls a watchable face nothing, since it would have claimed the press', () => {
    expect(underFingerReason({ actionFace: 'watchable', otherButton: false }))
      .toBe('nothing');
  });
});

describe('nearestFaceMiss: how far the finger fell outside a face', () => {
  // The reading the twelfth episode needed. Its line said only that the press
  // took no face. A finger 8px off a live Send produces that, and so does one
  // in the middle of the row. Only the first is geometry we own (ADR 0183).
  const SEND = { name: 'Send message', rect: { left: 330, right: 374, top: 400, bottom: 444 } };
  const DIFF = { name: 'Diff', rect: { left: 250, right: 310, top: 408, bottom: 436 } };

  it('is zero anywhere inside the face', () => {
    expect(distanceOutside(SEND.rect, { x: 350, y: 420 })).toBe(0);
    expect(distanceOutside(SEND.rect, { x: 374, y: 444 })).toBe(0);
  });

  it('measures the gap to the nearest edge', () => {
    expect(distanceOutside(SEND.rect, { x: 382, y: 420 })).toBe(8);
    expect(distanceOutside(SEND.rect, { x: 350, y: 450 })).toBe(6);
  });

  it('measures a corner as the diagonal it is', () => {
    expect(distanceOutside(SEND.rect, { x: 377, y: 448 })).toBe(5);
  });

  it('picks the face the finger came closest to', () => {
    expect(nearestFaceMiss([DIFF, SEND], { x: 382, y: 420 }))
      .toEqual({ face: SEND, px: 8, dx: 8, dy: 0 });
    expect(nearestFaceMiss([DIFF, SEND], { x: 320, y: 420 }))
      .toEqual({ face: DIFF, px: 10, dx: 10, dy: 0 });
  });

  // The caller's own entry comes back, not a copy of its name, because the
  // lift repairs the element the finger was reaching for.
  it('hands back the entry it was given', () => {
    const picked = nearestFaceMiss([DIFF, SEND], { x: 382, y: 420 });
    expect(picked?.face).toBe(SEND);
  });

  it('answers for a row holding no face at all', () => {
    // An empty composer is faceless, and a tap into it misses nothing.
    expect(nearestFaceMiss([], { x: 10, y: 420 })).toBeNull();
  });
});

describe('screenOffset: where the finger is against where the page says it is', () => {
  // Every other reading in the module comes from the layout side, so they
  // agree with each other during an episode. This one does not (ADR 0183).
  it('is the difference between the two coordinate spaces', () => {
    expect(screenOffset({ screenX: 350, screenY: 479, clientX: 350, clientY: 420 }))
      .toEqual({ x: 0, y: 59 });
  });

  it('rounds, because a sub-pixel offset is noise against a finger', () => {
    expect(screenOffset({ screenX: 350.4, screenY: 479.6, clientX: 350, clientY: 420 }))
      .toEqual({ x: 0, y: 60 });
  });
});

describe('morphStateOf: what "the send button" was showing', () => {
  // "The button is there, it just doesn't work" is a claim about this, and no
  // line has ever carried it.
  const base = { present: true, placeholder: false, disabled: false, label: 'Send message' };

  it('reads a live Send', () => {
    expect(morphStateOf(base)).toBe('send');
  });

  it('reads a live Stop by its label, since the node is the same one', () => {
    expect(morphStateOf({ ...base, label: 'Cancel' })).toBe('cancel');
  });

  it('reads the settling Stop as disabled rather than as a Cancel', () => {
    expect(morphStateOf({ ...base, disabled: true, label: 'Cancel' })).toBe('disabled');
  });

  it('reads the invisible placeholder ahead of the disabled it also carries', () => {
    expect(morphStateOf({ ...base, placeholder: true, disabled: true })).toBe('placeholder');
  });

  it('reads answer mode, where the morph is not rendered at all', () => {
    expect(morphStateOf({ ...base, present: false })).toBe('absent');
  });
});

describe('the breadcrumb channel', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../deadPressProbe.ts'), 'utf-8');
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');

  it('writes one line per watched press, under one category', () => {
    expect(code).toContain(`postClientLog('composer-press'`);
  });

  it('records every verdict a press can end on', () => {
    for (const verdict of [
      'dead', 'multi-touch', 'clicked', 'canceled', 'missed', 'no-lift', 'click-no-touch',
      'covered', 'stray-click', 'untouched',
    ]) {
      expect(code).toContain(`'${verdict}'`);
    }
    // 'served' and 'swallowed' come from `takePressOutcome`, not from a
    // literal here, and reach the line through the same fallback.
    expect(code).toContain(`outcome ?? (alone ? 'dead' : 'multi-touch')`);
  });

  it('dispatches nothing, on any path', () => {
    // The seventeenth report. The settle used to click the commit face, and it
    // sent a draft on a tap 138px from Send. A diagnostic that acts is a bug of
    // its own, so the module is read-only with respect to activation.
    expect(code).not.toMatch(/\.click\(/);
    expect(code).not.toContain('activated');
  });

  it('carries nothing the user typed', () => {
    // A log line is not a place for the draft (.claude/rules/no-private-data.md).
    // The module must not even be able to reach one.
    expect(code).not.toContain('getDraft');
    expect(code).not.toContain('composeDrafts');
    expect(code).not.toMatch(/\.value\b/);
  });
});

// The lift that never came. WebKit owes a `touchend` or a `touchcancel` for
// every `touchstart`. Neither arriving is the touch pipeline stopping
// mid-gesture, rather than a button declining a press.
describe('noLiftReport: the press arrived and the lift did not', () => {
  const BASE = { face: 'Send message', movedPx: 0, alone: true, viewport: VIEWPORT };

  it('reports a stationary press whose lift never arrived', () => {
    const report = noLiftReport(BASE);
    expect(report).toMatch(/^Send message did not register/);
    expect(report).toContain('the lift never arrived');
  });

  it('stays silent on a press that moved, which the page may hand to a scroller', () => {
    expect(noLiftReport({ ...BASE, movedPx: 40 })).toBeNull();
  });

  it('stays silent on a press that shared the glass', () => {
    expect(noLiftReport({ ...BASE, alone: false })).toBeNull();
  });

  it('shares the tap gate threshold rather than inventing a third one', () => {
    expect(noLiftReport({ ...BASE, movedPx: 8 })).not.toBeNull();
    expect(noLiftReport({ ...BASE, movedPx: 9 })).toBeNull();
  });

  it('carries the viewport numbers, like every other report here', () => {
    expect(noLiftReport(BASE)).toContain('kbd on');
  });
});

// The seventeenth report opened with an Archive press called dead. Its
// `quiet.ms` of 13 places a second contact on the glass 13ms in front of it.
// Nothing on the line could say so, because nothing counted the fingers.
describe('pressWasAlone: was this press the only contact on the glass', () => {
  it('takes one finger down and nothing left at the lift', () => {
    expect(pressWasAlone({ fingers: 1, fingersAtLift: 0 })).toBe(true);
  });

  it('refuses a press that began beside another finger', () => {
    expect(pressWasAlone({ fingers: 2, fingersAtLift: 0 })).toBe(false);
  });

  it('refuses one a finger joined halfway through', () => {
    // The running maximum is why the count is not read at touchdown alone.
    expect(pressWasAlone({ fingers: 2, fingersAtLift: 1 })).toBe(false);
  });

  it('refuses one that left a finger behind', () => {
    expect(pressWasAlone({ fingers: 1, fingersAtLift: 1 })).toBe(false);
  });

  it('takes a count of zero, which a synthetic event reports', () => {
    expect(pressWasAlone({ fingers: 0, fingersAtLift: 0 })).toBe(true);
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
    expect(code).toContain(`'.action-btn'`);
    expect(code).toMatch(/querySelectorAll<HTMLButtonElement>\(\s*`\$\{ROW_SELECTOR\} \$\{FACE_SELECTOR\}`/);
  });

  it('names the morph once, to read it, never to pick what it watches', () => {
    // "The button is there, it just doesn't work" is a claim about the morph's
    // own state, and no line used to carry it. Reaching for the node is
    // legitimate; building the watched SET from it is the miss above.
    //
    // ONE reach. Its two readers, the mode and the rescue, share `morphElement`
    // so they can never end up asking about two different nodes.
    const morphLines = code.split('\n').filter((l: string) => l.includes('send-cancel-morph'));
    expect(morphLines).toHaveLength(1);
    for (const line of morphLines) expect(line).toContain('querySelector<HTMLButtonElement>');
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
    // `continue`, not `return`: more than one press can be settling at once, so
    // a click has to be offered to each before it is called touchless.
    expect(code).toMatch(/if \(!onPressedFace && !onReplacement\) continue/);
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

  it('takes the outcome a task after the press\u2019s OWN lift', () => {
    // `takePressOutcome` is one consuming slot. Read at the end of a 600ms
    // grace window, an earlier press swallows a later press's claim. Taking it
    // right after each lift keeps every claim with the press that earned it.
    // The window is still measured from that press's own arm.
    expect(code).toContain('takePressOutcome(Date.now() - lifted.armedAt)');
    // And the task is HELD, so a click that rules the press early can cancel
    // it. `takePressOutcome` consumes, so a stray one eats the next claim.
    expect(code).toMatch(/lifted\.outcomeTimer = setTimeout/);
    expect(code).toMatch(/clearTimeout\(press\.outcomeTimer\)/);
  });

  it('rules an armed press that never lifted, rather than dropping it', () => {
    // The shape a touch pipeline that stops mid-gesture leaves, and the one the
    // eighth episode's silence points at.
    expect(code).toContain('ruleArmedWithNoLift');
    // And on a deadline, because a stopped pipeline delivers no next touch to
    // notice the loss with.
    expect(code).toContain('LIFT_DEADLINE_MS');
  });

  it('records a click that had no touch behind it', () => {
    // A live click path over a dead touch path. The probe only ever armed from
    // a `touchstart`, so it could not see that split at all.
    expect(code).toContain('lastTouchStartAt');
    // Gated on touch CAPABILITY, not on a narrow window: a mouse client cannot
    // have a touch pipeline that stopped.
    expect(code).toContain('isTouchDevice()');
  });

  it('carries the row and face boxes, so an offset is measured', () => {
    expect(code).toContain('rowRect');
    expect(code).toContain('faceRect');
  });

  it('watches the row by where it is, not by what has focus', () => {
    // iOS can hold the keyboard up after focus has left the textarea, and the
    // old focus gate excluded exactly that state.
    expect(code).not.toContain('activeElement');
    expect(code).toContain('watchableRow');
  });

  it('stands the reachability check down while an overlay is open', () => {
    // One of the two times something is MEANT to cover the row.
    expect(code).toContain(`'data-overlay-open'`);
  });

  it('stands it down under the cover a client refresh raises too', () => {
    // The other one, and it was missing. A refresh dims and locks the page on
    // purpose, so the composer under it is unreachable by design. The probe
    // named the blocker in a wedge report, over the app's own "Refreshing"
    // status. It then spent the episode's one repair on a doomed layout.
    expect(code).toContain(`'data-ui-blocked'`);
  });
});

describe('missVector: the signed miss, which a scalar cannot express', () => {
  // The one extra reading round 14 asked for. `screenOff` reads {0,0} on every
  // ledger line, healthy or dead. So nothing else separates a finger that
  // landed off a face from a page hit-testing at an offset.
  const RECT = { left: 289, right: 357, top: 807, bottom: 832 };

  it('is zero on both axes inside the face', () => {
    expect(missVector(RECT, { x: 321, y: 819 })).toEqual({ dx: 0, dy: 0 });
  });

  it('signs a press ABOVE the face negative, as the caught one was', () => {
    expect(missVector(RECT, { x: 321, y: 798 })).toEqual({ dx: 0, dy: -9 });
  });

  it('signs a press below the face positive', () => {
    expect(missVector(RECT, { x: 321, y: 841 })).toEqual({ dx: 0, dy: 9 });
  });

  it('signs the horizontal axis the same way, and both at once', () => {
    expect(missVector(RECT, { x: 280, y: 798 })).toEqual({ dx: -9, dy: -9 });
    expect(missVector(RECT, { x: 366, y: 841 })).toEqual({ dx: 9, dy: 9 });
  });

  it('agrees with the scalar distance it replaced', () => {
    expect(distanceOutside(RECT, { x: 321, y: 798 })).toBe(9);
    expect(distanceOutside(RECT, { x: 280, y: 798 })).toBe(13);
  });
});

describe('nearestFaceMiss carries the vector to the face it names', () => {
  it('reports the signed miss beside the distance', () => {
    const face = { name: 'Submit answer', rect: { left: 289, right: 357, top: 807, bottom: 832 } };
    expect(nearestFaceMiss([face], { x: 321, y: 798 }))
      .toEqual({ face, px: 9, dx: 0, dy: -9 });
  });
});

describe('shouldReportSilence: the wedge whose signature is silence', () => {
  // The eighteenth report. Thirteen scheduled checks ran through a 36 second
  // stretch where the page took nothing, and the ledger held not one line. The
  // check is the only reading that runs in that state, so it is the only one
  // that can name it.
  const wedged = {
    hasCommitFace: true,
    msSinceKeyboardClose: 4000,
    msSinceInput: 40000,
  };

  it('names a silence that followed the keyboard going', () => {
    expect(shouldReportSilence(wedged)).toBe(true);
  });

  it('says nothing before the silence has lasted a tick', () => {
    // The user who closes the keyboard and taps Send at once.
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 900 })).toBe(false);
  });

  it('measures from the LAST input, never from the close alone', () => {
    // The correction the episode forced. Two inputs landed a second after the
    // keys went, so a close-only anchor would have refused the whole window.
    expect(shouldReportSilence({ ...wedged, msSinceInput: 500 })).toBe(false);
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 40000, msSinceInput: 4000 }))
      .toBe(true);
  });

  it('says nothing until a keyboard has actually closed', () => {
    // The state this describes starts at the close. Without one there is
    // nothing to say, however long the page has been quiet.
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: null })).toBe(false);
  });

  it('holds the silence against the close when the page took nothing at all', () => {
    expect(shouldReportSilence({ ...wedged, msSinceInput: null })).toBe(true);
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 900, msSinceInput: null }))
      .toBe(false);
  });

  it('has nothing to report with no commit face, which is an idle phone', () => {
    // The bound that keeps the line rare. A live commit face says the composer
    // has something to send, which is the state a user taps at.
    expect(shouldReportSilence({ ...wedged, hasCommitFace: false })).toBe(false);
  });
});

describe('shouldNudgeUntouched: the recovery with no gesture behind it', () => {
  // The fourteenth PWA episode took no touch and no click for 18.5 seconds.
  // Both of ADR 0183's answers wait to be touched, so this is the one state
  // neither can reach. Typing is what arms it instead.
  const waiting = {
    hasCommitFace: true,
    typedSinceComposerInput: true,
    msSinceKeystroke: 4000,
  };

  it('runs for a typed draft the composer has taken no touch since', () => {
    expect(shouldNudgeUntouched(waiting)).toBe(true);
  });

  it('stays out of a healthy send, where the tap follows the typing at once', () => {
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 800 })).toBe(false);
  });

  it('has nothing to protect with no commit face in the row', () => {
    expect(shouldNudgeUntouched({ ...waiting, hasCommitFace: false })).toBe(false);
  });

  it('stands down once a touch did reach the composer after the typing', () => {
    // The COMPOSER, not the page, and round 15 is the reason. A touch the page
    // took elsewhere used to disarm this for the rest of an episode.
    expect(shouldNudgeUntouched({ ...waiting, typedSinceComposerInput: false })).toBe(false);
  });

  it('never fires before the user has typed at all', () => {
    // `msSinceKeystroke` is 0 with no keystroke on record, which the elapsed
    // gate already refuses. Both gates say no, and neither alone should.
    expect(shouldNudgeUntouched({
      ...waiting, typedSinceComposerInput: false, msSinceKeystroke: 0,
    })).toBe(false);
  });

  it('stops once the user has plainly put the phone down', () => {
    // A COUNT cannot bound this, and the wedge is why: it delivers no touch and
    // no click, so nothing it allows would ever reset one. A user re-reading a
    // draft would spend the budget before the first tap.
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 59_000 })).toBe(true);
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 61_000 })).toBe(false);
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 600_000 })).toBe(false);
  });
});
