/**
 * The call the UI reads, and the one press that starts and ends it.
 *
 * A call belongs to the thread it was placed on and ends when that thread is
 * left, which is the watcher below. The rules live in `voice/callState.ts` and
 * the devices in `voice/ports.ts`, so this file is the wiring between them and
 * the shell.
 *
 * **A call also ends when the thread stops being one it can run on** (ADR
 * 0165). Moving a draft's destination to a coding agent is the same kind of
 * departure as navigating away, so it takes the same exit. The engine refuses
 * such a thread at two layers. This keeps the caller from talking into a call
 * those layers have already decided against.
 *
 * Built through a factory so a test supplies its own devices, its own focus
 * signal and its own thread resolver. The live instance is the last few lines.
 */
import { type Signal, computed, effect, signal } from '@preact/signals';
import { focusedThreadId, showToast, threadMap } from './store';
import { resolveCodingAgent } from './composeSelections';
import { awaitThreadStarted, ensureFocusedComposeThread } from './actions/compose';
import { openSettingsSubview } from './actions/menu';
import { storedVoiceInputDevice } from './actions/preferences';
import { effectiveCodingAgentBackend } from '../components/chat/promptToggleMode';
import { createCallRunner } from '../voice/call';
import { CALL_IDLE, type CallState } from '../voice/callState';
import { type CallPorts, browserPorts } from '../voice/ports';
import { isSettingsProblem } from '../voice/refusals';
import { errorDetail } from '../utils/errorDetail';

/** Said when a call has no thread to run on, which is the only way it fails
 *  before the microphone is ever asked for. */
export const NO_THREAD_TO_CALL_ON = 'The call needs a thread, and one could not be started.';

/** Said when the destination moves to a coding agent under a live call. */
export const DESTINATION_LEFT_THE_CALL =
  'The call ended: it runs on a Lucidos Agent thread, and this one is now a coding agent.';

/** Said when the destination moves while the dial is still waiting on its
 *  thread row, so the call never rings at all. Distinct from the one above,
 *  which reports a call that was up and has gone. */
export const DESTINATION_LEFT_THE_DIAL =
  'The call did not start: it runs on a Lucidos Agent thread, and the destination changed.';

export interface VoiceCallStore {
  call: Signal<CallState>;
  /** The one control was pressed. */
  press(): void;
  /** Stop watching the focus. For a test, and for nothing else. */
  dispose(): void;
}

export interface VoiceCallDeps {
  ports: CallPorts;
  /** The thread on screen. A call ends when it stops being its own. */
  focused: Signal<string | null>;
  /** The thread to call on: the focused one, or a fresh draft in compose. */
  resolveThread(): string;
  /** Resolves once that thread exists on the engine. */
  awaitThread(threadId: string): Promise<void>;
  onProblem(message: string): void;
  /** The microphone picked on this device, or `null` for the system default. */
  microphone(): string | null;
  /** Whether the focused destination is one a call can run on.
   *
   *  A signal rather than a call, because the watcher below has to WAKE when
   *  it changes. A destination picked mid-call moves nothing else. */
  reachable: Signal<boolean>;
}

export function createVoiceCallStore(deps: VoiceCallDeps): VoiceCallStore {
  const call = signal<CallState>(CALL_IDLE);

  const runner = createCallRunner({
    ports: deps.ports,
    microphone: deps.microphone,
    onState: (next) => {
      const previous = call.peek();
      call.value = next;
      // A call draws nothing of its own, so a reason left on an ended one has
      // nowhere to be read. A toast is what carries it after the call is gone.
      if (previous.phase !== 'idle' && next.phase === 'idle' && next.note) {
        deps.onProblem(next.note);
      }
    },
    onProblem: deps.onProblem,
  });

  /**
   * A dial waiting on its thread row to exist.
   *
   * The call is not `connecting` yet, so the toggle still reads as off and a
   * second press would take the branch below again. Two dials then land in
   * order: the first places the call and the second rings it straight off,
   * which reads as a button that did nothing. A press inside the window is
   * absorbed instead, because there is already a call on its way.
   *
   * Only a brand-new compose draft opens a window worth naming. On a thread
   * that exists, `awaitThread` settles on the next microtask, well before a
   * second click can arrive.
   */
  let dialing = false;

  function press(): void {
    const current = call.peek();
    if (current.phase !== 'idle' && current.threadId !== null) {
      dialing = false;
      runner.press(current.threadId);
      return;
    }
    if (dialing) return;
    // Synchronous, and before anything is awaited. This is the press, and on
    // iOS it is the only moment audio can be unlocked.
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
        // The row landed, but the reader may have moved on while it did. The
        // focus watcher cannot catch this one: it ran while the call was still
        // idle, and nothing moves the focus again to give it a second chance.
        if (deps.focused.peek() !== threadId) {
          runner.release();
          return;
        }
        // The destination has the same blind spot, for the same reason, and it
        // can move inside this window: the picker sits beside the control that
        // was pressed. Dialling on would open a socket the engine refuses.
        //
        // Said, where the focus case is silent. That reader navigated away and
        // is looking at another thread. This one is still looking at the row
        // they pressed, where the control has quietly gone.
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

  /** Say why, and hand back the audio device the press woke for nothing. */
  function abandonDial(message: string): void {
    runner.release();
    deps.onProblem(message);
  }

  // Two ways a call stops being this thread's: the reader leaves it, or the
  // thread stops being one a call can run on. Both read as a departure, so
  // both take the same exit.
  //
  // Only the second is worth a word. Navigating away is the reader's own doing
  // and they can see it happen. A destination pick is a dropdown three controls
  // away, and nothing else on screen would say the call had gone.
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

/**
 * Whether the focused destination is one a call can run on.
 *
 * The same two calls the prompt row makes, with the same arguments, so the
 * control the reader sees and the call they placed cannot disagree. Restating
 * the rule instead would be a third copy of it, and a third copy is how they
 * drift.
 *
 * `resolveCodingAgent` rather than the bare `selectedCodingAgent`, which
 * `.claude/rules/frontend.md` bans a compose surface from reading directly: a
 * per-draft pick must not fall through to the account default. It cannot
 * change this answer today, since the `null` here turns on the send MODE
 * alone. It would the moment a backend of its own can decline a call.
 *
 * It answers for all three shapes: a started thread, a composing draft, and
 * the fresh compose view with a destination picked and no draft yet.
 */
export const callReachesTheFocusedThread = computed(() => {
  const id = focusedThreadId.value;
  const thread = id ? threadMap.value.get(id) : undefined;
  return effectiveCodingAgentBackend(thread, resolveCodingAgent(id)) === null;
});

/**
 * Say why a call could not run, with a way to fix it when there is one.
 *
 * A reason the reader can act on gets the button that lands them on it. The
 * engine's talker refusal is the case: it says to set a model in Settings, and
 * until this it named a page with no such control.
 */
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
  ports: browserPorts,
  focused: focusedThreadId,
  // The focused thread when there is one, active threads included. In the
  // compose view this allocates the draft and POSTs its row, which is how
  // voice creates a thread by exactly the path typing uses.
  resolveThread: ensureFocusedComposeThread,
  awaitThread: awaitThreadStarted,
  onProblem: reportCallProblem,
  // Read per call, so a device picked between two calls reaches the second.
  microphone: () => storedVoiceInputDevice() || null,
  reachable: callReachesTheFocusedThread,
});

/** The live call, for the toggle to read. */
export const voiceCall: Signal<CallState> = live.call;

/** Press the call toggle: place a call, or ring off. */
export const pressCallToggle = live.press;
