// @vitest-environment jsdom
/**
 * A call has four non-idle phases, and the button must show which one it is in.
 *
 * Two of them are WORK: the connect takes seconds and the hang-up takes a round
 * trip. Both used to paint the same red as a live call, so the press that
 * mattered most read as a dead button. A caller reported exactly that.
 *
 * Rendered rather than poked through props, because the claim is about what a
 * reader sees. The stylesheet's half of it is scanned separately, in
 * `styles/__tests__/call-toggle-phase-paint.test.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/voice', async () => {
  const { signal } = await import('@preact/signals');
  const { CALL_IDLE } = await import('../../../voice/callState');
  return { voiceCall: signal(CALL_IDLE), pressCallToggle: vi.fn() };
});

import { CallToggle } from '../CallToggle';
import { preferences } from '../../../store/store';
import { pressCallToggle, voiceCall } from '../../../store/voice';
import { microphones } from '../../../store/microphones';
import { CALL_IDLE, callStatusLabel } from '../../../voice/callState';
import type { CallPhase, CallState } from '../../../voice/callState';

const PHASES: CallPhase[] = ['idle', 'connecting', 'listening', 'speaking', 'ending'];

let host: HTMLDivElement;

/** Rendered inside `act`, so the dwell timer is armed by the time a test
 *  advances the clock. */
function mount(phase: CallPhase, extra: Partial<CallState> = {}): void {
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
  voiceCall.value = { ...CALL_IDLE, phase, threadId: 't', ...extra };
  act(() => {
    render(<CallToggle />, host);
  });
}

/** Move the call on without remounting, so the dwell's own effect decides what
 *  happens to the flag. */
function moveTo(phase: CallPhase): void {
  act(() => {
    voiceCall.value = { ...voiceCall.value, phase };
  });
}

function tick(ms: number): void {
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

function control(): HTMLElement {
  const el = host.querySelector<HTMLElement>('[data-role="call-toggle"]');
  expect(el, 'no call toggle rendered').not.toBeNull();
  return el as HTMLElement;
}

function region(): HTMLElement {
  const el = host.querySelector<HTMLElement>('[data-role="call-state"]');
  expect(el, 'no status region rendered').not.toBeNull();
  return el as HTMLElement;
}

/**
 * What the control says about itself, minus the phase attribute.
 *
 * Dropped on purpose: it is the hook the stylesheet paints through, so leaving
 * it in would make every phase distinct by construction and prove nothing.
 */
function look(): string {
  const el = control();
  return [
    el.className,
    el.getAttribute('aria-disabled'),
    el.getAttribute('aria-label'),
    el.getAttribute('data-tooltip'),
  ].join(' | ');
}

function pointer(type: string): PointerEvent {
  // jsdom has no PointerEvent constructor, so a MouseEvent carrying the same
  // fields stands in. `useLongPress` reads only `button` and `clientX/Y`.
  return new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: 0,
    clientY: 0,
  }) as unknown as PointerEvent;
}

