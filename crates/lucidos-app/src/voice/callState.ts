/**
 * One call, as a state machine with no side effects.
 *
 * Every rule about a call lives here: what a press does, what each engine frame
 * means, and what ends a call. The shell (`voice/call.ts`) owns the microphone,
 * the socket and the speaker, and owns no policy at all. It hands this reducer
 * an input and carries out the effects it returns.
 *
 * That split is what makes the hard part testable. Terminal paths are the
 * property that matters, and there are six. Each is a test, rather than a hope
 * about an audio device.
 */
import type { ClientControl, ServerFrame } from './frames';

/**
 * Where a call is.
 *
 * `listening` and `speaking` are both live, and they differ only in who has the
 * floor. The barge-in gate is armed on `speaking`, and the toggle's status
 * region reads the pair as the call's state.
 */
export type CallPhase = 'idle' | 'connecting' | 'listening' | 'speaking' | 'ending';

/**
 * A call holds no words.
 *
 * Nothing captions a call in flight, so the utterances the engine reports are
 * read for the phase they imply and dropped. What was said is written down by
 * the engine as thread events, which is where the reader finds it.
 */
export interface CallState {
  phase: CallPhase;
  /** The thread this call belongs to. A call is bound to one for its life. */
  threadId: string | null;
  /**
   * Why a call could not start, or could not go on, in plain English.
   *
   * Survives into `idle`, because that is when the reader needs it: the call is
   * gone and this is all that is left to explain it. Cleared by the next press.
   */
  note: string | null;
}

export const CALL_IDLE: CallState = {
  phase: 'idle',
  threadId: null,
  note: null,
};

/** Everything that can move a call. */
export type CallInput =
  /** The one control was pressed. Places a call, or rings off. */
  | { kind: 'toggle'; threadId: string }
  /** The call's thread stopped being the focused one. */
  | { kind: 'leave' }
  | { kind: 'frame'; frame: ServerFrame }
  /** The local detector heard the caller speak over the talker. */
  | { kind: 'barge-in' }
  | { kind: 'socket-closed' }
  /** The microphone, the audio device or the upgrade refused us. */
  | { kind: 'failed'; message: string }
  /**
   * The socket closed before the handshake, and why is still being measured.
   *
   * Carries no note on purpose. The devices go back NOW, rather than waiting
   * on a probe with the microphone open. The reason is reported separately
   * once it is known.
   */
  | { kind: 'refused' };

/** What the shell owes the world after a step. */
export type CallEffect =
  | { kind: 'open'; threadId: string }
  | { kind: 'send'; control: ClientControl }
  | { kind: 'stop-playback' }
  | { kind: 'teardown' };

const HANG_UP: CallEffect = { kind: 'send', control: { type: 'hang_up' } };
const BARGE_IN: CallEffect = { kind: 'send', control: { type: 'barge_in' } };
const STOP_PLAYBACK: CallEffect = { kind: 'stop-playback' };
const TEARDOWN: CallEffect = { kind: 'teardown' };

/** True while the socket is up and the call is neither starting nor ending. */
export function isLive(phase: CallPhase): boolean {
  return phase === 'listening' || phase === 'speaking';
}

/** True while a call exists in any form, so the toggle reads as on. */
export function isOnCall(phase: CallPhase): boolean {
  return phase !== 'idle';
}

/** What the toggle's status region says the call is doing. */
export function callStatusLabel(phase: CallPhase): string {
  switch (phase) {
    case 'connecting':
      return 'Connecting';
    case 'listening':
      return 'Listening';
    case 'speaking':
      return 'Speaking';
    case 'ending':
      return 'Ending';
    case 'idle':
      return '';
  }
}

/** Advance a call by one input, and say what the shell must do about it. */
export function stepCall(
  state: CallState,
  input: CallInput,
): { state: CallState; effects: CallEffect[] } {
  switch (input.kind) {
    case 'toggle':
      return state.phase === 'idle' ? place(input.threadId) : ringOff(state);
    case 'leave':
      return ringOff(state);
    case 'frame':
      return onFrame(state, input.frame);
    case 'barge-in':
      return state.phase === 'speaking'
        ? { state: { ...state, phase: 'listening' }, effects: [STOP_PLAYBACK, BARGE_IN] }
        : unchanged(state);
    case 'socket-closed':
      return state.phase === 'idle' ? unchanged(state) : hungUp(state);
    case 'failed':
      return { state: { ...CALL_IDLE, note: input.message }, effects: [TEARDOWN] };
    case 'refused':
      return { state: CALL_IDLE, effects: [TEARDOWN] };
  }
}

function unchanged(state: CallState): { state: CallState; effects: CallEffect[] } {
  return { state, effects: [] };
}

function place(threadId: string): { state: CallState; effects: CallEffect[] } {
  return {
    state: { ...CALL_IDLE, phase: 'connecting', threadId },
    effects: [{ kind: 'open', threadId }],
  };
}

/**
 * End whatever is running, by press or by leaving the thread.
 *
 * A call still connecting has no socket to say goodbye on, so it is torn down
 * instead. The engine reads the dropped handshake as a disconnect and pairs the
 * session itself. That is what it already does for a caller who walks away.
 */
function ringOff(state: CallState): { state: CallState; effects: CallEffect[] } {
  if (state.phase === 'idle' || state.phase === 'ending') return unchanged(state);
  if (state.phase === 'connecting') return { state: CALL_IDLE, effects: [TEARDOWN] };
  return { state: { ...state, phase: 'ending' }, effects: [HANG_UP, STOP_PLAYBACK] };
}

/** The call is over. The note survives, because it is the only explanation. */
function hungUp(state: CallState): { state: CallState; effects: CallEffect[] } {
  return { state: { ...CALL_IDLE, note: state.note }, effects: [TEARDOWN] };
}

function onFrame(
  state: CallState,
  frame: ServerFrame,
): { state: CallState; effects: CallEffect[] } {
  switch (frame.type) {
    case 'session_started':
      return state.phase === 'connecting'
        ? { state: { ...state, phase: 'listening' }, effects: [] }
        : unchanged(state);
    case 'user_turn_ended':
      // The floor is already the caller's while they are speaking, so a
      // finished utterance moves nothing. The words are the engine's to write
      // down, not this reducer's to hold.
      return unchanged(state);
    case 'talker_transcript':
      // Read for what it means rather than what it says: the first delta of a
      // reply is how the client learns the talker has taken the floor.
      return isLive(state.phase)
        ? { state: { ...state, phase: 'speaking' }, effects: [] }
        : unchanged(state);
    case 'talker_turn_ended':
      return isLive(state.phase)
        ? { state: { ...state, phase: 'listening' }, effects: [] }
        : unchanged(state);
    case 'interrupted':
      return isLive(state.phase)
        ? { state: { ...state, phase: 'listening' }, effects: [STOP_PLAYBACK] }
        : unchanged(state);
    case 'session_ended':
      return state.phase === 'idle' ? unchanged(state) : hungUp(state);
    case 'error':
      // Never terminal on its own. The engine sends one for a control it could
      // not read and carries on, and sends one before it closes for anything
      // worse. The close is what ends the call, so this only records why.
      return { state: { ...state, note: frame.message }, effects: [] };
  }
}
