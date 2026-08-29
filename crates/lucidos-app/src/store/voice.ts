/**
 * The call the UI reads, and the one press that starts and ends it.
 *
 * A call belongs to the thread it was placed on and ends when that thread is
 * left, which is the watcher below. The rules live in `voice/callState.ts` and
 * the devices in `voice/ports.ts`, so this file is the wiring between them and
 * the shell.
 *
 * Built through a factory so a test supplies its own devices, its own focus
 * signal and its own thread resolver. The live instance is the last few lines.
 */
import { type Signal, effect, signal } from '@preact/signals';
import { focusedThreadId, showToast } from './store';
import { awaitThreadStarted, ensureFocusedComposeThread } from './actions/compose';
import { openSettingsSubview } from './actions/menu';
import { createCallRunner } from '../voice/call';
import { CALL_IDLE, type CallState } from '../voice/callState';
import { type CallPorts, browserPorts } from '../voice/ports';
import { isSettingsProblem } from '../voice/refusals';
import { errorDetail } from '../utils/errorDetail';

/** Said when a call has no thread to run on, which is the only way it fails
 *  before the microphone is ever asked for. */
export const NO_THREAD_TO_CALL_ON = 'The call needs a thread, and one could not be started.';

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
}

export function createVoiceCallStore(deps: VoiceCallDeps): VoiceCallStore {
  const call = signal<CallState>(CALL_IDLE);

  const runner = createCallRunner({
    ports: deps.ports,
    onState: (next) => {
      const previous = call.peek();
      call.value = next;
      // The strip goes with the call, so a reason left on an ended call has
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

  const dispose = effect(() => {
    const focused = deps.focused.value;
    const current = call.peek();
    if (current.phase === 'idle' || current.threadId === null) return;
    if (current.threadId !== focused) runner.leave();
  });

  return { call, press, dispose };
}

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
});

/** The live call, for the toggle and the strip to read. */
export const voiceCall: Signal<CallState> = live.call;

/** Press the call toggle: place a call, or ring off. */
export const pressCallToggle = live.press;
