/**
 * Voice is experimental and ships off, so the composer offers no way in until
 * the workspace turns it on.
 *
 * The engine refuses the socket too, and that refusal is the boundary. This is
 * the half that keeps a control nobody can use off the screen.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { CallToggle } from '../CallToggle';
import { preferences } from '../../../store/store';
import { voiceCall } from '../../../store/voice';
import { CALL_IDLE } from '../../../voice/callState';

afterEach(() => {
  preferences.value = { status: 'not-loaded' };
  voiceCall.value = CALL_IDLE;
});

describe('the call toggle behind the voice switch', () => {
  it('renders nothing while voice is off', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(CallToggle()).toBeNull();
  });

  /** The gap before the answer arrives reads as off. So the control appears
   *  once we know it should, rather than blinking in on every load. */
  it('renders nothing while the preferences are still loading', () => {
    preferences.value = { status: 'loading' };
    expect(CallToggle()).toBeNull();
  });

  it('renders the control once voice is on', () => {
    preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
    expect(CallToggle()).not.toBeNull();
  });

  /** A switch flipped mid-call must not take away the only way to ring off.
   *  The engine's own refusal never reaches a socket that is already up. */
  it('keeps the control while a call is up, whatever the switch says', () => {
    preferences.value = { status: 'loaded', data: {} };
    voiceCall.value = { ...CALL_IDLE, phase: 'listening', threadId: 't' };
    expect(CallToggle()).not.toBeNull();
  });
});
