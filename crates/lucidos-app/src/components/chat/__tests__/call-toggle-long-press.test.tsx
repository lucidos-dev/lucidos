// @vitest-environment jsdom
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/voice', () => ({ pressCallToggle: vi.fn() }));

import { CallToggle, MicrophoneList } from '../CallToggle';
import { preferences } from '../../../store/store';
import { pressCallToggle } from '../../../store/voice';
import { microphones } from '../../../store/microphones';
import { SYSTEM_DEFAULT_LABEL } from '../../../voice/microphone';

beforeAll(() => MicrophoneList.preload());

let host: HTMLDivElement;

function input(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, label, kind: 'audioinput', groupId: 'g' } as MediaDeviceInfo;
}

function mount(): void {
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
  render(<CallToggle />, host);
}

function control(): HTMLElement {
  const el = host.querySelector<HTMLElement>('[data-role="call-toggle"]');
  expect(el, 'no call toggle rendered').not.toBeNull();
  return el as HTMLElement;
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.thread-overflow-menu');
}

function rows(): string[] {
  return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitemradio"]')]
    .map((el) => el.textContent?.replace('✓', '').trim() ?? '');
}

function pointer(type: string, init: { clientX?: number; clientY?: number } = {}): PointerEvent {
  return new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: init.clientX ?? 0,
    clientY: init.clientY ?? 0,
  }) as unknown as PointerEvent;
}

async function flush(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

async function hold(target: HTMLElement, moveTo?: { x: number; y: number }): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  if (moveTo) target.dispatchEvent(pointer('pointermove', { clientX: moveTo.x, clientY: moveTo.y }));
  vi.advanceTimersByTime(500);
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

async function tap(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(pressCallToggle).mockReset();
  microphones.value = {
    status: 'loaded',
    data: [input('mic-a', 'MacBook Pro Microphone'), input('mic-b', 'Headset')],
  };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  microphones.value = { status: 'not-loaded' };
  preferences.value = { status: 'not-loaded' };
  vi.useRealTimers();
});

describe('the call control tells its two gestures apart', () => {
  it('places the call on a tap, and opens nothing', async () => {
    mount();
    await tap(control());
    expect(pressCallToggle).toHaveBeenCalledTimes(1);
    expect(menu()).toBeNull();
  });

  it('opens the picker on a hold, and places no call', async () => {
    mount();
    await hold(control());
    expect(menu()).not.toBeNull();
    expect(pressCallToggle).not.toHaveBeenCalled();
  });

  it('treats a hold that travels as a tap', async () => {
    mount();
    await hold(control(), { x: 60, y: 0 });
    expect(menu()).toBeNull();
    expect(pressCallToggle).toHaveBeenCalledTimes(1);
  });
});

describe('what the picker offers', () => {
  it('leads with the system default and names every microphone', async () => {
    mount();
    await hold(control());
    expect(rows()).toEqual([SYSTEM_DEFAULT_LABEL, 'MacBook Pro Microphone', 'Headset']);
  });

  it('checks the stored choice, and only that one', async () => {
    preferences.value = {
      status: 'loaded',
      data: { voice_enabled: 'true', voice_input_device: 'mic-b' },
    };
    render(<CallToggle />, host);
    await hold(control());
    const checked = [
      ...document.querySelectorAll<HTMLElement>('[role="menuitemradio"][aria-checked="true"]'),
    ];
    expect(checked).toHaveLength(1);
    expect(checked[0].dataset.value).toBe('mic-b');
  });

  it('offers the permission row while the browser will name nothing', async () => {
    microphones.value = { status: 'loaded', data: [input('mic-a', ''), input('mic-b', '')] };
    mount();
    await hold(control());
    expect(rows()).toEqual([SYSTEM_DEFAULT_LABEL]);
    expect(menu()?.textContent).toContain('Allow Lucidos to see your microphone names');
  });
});
