/**
 * The call's one control: press to place a call, press again to ring off.
 *
 * A toggle rather than a push-to-talk button, because the microphone is open
 * for the whole call. A second control for hanging up would be a second thing
 * to find, and so would a picker of its own. So the tap is the call, and the
 * HOLD picks which microphone the next one opens, through `MicrophonePicker`.
 *
 * It renders in every prompt input, the compose view included. So a call
 * starts a thread by exactly the path typing uses (the parent plan's decision
 * 2). Which BACKEND it may run on is the caller's answer, not this file's:
 * `available` carries it, and nothing here names an agent.
 *
 * **Nothing at all until the workspace turns voice on.** Voice is experimental
 * and ships off, so the control is absent rather than inert: a dead button is a
 * thing to wonder about, and `/api/v1/voice` refuses the socket anyway.
 *
 * **Nor when the destination is a coding agent** (ADR 0165). Same reasoning,
 * and the way back is already on screen: the destination picker sits in this
 * row. `available` carries that, so one gate decides and the live-call
 * exemption below covers both reasons at once.
 *
 * It also carries the call's one `status` region. A screen reader hears a call
 * connect, hears it go live, and hears it pick the caller's voice up. It is
 * the only announcer a call has: a second one would talk over this.
 *
 * Ringing off is the BUTTON's announcement, not the region's. Its pressed
 * state and its label both flip, where emptying a live region says nothing at
 * all. So the region speaks each state a call arrives at, and never its end.
 */
import { useRef } from 'preact/hooks';
import { CallIcon } from '../shared/icons';
import { MicrophonePicker } from './MicrophonePicker';
import type { OverflowMenuOpener } from '../shared/OverflowMenu';
import { useLongPress } from '../../hooks/useLongPress';
import { callStatusLabel, isOnCall } from '../../voice/callState';
import { pressCallToggle, voiceCall } from '../../store/voice';
import { preferences } from '../../store/store';
import { voiceEnabled } from '../../store/actions/preferences';

export function CallToggle({ available = true }: { available?: boolean }) {
  // Subscribe to the preference signal.
  preferences.value;
  const call = voiceCall.value;
  const on = isOnCall(call.phase);
  const openRef = useRef<OverflowMenuOpener | null>(null);
  // The hold's own paired click is swallowed by the gesture, so opening the
  // picker never also places a call. The devices are read by the menu's own
  // body as it mounts, which is the one place that knows it is on screen.
  const press = useLongPress((button) => openRef.current?.(button), pressCallToggle);
  // A call already up survives either reason arriving mid-call, so the reader
  // never loses the control they ring off with. The switch can be turned off
  // and the destination can move; both leave the button where it was.
  if ((!voiceEnabled() || !available) && !on) return null;
  return (
    <>
      <button
        class={`icon-btn header-icon${on ? ' active' : ''}`}
        data-role="call-toggle"
        data-row-item
        aria-pressed={on}
        aria-label={on ? 'End the call' : 'Start a call'}
        data-tooltip={on ? 'End the call' : 'Start a call and talk to Lucidos'}
        onPointerDown={press.onPointerDown}
        onPointerMove={press.onPointerMove}
        onPointerUp={press.onPointerUp}
        onPointerLeave={press.onPointerLeave}
        onPointerCancel={press.onPointerCancel}
        onContextMenu={press.onContextMenu}
        onClick={press.onClick}
      >
        <CallIcon />
      </button>
      {/* Out of flow, so it takes no room in the prompt row and is no item of
          it. Empty while idle, so the first state a call reaches is an
          announcement rather than a change nobody heard the start of. */}
      <span class="visually-hidden" role="status" data-role="call-state">
        {callStatusLabel(call)}
      </span>
      <MicrophonePicker openRef={openRef} />
    </>
  );
}
