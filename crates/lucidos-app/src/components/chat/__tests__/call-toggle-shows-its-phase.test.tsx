// @vitest-environment jsdom
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/voice', () => ({ pressCallToggle: vi.fn() }));

import { CallToggle, MicrophoneList } from '../CallToggle';
import { preferences } from '../../../store/store';
import { pressCallToggle } from '../../../store/voice';
import { voiceCall } from '../../../store/voiceCall';
import { microphones } from '../../../store/microphones';
import { CALL_IDLE, callStatusLabel } from '../../../voice/callState';
import { tooltipWithShortcut } from '../../../store/actions/keybindings';
import type { CallPhase, CallState } from '../../../voice/callState';

beforeAll(() => MicrophoneList.preload());

const PHASES: CallPhase[] = ['idle', 'connecting', 'listening', 'speaking', 'ending'];

let host: HTMLDivElement;

function mount(phase: CallPhase, extra: Partial<CallState> = {}): void {
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
  voiceCall.value = { ...CALL_IDLE, phase, threadId: 't', ...extra };
  act(() => {
    render(<CallToggle />, host);
  });
}

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

  it('gives every phase a look of its own', () => {
    const looks = PHASES.map((phase) => {
      mount(phase);
      return look();
    });
    expect(new Set(looks).size, looks.join('\n')).toBe(PHASES.length);
  });

  it('stays pressed for every phase a call exists in', () => {
    mount('idle');
    expect(control().getAttribute('aria-pressed')).toBe('false');
    for (const phase of PHASES.filter((p) => p !== 'idle')) {
      mount(phase);
      expect(control().getAttribute('aria-pressed'), phase).toBe('true');
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

describe('a connect that dwells says what it is waiting for', () => {
  const DWELL_MS = 2_000;
  const CONNECTING_TIP = tooltipWithShortcut('Connecting. Press to cancel', 'toggleCall');
  const WAITING_TIP = 'Waiting for microphone access';

  function tip(): string | null {
    return control().getAttribute('data-tooltip');
  }

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

  it('leaves the hidden region alone throughout', () => {
    mount('connecting');
    expect(region().textContent).toBe('Connecting');
    tick(DWELL_MS * 3);
    expect(region().textContent).toBe('Connecting');
  });

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
    expect(tip()).toBe(tooltipWithShortcut('On a call. Press to end it', 'toggleCall'));
  });

  it('never dwells in a phase that is not connecting', () => {
    for (const phase of PHASES.filter((p) => p !== 'connecting')) {
      mount(phase);
      tick(DWELL_MS * 3);
      expect(control().hasAttribute('data-call-wait'), phase).toBe(false);
    }
  });
});
