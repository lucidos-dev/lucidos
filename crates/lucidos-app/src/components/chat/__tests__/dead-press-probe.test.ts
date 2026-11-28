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
  faceExclusion,
  underFingerReason,
  missEarnsRelayout,
  distanceOutside,
  nearestFaceMiss,
  missVector,
  screenOffset,
  morphStateOf,
  faceName,
  shouldNudgeUntouched,
  nudgeIsTooSoon,
  shouldReportSilence,
  type LandingFacts,
  type ProbeViewport,
} from '../deadPressProbe';
import { LONG_PRESS_DELAY_MS } from '../../../hooks/useLongPress';


const VIEWPORT: ProbeViewport = {
  vvHeight: 460,
  vvOffsetTop: 0,
  innerHeight: 844,
  appHeight: '460px',
  keyboardActive: true,
  pageScrollY: 0,
};

const FACE_RECT = { left: 300, right: 359, top: 420, bottom: 449 };

function facts(over: Partial<LandingFacts> = {}): LandingFacts {
  return {
    face: 'Submit answer',
    point: { x: 330, y: 434 },
    faceRect: FACE_RECT,
    targetIsFace: true,
    dispatchedTo: 'button.action-btn',
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
    expect(landingReport(facts({ point: { x: 40, y: 120 }, targetIsFace: false }))).toBeNull();
  });

  it('says nothing when the button is not on screen', () => {
    expect(landingReport(facts({ faceRect: null, targetIsFace: false }))).toBeNull();
  });

  it('reports the press that was on the button but went elsewhere', () => {
    const report = landingReport(facts({
      targetIsFace: false,
      elementAtPoint: 'div.thread-content',
    }));
    expect(report).toContain('the tap was on the button');
    expect(report).toContain('div.thread-content');
  });

  it('names where the touch went apart from what answers at the point', () => {
    const report = landingReport(facts({
      targetIsFace: false,
      dispatchedTo: 'div.app-shell',
      elementAtPoint: 'button.action-btn',
    }));
    expect(report).toContain('sent it to div.app-shell');
    expect(report).toContain('button.action-btn answers at that point');
  });

  it('names the face, so the report says WHICH button died', () => {
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
    const report = landingReport(facts({
      targetIsFace: false,
      viewport: { ...VIEWPORT, pageScrollY: 132 },
    }));
    expect(report).toContain('scroll 132');
  });
});

describe('deadPressReport: the press arrived and no path took it', () => {
  const BASE = {
    face: 'Submit answer',
    movedPx: 0,
    connectedAtLift: true,
    rowMutations: 0,
    outcome: null,
    alone: true,
    heldMs: 90,
    viewport: VIEWPORT,
  };

  it('stays silent on a press held past a tap', () => {
    expect(deadPressReport({ ...BASE, heldMs: 1200 })).toBeNull();
    expect(deadPressReport({ ...BASE, heldMs: 1200, connectedAtLift: false })).toBeNull();
  });

  it('draws the hold line where the app’s own long press begins', () => {
    expect(deadPressReport({ ...BASE, heldMs: LONG_PRESS_DELAY_MS })).not.toBeNull();
    expect(deadPressReport({ ...BASE, heldMs: LONG_PRESS_DELAY_MS + 1 })).toBeNull();
  });

  it('says how long the finger was down', () => {
    expect(deadPressReport(BASE)).toContain('Held 90ms.');
  });

  it('stays silent on a press that shared the glass', () => {
    expect(deadPressReport({ ...BASE, alone: false })).toBeNull();
    expect(deadPressReport({ ...BASE, alone: false, connectedAtLift: false })).toBeNull();
  });

  it('stays silent on a press somebody claimed, whichever of them took it', () => {
    expect(deadPressReport({ ...BASE, outcome: 'served' })).toBeNull();
    expect(deadPressReport({ ...BASE, outcome: 'swallowed' })).toBeNull();
    expect(deadPressReport({ ...BASE, outcome: 'served', rowMutations: 4 })).toBeNull();
  });

  it('stays silent on a press that slid off the button', () => {
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
    const report = deadPressReport({ ...BASE, viewport: { ...VIEWPORT, appHeight: '' } });
    expect(report).toContain('app unset');
  });
});

