// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { CallToggle } from '../CallToggle';
import { preferences } from '../../../store/store';
import { voiceCall } from '../../../store/voiceCall';
import { CALL_IDLE } from '../../../voice/callState';

const reach = vi.hoisted(() => ({ reachable: true }));
vi.mock('../../../voice/socketUrl', async (original) => ({
  ...(await original<typeof import('../../../voice/socketUrl')>()),
  voiceReachable: () => reach.reachable,
}));

let host: HTMLDivElement;

function toggle(available = true): HTMLElement | null {
  render(<CallToggle available={available} />, host);
  return host.querySelector<HTMLElement>('[data-role="call-toggle"]');
}

function voiceIsOn(): void {
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  preferences.value = { status: 'not-loaded' };
  voiceCall.value = CALL_IDLE;
  reach.reachable = true;
});

describe('the call toggle behind the voice switch', () => {
  it('renders nothing while voice is off', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(toggle()).toBeNull();
  });

  it('renders nothing while the preferences are still loading', () => {
    preferences.value = { status: 'loading' };
    expect(toggle()).toBeNull();
  });

  it('renders the control once voice is on', () => {
    preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
    expect(toggle()).not.toBeNull();
  });

  it('keeps the control while a call is up, whatever the switch says', () => {
    preferences.value = { status: 'loaded', data: {} };
    voiceCall.value = { ...CALL_IDLE, phase: 'listening', threadId: 't' };
    expect(toggle()).not.toBeNull();
  });
});

describe('the call toggle behind the destination', () => {
  it('renders nothing when a coding agent holds the destination', () => {
    voiceIsOn();
    expect(toggle(false)).toBeNull();
  });

  it('renders the control for the Lucidos Agent', () => {
    voiceIsOn();
    expect(toggle(true)).not.toBeNull();
  });

  it('keeps the control while a call is up, whatever the destination says', () => {
    voiceIsOn();
    voiceCall.value = { ...CALL_IDLE, phase: 'listening', threadId: 't' };
    expect(toggle(false)).not.toBeNull();
  });
});

describe('the call toggle in a workspace opened through another gateway', () => {
  it('renders nothing, voice on or not', () => {
    voiceIsOn();
    reach.reachable = false;
    expect(toggle()).toBeNull();
  });
});
