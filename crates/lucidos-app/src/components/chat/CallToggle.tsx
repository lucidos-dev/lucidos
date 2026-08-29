/**
 * The one control a call has: press to place it, press again to ring off.
 *
 * A toggle rather than a microphone, because the microphone is open for the
 * whole call. Holding a button down is not how a call works, and a second
 * control for hanging up would be a second thing to find.
 *
 * It renders in every prompt input, the compose view included. So a call
 * starts a thread by exactly the path typing uses (the parent plan's decision
 * 2). Nothing here is backend-specific.
 *
 * **Nothing at all until the workspace turns voice on.** Voice is experimental
 * and ships off, so the control is absent rather than inert: a dead button is a
 * thing to wonder about, and `/api/v1/voice` refuses the socket anyway.
 */
import { CallIcon } from '../shared/icons';
import { isOnCall } from '../../voice/callState';
import { pressCallToggle, voiceCall } from '../../store/voice';
import { preferences } from '../../store/store';
import { voiceEnabled } from '../../store/actions/preferences';

export function CallToggle() {
  // Subscribe to the preference signal.
  preferences.value;
  const phase = voiceCall.value.phase;
  const on = isOnCall(phase);
  // A call already up survives the switch being turned off mid-call, so the
  // reader keeps the control they need to ring off with.
  if (!voiceEnabled() && !on) return null;
  return (
    <button
      class={`icon-btn header-icon${on ? ' active' : ''}`}
      data-role="call-toggle"
      data-row-item
      aria-pressed={on}
      aria-label={on ? 'End the call' : 'Start a call'}
      data-tooltip={on ? 'End the call' : 'Start a call and talk to Lucidos'}
      onClick={pressCallToggle}
    >
      <CallIcon />
    </button>
  );
}