async function flush(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

async function tap(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

async function hold(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  vi.advanceTimersByTime(500);
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(pressCallToggle).mockReset();
  microphones.value = { status: 'loaded', data: [] };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  microphones.value = { status: 'not-loaded' };
  preferences.value = { status: 'not-loaded' };
  voiceCall.value = CALL_IDLE;
  vi.useRealTimers();
});

describe('the call control shows which phase it is in', () => {
  it('carries the phase on the button, where the stylesheet can reach it', () => {
    for (const phase of PHASES) {
      mount(phase);
      expect(control().dataset.callPhase, phase).toBe(phase);
    }
  });

  /** The whole defect. Four phases painted one binary, so a hang-up looked
   *  like nothing at all until the socket closed. */
  it('gives every phase a look of its own', () => {
    const looks = PHASES.map((phase) => {
      mount(phase);
      return look();
    });
    expect(new Set(looks).size, looks.join('\n')).toBe(PHASES.length);
  });

  it('keeps the on-state class for every phase a call exists in', () => {
    mount('idle');
    expect(control().className).not.toContain('active');
    for (const phase of PHASES.filter((p) => p !== 'idle')) {
      mount(phase);
      expect(control().className, phase).toContain('active');
    }
  });
});

describe('the name and the tooltip track the phase', () => {
  const NAMES: Record<CallPhase, string> = {
    idle: 'Start a call',
    connecting: 'Cancel the call',
    listening: 'End the call',
    speaking: 'End the call',
    ending: 'Ending the call',
  };

  for (const phase of PHASES) {
    it(`names the press for ${phase}`, () => {
      mount(phase);
      expect(control().getAttribute('aria-label')).toBe(NAMES[phase]);
    });
  }

  /** "End the call" on a button that is already ending is a lie, and it was
   *  the label both transitional phases wore. */
  it('never offers to end a call that is already ending', () => {
    mount('ending');
    expect(control().getAttribute('aria-label')).not.toBe('End the call');
    expect(control().getAttribute('data-tooltip')).not.toBe('End the call');
  });

  it('gives every phase its own tooltip, and names the floor in a live one', () => {
    const tips = PHASES.map((phase) => {
      mount(phase);
      return control().getAttribute('data-tooltip') ?? '';
    });
    expect(new Set(tips).size, tips.join('\n')).toBe(PHASES.length);
    mount('speaking');
    expect(control().getAttribute('data-tooltip')).toContain('Lucidos');
  });
});

describe('the ending control is dead, and looks it', () => {
  it('announces itself unavailable while ending, and never otherwise', () => {
    for (const phase of PHASES) {
      mount(phase);
      expect(control().getAttribute('aria-disabled'), phase).toBe(String(phase === 'ending'));
    }
  });

  it('places no press while ending', async () => {
    mount('ending');
    await tap(control());
    expect(pressCallToggle).not.toHaveBeenCalled();
  });

  it('still takes a press in every other phase', async () => {
    for (const phase of PHASES.filter((p) => p !== 'ending')) {
      vi.mocked(pressCallToggle).mockReset();
      mount(phase);
      await tap(control());
      expect(pressCallToggle, phase).toHaveBeenCalledTimes(1);
    }
  });

  /** The hold settles the microphone the NEXT call opens, which this call
   *  ending decides nothing about. So it survives the dead press. */
  it('still opens the microphone picker on a hold', async () => {
    mount('ending');
    await hold(control());
    expect(document.querySelector('.thread-overflow-menu')).not.toBeNull();
    expect(pressCallToggle).not.toHaveBeenCalled();
  });
});

describe('the hidden region is left exactly as it was', () => {
  it('says what callStatusLabel says, in every phase', () => {
    for (const phase of PHASES) {
      mount(phase);
      expect(region().textContent, phase).toBe(callStatusLabel(voiceCall.value));
    }
  });

  /** A live utterance still wins over the phase, which is the region's own
   *  rule and not this component's to restate. */
  it('lets a live utterance speak over the phase, as before', () => {
    mount('listening', { utterance: 'live', utteranceCount: 1 });
    expect(region().textContent).toBe(callStatusLabel(voiceCall.value));
    expect(region().textContent).toBe('Hearing you');
  });

  it('stays out of the tab order and out of sight', () => {
    mount('connecting');
    expect(region().className).toContain('visually-hidden');
    expect(region().getAttribute('role')).toBe('status');
  });
});

/**
 * On iOS a first call after a page load pays the browser's microphone prompt,
 * and a human reads it behind a sheet covering our UI. A spinner through that
 * tells the reader we are busy when we are waiting on them.
 */
describe('a connect that dwells says what it is waiting for', () => {
  /** The component's own threshold. Restated rather than exported: the number
   *  is a product decision, and a test reading it from the source proves only
   *  that the source equals itself. */
  const DWELL_MS = 2_000;
  const CONNECTING_TIP = 'Connecting. Press to cancel';
  const WAITING_TIP = 'Waiting for microphone access';

  function tip(): string | null {
    return control().getAttribute('data-tooltip');
  }

  /** Both sides of the threshold on one mount, so the last millisecond is what
   *  flips the copy. Two separate mounts would pass with the timer armed at the
   *  wrong moment, or never armed at all. */
  it('claims progress until the dwell is up, then names the microphone', () => {
    mount('connecting');
    expect(tip()).toBe(CONNECTING_TIP);
    tick(DWELL_MS - 1);
    expect(tip(), 'the copy changed early').toBe(CONNECTING_TIP);
    expect(control().hasAttribute('data-call-wait')).toBe(false);
    tick(1);
    expect(tip(), 'the copy never changed').toBe(WAITING_TIP);
    expect(control().dataset.callWait).toBe('microphone');
  });

  it('says it once, and says nothing further', () => {
    mount('connecting');
    tick(DWELL_MS);
    expect(tip()).toBe(WAITING_TIP);
    tick(DWELL_MS * 5);
    expect(tip()).toBe(WAITING_TIP);
  });

  /** The region narrates the phase, and the phase never moved. A second
   *  announcement here would be the timer talking, not the call. */
  it('leaves the hidden region alone throughout', () => {
    mount('connecting');
    expect(region().textContent).toBe('Connecting');
    tick(DWELL_MS * 3);
    expect(region().textContent).toBe('Connecting');
  });

  /** A name says what a press does, and the press still cancels. */
  it('leaves the name of the button alone throughout', () => {
    mount('connecting');
    tick(DWELL_MS * 3);
    expect(control().getAttribute('aria-label')).toBe('Cancel the call');
  });

  it('drops the wait the moment the call goes live', () => {
    mount('connecting');
    tick(DWELL_MS);
    expect(control().dataset.callWait).toBe('microphone');
    moveTo('listening');
    expect(control().hasAttribute('data-call-wait')).toBe(false);
    expect(tip()).toBe('On a call. Press to end it');
  });

  it('never dwells in a phase that is not connecting', () => {
    for (const phase of PHASES.filter((p) => p !== 'connecting')) {
      mount(phase);
      tick(DWELL_MS * 3);
      expect(control().hasAttribute('data-call-wait'), phase).toBe(false);
    }
  });
});
