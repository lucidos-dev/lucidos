import { useRef } from 'preact/hooks';
import { CallIcon } from '../shared/icons';
import { OverflowMenu } from '../shared/OverflowMenu';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import type { HeaderActionSpec } from '../layout/headerActions';
import type { OverflowMenuOpener } from '../shared/OverflowMenu';
import { useLongPress } from '../../hooks/useLongPress';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { callStatusLabel, isOnCall } from '../../voice/callState';
import type { CallPhase } from '../../voice/callState';
import { pressCallToggle } from '../../store/voice';
import { voiceCall } from '../../store/voiceCall';
import { preferences } from '../../store/store';
import { voiceEnabled } from '../../store/actions/preferences';
import { tooltipWithShortcut } from '../../store/actions/keybindings';

const PRESS_NAME: Record<CallPhase, string> = {
  idle: 'Start a call',
  connecting: 'Cancel the call',
  listening: 'End the call',
  speaking: 'End the call',
  ending: 'Ending the call',
};

const TOOLTIP: Record<CallPhase, string> = {
  idle: 'Start a call and talk to Lucidos',
  connecting: 'Connecting. Press to cancel',
  listening: 'On a call. Press to end it',
  speaking: 'Lucidos is speaking. Press to end the call',
  ending: 'Ending the call',
};

function tooltipFor(phase: CallPhase): string {
  return phase === 'ending' ? TOOLTIP.ending : tooltipWithShortcut(TOOLTIP[phase], 'toggleCall');
}

const CONNECT_DWELL_MS = 2_000;

const WAITING_TOOLTIP = 'Waiting for microphone access';

export const MicrophoneList = lazyComponent(() => import('./MicrophonePicker').then((m) => m.MicrophoneList));
prefetchWhenIdle(MicrophoneList);

export function CallToggle({ available = true, attrs }: { available?: boolean; attrs?: Record<string, string> }) {
  preferences.value;
  const call = voiceCall.value;
  const phase = call.phase;
  const on = isOnCall(phase);
  const dead = phase === 'ending';
  const connecting = phase === 'connecting';
  const waiting = useDelayedFlag(connecting, CONNECT_DWELL_MS) && connecting;
  const openRef = useRef<OverflowMenuOpener | null>(null);
  const press = useLongPress(
    (button) => openRef.current?.(button),
    () => {
      if (!dead) pressCallToggle();
    },
  );
  if (!callOffered(available) && !on) return null;
  return (
    <>
      <ToggleIconButton
        {...attrs}
        tone="live"
        pressed={on}
        class="header-icon"
        data-role="call-toggle"
        data-call-phase={phase}
        data-call-wait={waiting ? 'microphone' : undefined}
        aria-disabled={dead}
        aria-label={PRESS_NAME[phase]}
        data-tooltip={waiting ? WAITING_TOOLTIP : tooltipFor(phase)}
        onPointerDown={press.onPointerDown}
        onPointerMove={press.onPointerMove}
        onPointerUp={press.onPointerUp}
        onPointerLeave={press.onPointerLeave}
        onPointerCancel={press.onPointerCancel}
        onContextMenu={press.onContextMenu}
        onClick={press.onClick}
      >
        <CallIcon />
      </ToggleIconButton>
      <span class="visually-hidden" role="status" data-role="call-state">
        {callStatusLabel(call)}
      </span>
      <OverflowMenu
        ariaLabel="Choose a microphone"
        stopPropagation
        hostOpener={{ ref: openRef, trigger: false }}
        itemsChunk={MicrophoneList}
        items={({ run }) => <MicrophoneList run={run} />}
      />
    </>
  );
}

function callOffered(available: boolean): boolean {
  return voiceEnabled() && available;
}

export function callToggleAction(available: boolean): HeaderActionSpec | null {
  preferences.value;
  const call = voiceCall.value;
  const on = isOnCall(call.phase);
  if (!callOffered(available) && !on) return null;
  return {
    key: 'call-toggle',
    dataRole: 'call-toggle',
    label: PRESS_NAME[call.phase],
    tooltip: tooltipFor(call.phase),
    icon: () => <CallIcon />,
    active: on,
    render: (attrs) => <CallToggle available={available} attrs={attrs} />,
    onClick: () => pressCallToggle(),
  };
}
