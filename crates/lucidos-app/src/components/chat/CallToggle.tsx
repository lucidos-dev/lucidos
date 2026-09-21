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
 * **Every phase is on the button, visibly.** A call has four non-idle phases
 * and two of them are WORK: the connect takes seconds, and the hang-up takes a
 * network round trip. Painting one binary on-state left both reading as a dead
 * press, and a caller reported exactly that. So the phase rides
 * `data-call-phase`, and the stylesheet gives each one a look of its own
 * (`chat/input-messages.css`).
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
import type { HeaderActionSpec } from '../layout/headerActions';
import type { OverflowMenuOpener } from '../shared/OverflowMenu';
import { useLongPress } from '../../hooks/useLongPress';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { callStatusLabel, isOnCall } from '../../voice/callState';
import type { CallPhase } from '../../voice/callState';
import { pressCallToggle, voiceCall } from '../../store/voice';
import { preferences } from '../../store/store';
import { voiceEnabled } from '../../store/actions/preferences';

/**
 * What the button is CALLED in each phase, which is what a press DOES.
 *
 * Never what the call is doing. The status region below says that, and a name
 * repeating it would have a screen reader hear one state twice. `ending` is the
 * one exception, because there is no press left to name.
 */
const PRESS_NAME: Record<CallPhase, string> = {
  idle: 'Start a call',
  connecting: 'Cancel the call',
  listening: 'End the call',
  speaking: 'End the call',
  ending: 'Ending the call',
};

/** What the tooltip says. It is the only copy a sighted reader gets, so it
 *  names the phase as well as the press. */
const TOOLTIP: Record<CallPhase, string> = {
  idle: 'Start a call and talk to Lucidos',
  connecting: 'Connecting. Press to cancel',
  listening: 'On a call. Press to end it',
  speaking: 'Lucidos is speaking. Press to end the call',
  ending: 'Ending the call',
};

/**
 * How long a connect may run before the control stops claiming progress.
 *
 * A first call after a page load pays the browser's microphone prompt, and the
 * sheet sits over our UI while a human reads it. A pulse through that says we
 * are busy when we are waiting on THEM. The grant then holds for the life of
 * the page, so a second call never reaches this dwell and reads as it always
 * did.
 */
const CONNECT_DWELL_MS = 2_000;

/**
 * What a dwelling connect says instead.
 *
 * The tooltip only, and never the status region. That region narrates the
 * phase, which really is still `connecting`. This is a guess about what the
 * wait is, and a guess does not belong in a live region. The button keeps its
 * name too: a name says what a press does, and this says nothing about that.
 */
const WAITING_TOOLTIP = 'Waiting for microphone access';

export function CallToggle({ available = true, attrs }: { available?: boolean; attrs?: Record<string, string> }) {
  // Subscribe to the preference signal.
  preferences.value;
  const call = voiceCall.value;
  const phase = call.phase;
  const on = isOnCall(phase);
  // A press while `ending` does nothing already: `ringOff` returns the state
  // unchanged. So the control says so, and the press that does nothing also
  // LOOKS like it does nothing.
  //
  // `aria-disabled` rather than the attribute, deliberately. A real one takes
  // the pointer events with it. The hold below picks the microphone the NEXT
  // call opens, and this call ending settles nothing about that.
  const dead = phase === 'ending';
  const connecting = phase === 'connecting';
  // The phase is re-checked, because the flag clears in an effect and this
  // render already knows the call has moved on.
  const waiting = useDelayedFlag(connecting, CONNECT_DWELL_MS) && connecting;
  const openRef = useRef<OverflowMenuOpener | null>(null);
  // The hold's own paired click is swallowed by the gesture, so opening the
  // picker never also places a call. The devices are read by the menu's own
  // body as it mounts, which is the one place that knows it is on screen.
  const press = useLongPress(
    (button) => openRef.current?.(button),
    () => {
      if (!dead) pressCallToggle();
    },
  );
  // A call already up survives either reason arriving mid-call, so the reader
  // never loses the control they ring off with. The switch can be turned off
  // and the destination can move; both leave the button where it was.
  if ((!voiceEnabled() || !available) && !on) return null;
  return (
    <>
      <button
        {...attrs}
        class={`icon-btn header-icon${on ? ' active' : ''}`}
        data-role="call-toggle"
        data-call-phase={phase}
        data-call-wait={waiting ? 'microphone' : undefined}
        aria-pressed={on}
        aria-disabled={dead}
        aria-label={PRESS_NAME[phase]}
        data-tooltip={waiting ? WAITING_TOOLTIP : TOOLTIP[phase]}
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

/** The call toggle as a foldable member, or `null` when it would draw nothing.
 *
 *  The factory lives here rather than in the composer because deciding whether
 *  the control exists reads the call. Nothing in the composer may
 *  (`__tests__/composer-live-during-a-call.test.ts`), and this file already
 *  owns that state.
 *
 *  It folds LAST with the follow toggle, so its slot holds at every width that
 *  can show it. A hang-up one tap deeper is the price of a row that fits. The
 *  row is never too narrow to hold the ⋯ the hang-up moved into. */
export function callToggleAction(available: boolean): HeaderActionSpec | null {
  preferences.value;
  const call = voiceCall.value;
  const on = isOnCall(call.phase);
  if ((!voiceEnabled() || !available) && !on) return null;
  return {
    key: 'call-toggle',
    dataRole: 'call-toggle',
    label: PRESS_NAME[call.phase],
    tooltip: TOOLTIP[call.phase],
    icon: () => <CallIcon />,
    // The on-call state, which the row paints and a menu row cannot. Folded,
    // `aria-checked` is the only channel left saying a call is up, and that is
    // exactly when the hang-up is one tap deeper.
    active: on,
    render: (attrs) => <CallToggle available={available} attrs={attrs} />,
    onClick: () => pressCallToggle(),
  };
}