describe('canceledPressReport: the system took the gesture', () => {
  const BASE = { face: 'Cancel', movedPx: 0, alone: true, heldMs: 90, viewport: VIEWPORT };

  it('stays silent on a press held past a tap, which the system may take', () => {
    expect(canceledPressReport({ ...BASE, heldMs: LONG_PRESS_DELAY_MS + 1 })).toBeNull();
    expect(canceledPressReport({ ...BASE, heldMs: LONG_PRESS_DELAY_MS })).not.toBeNull();
  });

  it('reports a stationary press the system still cancelled', () => {
    const report = canceledPressReport(BASE);
    expect(report).toContain('the system cancelled the touch after 0px');
    expect(report).toMatch(/^Cancel did not register/);
  });

  it('stays silent on a press that moved, which is a scroll', () => {
    expect(canceledPressReport({ ...BASE, movedPx: 40 })).toBeNull();
  });

  it('stays silent on a press that shared the glass, which is a pinch', () => {
    expect(canceledPressReport({ ...BASE, alone: false })).toBeNull();
  });

  it('shares the tap gate threshold rather than inventing a second one', () => {
    expect(canceledPressReport({ ...BASE, movedPx: 8 })).not.toBeNull();
    expect(canceledPressReport({ ...BASE, movedPx: 9 })).toBeNull();
  });
});

describe('faceExclusion: every actionable face in the row', () => {
  const LIVE = { disabled: false };

  it('watches an enabled face', () => {
    expect(faceExclusion(LIVE)).toBe('watchable');
  });

  it('ignores a disabled face: an empty Send, a settling Stop or a busy Apply', () => {
    expect(faceExclusion({ ...LIVE, disabled: true })).toBe('disabled');
  });
});

