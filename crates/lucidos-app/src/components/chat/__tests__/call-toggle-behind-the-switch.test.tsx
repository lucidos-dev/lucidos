// @vitest-environment jsdom
/**
 * Voice is experimental and ships off, so the composer offers no way in until
 * the workspace turns it on.
 *
 * The engine refuses the socket too, and that refusal is the boundary. This is
 * the half that keeps a control nobody can use off the screen.
 *
 * Rendered rather than called, because the toggle holds hooks now: it owns the
 * long-press gesture and the handle its microphone picker is opened through.
 */
import { describe, it, expect, afterEach, beforeEach } from 'vitest';
import { render } from 'preact';
import { CallToggle } from '../CallToggle';
import { preferences } from '../../../store/store';
import { voiceCall } from '../../../store/voice';
import { CALL_IDLE } from '../../../voice/callState';

let host: HTMLDivElement;

function toggle(available = true): HTMLElement | null {
  render(<CallToggle available={available} />, host);
  return host.querySelector<HTMLElement>('[data-role="call-toggle"]');
}

/** Voice on, so the tests below are about the destination and nothing else. */
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
});

describe('the call toggle behind the voice switch', () => {
  it('renders nothing while voice is off', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(toggle()).toBeNull();
  });

  /** The gap before the answer arrives reads as off. So the control appears
   *  once we know it should, rather than blinking in on every load. */
  it('renders nothing while the preferences are still loading', () => {
    preferences.value = { status: 'loading' };
    expect(toggle()).toBeNull();
  });

  it('renders the control once voice is on', () => {
    preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
    expect(toggle()).not.toBeNull();
  });

  /** A switch flipped mid-call must not take away the only way to ring off.
   *  The engine's own refusal never reaches a socket that is already up. */
  it('keeps the control while a call is up, whatever the switch says', () => {
    preferences.value = { status: 'loaded', data: {} };
    voiceCall.value = { ...CALL_IDLE, phase: 'listening', threadId: 't' };
    expect(toggle()).not.toBeNull();
  });
});

describe('the call toggle behind the destination', () => {
  /** A call reaches the Lucidos Agent and nothing else (ADR 0165). Absent
   *  rather than inert, matching the switch above: the way back is the
   *  destination picker, which sits in this same row. */
  it('renders nothing when a coding agent holds the destination', () => {
    voiceIsOn();
    expect(toggle(false)).toBeNull();
  });

  it('renders the control for the Lucidos Agent', () => {
    voiceIsOn();
    expect(toggle(true)).not.toBeNull();
  });

  /** Both reasons share the live-call exemption, so a destination moved
   *  mid-call leaves the caller the button they ring off with. */
  it('keeps the control while a call is up, whatever the destination says', () => {
    voiceIsOn();
    voiceCall.value = { ...CALL_IDLE, phase: 'listening', threadId: 't' };
    expect(toggle(false)).not.toBeNull();
  });
});
