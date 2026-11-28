import { type Signal, computed, effect, signal } from '@preact/signals';
import { focusedThreadId, showToast, threadMap } from './store';
import { resolveCodingAgent } from './composeSelections';
import { awaitThreadStarted, ensureFocusedComposeThread } from './actions/compose';
import { openSettingsSubview } from './actions/menu';
import { storedVoiceInputDevice } from './actions/preferences';
import { effectiveCodingAgentBackend } from '../components/chat/promptToggleMode';
import { callIsOffered } from '../voice/callOffer';
import { createCallRunner } from '../voice/call';
import { CALL_IDLE, type CallState } from '../voice/callState';
import { voiceCall } from './voiceCall';
import { type CallPorts, browserPorts } from '../voice/ports';
import { isSettingsProblem } from '../voice/refusals';
import { errorDetail } from '../utils/errorDetail';

export const NO_THREAD_TO_CALL_ON = 'The call needs a thread, and one could not be started.';

export const DESTINATION_LEFT_THE_CALL =
  'The call ended: it runs on a Lucidos Agent thread, and this one is now a coding agent.';

export const DESTINATION_LEFT_THE_DIAL =
  'The call did not start: it runs on a Lucidos Agent thread, and the destination changed.';

export interface VoiceCallStore {
  call: Signal<CallState>;
  press(): void;
  dispose(): void;
}

export interface VoiceCallDeps {
  call?: Signal<CallState>;
  ports: CallPorts;
  focused: Signal<string | null>;
  resolveThread(): string;
  awaitThread(threadId: string): Promise<void>;
  onProblem(message: string): void;
  microphone(): string | null;
  reachable: Signal<boolean>;
}

export function createVoiceCallStore(deps: VoiceCallDeps): VoiceCallStore {
  const call = deps.call ?? signal<CallState>(CALL_IDLE);

  const runner = createCallRunner({
    ports: deps.ports,
    microphone: deps.microphone,
    onState: (next) => {
      const previous = call.peek();
      call.value = next;
      if (previous.phase !== 'idle' && next.phase === 'idle' && next.note) {
        deps.onProblem(next.note);
      }
    },
    onProblem: deps.onProblem,
  });

  let dialing = false;

  function press(): void {
    const current = call.peek();
    if (current.phase !== 'idle' && current.threadId !== null) {
      dialing = false;
      runner.press(current.threadId);
      return;
    }
    if (dialing) return;
    runner.prime();
    let threadId: string;
    try {
      threadId = deps.resolveThread();
    } catch (err) {
      abandonDial(`${NO_THREAD_TO_CALL_ON} ${errorDetail(err)}`);
      return;
    }
    dialing = true;
    deps.awaitThread(threadId).then(
      () => {
        dialing = false;
        if (deps.focused.peek() !== threadId) {
          runner.release();
          return;
        }
        if (!deps.reachable.peek()) {
          abandonDial(DESTINATION_LEFT_THE_DIAL);
          return;
        }
        runner.press(threadId);
      },
      (err: unknown) => {
        dialing = false;
        abandonDial(`${NO_THREAD_TO_CALL_ON} ${errorDetail(err)}`);
      },
    );
  }

  function abandonDial(message: string): void {
    runner.release();
    deps.onProblem(message);
  }

  const dispose = effect(() => {
    const focused = deps.focused.value;
    const reachable = deps.reachable.value;
    const current = call.peek();
    if (current.phase === 'idle' || current.threadId === null) return;
    if (current.threadId !== focused) {
      runner.leave();
      return;
    }
    if (!reachable) {
      runner.leave();
      deps.onProblem(DESTINATION_LEFT_THE_CALL);
    }
  });

  return { call, press, dispose };
}

export const callReachesTheFocusedThread = computed(() => {
  const id = focusedThreadId.value;
  const thread = id ? threadMap.value.get(id) : undefined;
  return callIsOffered(thread, effectiveCodingAgentBackend(thread, resolveCodingAgent(id)));
});

export function reportCallProblem(message: string): void {
  showToast(
    message,
    'error',
    isSettingsProblem(message)
      ? { action: { label: 'Open settings', onClick: () => openSettingsSubview('models') } }
      : undefined,
  );
}

const live = createVoiceCallStore({
  call: voiceCall,
  ports: browserPorts,
  focused: focusedThreadId,
  resolveThread: ensureFocusedComposeThread,
  awaitThread: awaitThreadStarted,
  onProblem: reportCallProblem,
  microphone: () => storedVoiceInputDevice() || null,
  reachable: callReachesTheFocusedThread,
});

export const pressCallToggle = live.press;