describe('underFingerReason: why no watchable face took the press', () => {

  it('names an excluded action face, which is the one that matters', () => {
    expect(underFingerReason({ actionFace: 'disabled', otherButton: false }))
      .toBe('disabled-face');
  });

  it('tells a control button apart from an action face', () => {
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

  it('names a live face that the touch never reached', () => {
    expect(underFingerReason({ actionFace: 'watchable', otherButton: true }))
      .toBe('live-face');
    expect(underFingerReason({ actionFace: 'watchable', otherButton: false }))
      .toBe('live-face');
  });
});

describe('missEarnsRelayout: which missed presses the lift relays out for', () => {
  it('relays out for empty row space and for a live face the touch missed', () => {
    expect(missEarnsRelayout('nothing')).toBe(true);
    expect(missEarnsRelayout('live-face')).toBe(true);
  });

  it('leaves a control button and an excluded face alone', () => {
    expect(missEarnsRelayout('other-button')).toBe(false);
    expect(missEarnsRelayout('disabled-face')).toBe(false);
  });
});

describe('nearestFaceMiss: how far the finger fell outside a face', () => {
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

  it('hands back the entry it was given', () => {
    const picked = nearestFaceMiss([DIFF, SEND], { x: 382, y: 420 });
    expect(picked?.face).toBe(SEND);
  });

  it('answers for a row holding no face at all', () => {
    expect(nearestFaceMiss([], { x: 10, y: 420 })).toBeNull();
  });
});

describe('screenOffset: where the finger is against where the page says it is', () => {
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
  const base = { present: true, disabled: false, label: 'Send message' };

  it('reads a live Send', () => {
    expect(morphStateOf(base)).toBe('send');
  });

  it('reads a live Stop by its label, since the node is the same one', () => {
    expect(morphStateOf({ ...base, label: 'Cancel' })).toBe('cancel');
  });

  it('reads the settling Stop as disabled rather than as a Cancel', () => {
    expect(morphStateOf({ ...base, disabled: true, label: 'Cancel' })).toBe('disabled');
  });

  it('reads the empty composer\'s Send as disabled', () => {
    expect(morphStateOf({ ...base, disabled: true })).toBe('disabled');
  });

  it('reads answer mode, where the morph is not rendered at all', () => {
    expect(morphStateOf({ ...base, present: false })).toBe('absent');
  });
});

describe('the breadcrumb channel', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../deadPressProbe.ts'), 'utf-8');
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');

  it('reads the thread-action marker the banner stamps', () => {
    expect(code).toContain(`'[data-thread-action]'`);
  });

  it('writes one line per watched press, under one category', () => {
    expect(code).toContain(`postClientLog('composer-press'`);
  });

  it('records every verdict a press can end on', () => {
    for (const verdict of [
      'dead', 'multi-touch', 'long-hold', 'clicked', 'canceled', 'missed', 'no-lift',
      'click-no-touch', 'covered', 'stray-click', 'untouched',
    ]) {
      expect(code).toContain(`'${verdict}'`);
    }
    expect(code).toContain('outcome ?? unclaimedVerdict(');
  });

  it('dispatches nothing, on any path', () => {
    expect(code).not.toMatch(/\.click\(/);
    expect(code).not.toContain('activated');
  });

  it('carries nothing the user typed', () => {
    expect(code).not.toContain('getDraft');
    expect(code).not.toContain('composeDrafts');
    expect(code).not.toMatch(/\.value\b/);
  });
});

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

describe('pressWasAlone: was this press the only contact on the glass', () => {
  it('takes one finger down and nothing left at the lift', () => {
    expect(pressWasAlone({ fingers: 1, fingersAtLift: 0 })).toBe(true);
  });

  it('refuses a press that began beside another finger', () => {
    expect(pressWasAlone({ fingers: 2, fingersAtLift: 0 })).toBe(false);
  });

  it('refuses one a finger joined halfway through', () => {
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

describe('the probe consumes no gesture', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = ['../deadPressProbe.ts', '../pressCover.ts']
    .map((file) => readFileSync(resolve(here, file), 'utf-8'))
    .join('\n');
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
    expect(code).toContain(`'.prompt-actions-row'`);
    expect(code).toContain(`'.action-btn'`);
    expect(code).toMatch(/querySelectorAll<HTMLButtonElement>\(\s*`\$\{ROW_SELECTOR\} \$\{FACE_SELECTOR\}`/);
  });

  it('names the morph once, to read it, never to pick what it watches', () => {
    const morphLines = code.split('\n').filter((l: string) => l.includes('send-cancel-morph'));
    expect(morphLines).toHaveLength(1);
    for (const line of morphLines) expect(line).toContain('querySelector<HTMLButtonElement>');
  });

  it('reads isConnected at the lift, which is the decisive question', () => {
    expect(code).toMatch(/press\.el\.isConnected/);
  });

  it('settles on the pressed face, or on the node that replaced it', () => {
    expect(code).toMatch(/onPressedFace = press\.el\.contains\(target\)/);
    expect(code).toMatch(/onReplacement = !press\.el\.isConnected && !!target\.closest\(ROW_SELECTOR\)/);
    expect(code).toMatch(/if \(!onPressedFace && !onReplacement\) continue/);
  });

  it('hears a touchend nothing else let through', () => {
    const arm = code.match(/addEventListener\('touchend',[\s\S]*?\}, \{([^}]*)\}\)/);
    expect(arm).not.toBeNull();
    expect(arm?.[1]).toContain('capture: true');
  });

  it('asks who took the press instead of reading defaultPrevented', () => {
    expect(code).toContain('takePressOutcome');
    expect(code).not.toContain('defaultPrevented');
  });

  it('takes the outcome a task after the press\u2019s OWN lift', () => {
    expect(code).toContain('takePressOutcome(Date.now() - lifted.armedAt)');
    expect(code).toMatch(/lifted\.outcomeTimer = setTimeout/);
    expect(code).toMatch(/clearTimeout\(press\.outcomeTimer\)/);
  });

  it('rules an armed press that never lifted, rather than dropping it', () => {
    expect(code).toContain('ruleArmedWithNoLift');
    expect(code).toContain('LIFT_DEADLINE_MS');
  });

  it('records a click that had no touch behind it', () => {
    expect(code).toContain('lastTouchStartAt');
    expect(code).toContain('isTouchDevice()');
  });

  it('carries the row and face boxes, so an offset is measured', () => {
    expect(code).toContain('rowRect');
    expect(code).toContain('faceRect');
  });

  it('watches the row by where it is, not by what has focus', () => {
    expect(code).not.toContain('activeElement');
    expect(code).toContain('watchableRow');
  });

  it('stands the reachability check down while an overlay is open', () => {
    expect(code).toContain(`'data-overlay-open'`);
  });

  it('stands it down under the cover a client refresh raises too', () => {
    expect(code).toContain(`'data-ui-blocked'`);
  });
});

describe('missVector: the signed miss, which a scalar cannot express', () => {
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
  const wedged = {
    hasCommitFace: true,
    msSinceKeyboardClose: 4000,
    msSinceInput: 40000,
  };

  it('names a silence that followed the keyboard going', () => {
    expect(shouldReportSilence(wedged)).toBe(true);
  });

  it('says nothing before the silence has lasted a tick', () => {
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 900 })).toBe(false);
  });

  it('measures from the LAST input, never from the close alone', () => {
    expect(shouldReportSilence({ ...wedged, msSinceInput: 500 })).toBe(false);
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 40000, msSinceInput: 4000 }))
      .toBe(true);
  });

  it('says nothing until a keyboard has actually closed', () => {
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: null })).toBe(false);
  });

  it('holds the silence against the close when the page took nothing at all', () => {
    expect(shouldReportSilence({ ...wedged, msSinceInput: null })).toBe(true);
    expect(shouldReportSilence({ ...wedged, msSinceKeyboardClose: 900, msSinceInput: null }))
      .toBe(false);
  });

  it('has nothing to report with no commit face, which is an idle phone', () => {
    expect(shouldReportSilence({ ...wedged, hasCommitFace: false })).toBe(false);
  });
});

