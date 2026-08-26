import { describe, it, expect } from 'vitest';
import { landingReport, pressIsWatchable, silentPressReport, type LandingFacts, type ProbeViewport } from '../deadPressProbe';

// The probe exists because the dead-Send report has arrived three times and
// says only "nothing happened". Its whole value is that the NEXT report names
// which half of the stack failed. So these cases pin the two verdicts apart,
// and pin the silence on every healthy tap.

const VIEWPORT: ProbeViewport = {
  vvHeight: 460,
  vvOffsetTop: 0,
  innerHeight: 844,
  appHeight: '460px',
};

/** The Send morph as it sits with the keyboard up: a small circle low in the
 *  shrunken viewport. */
const SEND_RECT = { left: 330, right: 359, top: 420, bottom: 449 };

function facts(over: Partial<LandingFacts> = {}): LandingFacts {
  return {
    point: { x: 344, y: 434 },
    sendRect: SEND_RECT,
    targetIsSend: true,
    elementAtPoint: 'button.action-btn',
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
    expect(landingReport(facts({ point: { x: 40, y: 120 }, targetIsSend: false }))).toBeNull();
  });

  it('says nothing when the button is not on screen', () => {
    expect(landingReport(facts({ sendRect: null, targetIsSend: false }))).toBeNull();
  });

  it('reports the press that was on the button but went elsewhere', () => {
    // The hit-test family: the browser is testing against a layout it is no
    // longer painting, so the pixels under the finger are not what answered.
    const report = landingReport(facts({
      targetIsSend: false,
      elementAtPoint: 'div.thread-content',
    }));
    expect(report).toContain('the tap was on the button');
    expect(report).toContain('div.thread-content');
  });

  it('carries the touch and the button centre, so the offset is readable', () => {
    const report = landingReport(facts({ targetIsSend: false, point: { x: 344, y: 425 } }));
    expect(report).toContain('centre y 435');
    expect(report).toContain('touch y 425');
  });

  it('carries the four viewport numbers a layout fault shows up in', () => {
    const report = landingReport(facts({
      targetIsSend: false,
      viewport: { vvHeight: 460, vvOffsetTop: 87, innerHeight: 844, appHeight: '744px' },
    }));
    expect(report).toContain('vv 460 +87');
    expect(report).toContain('inner 844');
    expect(report).toContain('app 744px');
  });

  it('names the absence when nothing at all sits under the finger', () => {
    const report = landingReport(facts({ targetIsSend: false, elementAtPoint: null }));
    expect(report).toContain('nothing');
  });
});

// A diagnostic that cries wolf gets ignored. The morph button is one node
// across four modes. Three of them drop a press on purpose, so watching those
// would toast during ordinary use.
describe('pressIsWatchable: only the constructive Send is watched', () => {
  const SEND = { disabled: false, placeholder: false, ariaLabel: 'Send message' };

  it('watches the enabled Send', () => {
    expect(pressIsWatchable(SEND)).toBe(true);
  });

  it('ignores the invisible placeholder that holds the row height', () => {
    expect(pressIsWatchable({ ...SEND, placeholder: true })).toBe(false);
  });

  it('ignores a disabled face, which is the settling Stop', () => {
    expect(pressIsWatchable({ ...SEND, disabled: true })).toBe(false);
  });

  it('ignores the destructive face, which has no touch path to fail', () => {
    // A press sliding off Cancel is a deliberate abort, and it produces no
    // click. That looks exactly like the fault being chased.
    expect(pressIsWatchable({ ...SEND, ariaLabel: 'Cancel' })).toBe(false);
  });

  it('ignores a face with no label at all', () => {
    expect(pressIsWatchable({ ...SEND, ariaLabel: null })).toBe(false);
  });
});

describe('silentPressReport: the press arrived and no path took it', () => {
  it('says so, and carries the same viewport numbers', () => {
    const report = silentPressReport(VIEWPORT);
    expect(report).toContain('reached the button and nothing ran');
    expect(report).toContain('vv 460 +0');
    expect(report).toContain('app 460px');
  });

  it('survives a viewport that never published --app-height', () => {
    // Desktop, or a boot before MobileSwipeContainer's first write. An empty
    // string in the middle of the line reads as a truncated report.
    const report = silentPressReport({ ...VIEWPORT, appHeight: '' });
    expect(report).toContain('app unset');
  });
});