describe('nudgeIsTooSoon: two triggers, one relayout', () => {
  it('takes the first nudge, since nothing has been spent', () => {
    expect(nudgeIsTooSoon(null, 1000)).toBe(false);
  });

  it('refuses a second in the same millisecond', () => {
    expect(nudgeIsTooSoon(1000, 1000)).toBe(true);
  });

  it('refuses one a few frames behind the first', () => {
    expect(nudgeIsTooSoon(1000, 1200)).toBe(true);
  });

  it('takes one a whole scheduled interval later, which is the repeat rate', () => {
    expect(nudgeIsTooSoon(1000, 4000)).toBe(false);
  });
});

describe('shouldNudgeUntouched: the recovery with no gesture behind it', () => {
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
    expect(shouldNudgeUntouched({ ...waiting, typedSinceComposerInput: false })).toBe(false);
  });

  it('never fires before the user has typed at all', () => {
    expect(shouldNudgeUntouched({
      ...waiting, typedSinceComposerInput: false, msSinceKeystroke: 0,
    })).toBe(false);
  });

  it('stops once the user has plainly put the phone down', () => {
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 59_000 })).toBe(true);
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 61_000 })).toBe(false);
    expect(shouldNudgeUntouched({ ...waiting, msSinceKeystroke: 600_000 })).toBe(false);
  });
});
