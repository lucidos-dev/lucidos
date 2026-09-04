/**
 * One call, as a state machine with no side effects.
 *
 * Every rule about a call lives here: what a press does, what each engine frame
 * means, what the caller's own voice means, and what ends a call. The shell
 * (`voice/call.ts`) owns the microphone, the socket, the speaker and the clock,
 * and owns no policy at all. It hands this reducer an input and carries out the
 * effects it returns.
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
 * floor. That is what decides whether the caller's own voice reads as an
 * interruption or as an utterance. The toggle's status region reads the pair as
 * the call's state.
 */
export type CallPhase = 'idle' | 'connecting' | 'listening' | 'speaking' | 'ending';

/**
 * Where the caller's current utterance is, from the first word to the row.
 *
 * A phase says who has the floor, and this says whether they are using it. The
 * two are separate because `listening` covers the whole stretch a caller may
 * speak in, most of which is silence.
 *
 * `landing` and `transcribed` both mean the caller has stopped and the words
 * have not arrived. They are told apart by whether the PROVIDER has ended the
 * turn. That is the line an utterance is counted by, so a caller drawing breath
 * mid-sentence stays inside the one they are saying.
 */
export type CallerUtterance = 'none' | 'live' | 'landing' | 'transcribed';

/**
 * How long an utterance may wait on the provider before the row is withdrawn.
 *
 * The precise retraction is `user_turn_ended` with an empty transcript, which
 * is the engine's own word for "that was a noise". This covers the case where
 * the provider never flags the noise at all and so says nothing.
 */
export const LANDING_BOUND_MS = 4_000;

/**
 * How long it may then wait on the words themselves.
 *
 * The engine holds a transcript until the talker decides whether to answer it
 * or delegate it, and writes the row only after that. So this waits on a model
 * round-trip rather than on a network hop.
 */
export const WORDS_BOUND_MS = 10_000;

/** What the status region says while the caller is mid-utterance. */
export const HEARING_YOU = 'Hearing you';

/**
 * A call holds no words.
 *
 * Nothing captions a call in flight, so the utterances the engine reports are
 * read for what they imply and dropped. What was said is written down by the
 * engine as thread events, which is where the reader finds it.
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
  utterance: CallerUtterance;
  /**
   * Which utterance of this call the field above is about, counted from one.
   *
   * The transcript draws one row per utterance, and the count is what tells a
   * new one from the one before it. Reading the state alone cannot: two
   * utterances in a row are both `live`.
   */
  utteranceCount: number;
}

export const CALL_IDLE: CallState = {
  phase: 'idle',
  threadId: null,
  note: null,
  utterance: 'none',
  utteranceCount: 0,
};

/** Everything that can move a call. */
export type CallInput =
  /** The one control was pressed. Places a call, or rings off. */
  | { kind: 'toggle'; threadId: string }
  /** The call's thread stopped being the focused one. */
  | { kind: 'leave' }
  | { kind: 'frame'; frame: ServerFrame }
  /**
   * The local gate opened or shut on the caller's own voice.
   *
   * One input for both readings. Over the talker it is an interruption, and on
   * the caller's own floor it is an utterance starting. Which one it is depends
   * on the phase, which is this reducer's to know and not the gate's.
   */
  | { kind: 'speech'; open: boolean }
  /** An utterance whose words never landed has run out of time. */
  | { kind: 'utterance-timeout' }
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
  /**
   * Shut the speech gate and measure the caller's voice afresh.
   *
   * Emitted at a floor flip, in both directions, and nowhere else. What the
   * microphone measured under the other speaker says nothing about this one.
   *
   * The reducer keeps the two in step: a `live` utterance moves to `landing`
   * in the same step. Shut under one that stayed `live`, the gate would leave
   * a row nothing could close, the falling edge that retracts it having been
   * spent here.
   */
  | { kind: 'forget-speech' }
  | { kind: 'teardown' };

const HANG_UP: CallEffect = { kind: 'send', control: { type: 'hang_up' } };
const BARGE_IN: CallEffect = { kind: 'send', control: { type: 'barge_in' } };
const STOP_PLAYBACK: CallEffect = { kind: 'stop-playback' };
const FORGET_SPEECH: CallEffect = { kind: 'forget-speech' };
const TEARDOWN: CallEffect = { kind: 'teardown' };

/** True while the socket is up and the call is neither starting nor ending. */
export function isLive(phase: CallPhase): boolean {
  return phase === 'listening' || phase === 'speaking';
}

/** True while a call exists in any form, so the toggle reads as on. */
export function isOnCall(phase: CallPhase): boolean {
  return phase !== 'idle';
}

/**
 * What the toggle's status region says the call is doing.
 *
 * The utterance wins while it is `live`, because that is the one state the
 * phase cannot express: the caller has the floor either way. It stands down the
 * moment they stop. So waiting on the words is announced as the `listening` it
 * still is, and the region speaks once per utterance rather than three times.
 */
export function callStatusLabel(state: CallState): string {
  if (state.utterance === 'live') return HEARING_YOU;
  switch (state.phase) {
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
    case 'speech':
      return onSpeech(state, input.open);
    case 'utterance-timeout':
      // The words never came, so the row promising them is withdrawn. Only a
      // wait can time out: a `live` utterance is bounded by the caller.
      return state.utterance === 'landing' || state.utterance === 'transcribed'
        ? { state: { ...state, utterance: 'none' }, effects: [] }
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

/**
 * The caller started or stopped making speech.
 *
 * Over the talker, the start is an interruption AND the beginning of an
 * utterance. Both, because the barge-in hands the floor straight back: by the
 * time anything reads this state the caller has it and is using it. Splitting
 * the two would cost the row the 120 ms the gate has already spent.
 */
function onSpeech(state: CallState, open: boolean): { state: CallState; effects: CallEffect[] } {
  if (!isLive(state.phase)) return unchanged(state);
  if (!open) {
    return state.utterance === 'live'
      ? { state: { ...state, utterance: 'landing' }, effects: [] }
      : unchanged(state);
  }
  const heard = startUtterance(state);
  return state.phase === 'speaking'
    ? { state: { ...heard, phase: 'listening' }, effects: [STOP_PLAYBACK, BARGE_IN] }
    : { state: heard, effects: [] };
}

/**
 * Begin an utterance, unless one is already being said.
 *
 * **A pause is not the end of one.** The gate shuts after 320 ms of quiet and
 * a provider endpoints on longer. So a caller drawing breath mid-sentence is
 * still inside the turn the engine will report. Counting that as a second
 * utterance would leave its row waiting on words that never come separately.
 *
 * So only a turn the provider has ENDED takes a fresh count, and a fresh row
 * with it. `landing` is the one state that has not, and it resumes.
 */
function startUtterance(state: CallState): CallState {
  if (state.utterance === 'live') return state;
  if (state.utterance === 'landing') return { ...state, utterance: 'live' };
  return { ...state, utterance: 'live', utteranceCount: state.utteranceCount + 1 };
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
  // The utterance goes with the call. Whatever the caller was mid-way through
  // saying, nothing will transcribe it now, so the row promising it must go.
  return {
    state: { ...state, phase: 'ending', utterance: 'none' },
    effects: [HANG_UP, STOP_PLAYBACK],
  };
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
    case 'user_turn_ended': {
      // The floor is already the caller's while they are speaking, so a
      // finished utterance moves no phase. The words are the engine's to write
      // down, not this reducer's to hold.
      //
      // It does move the utterance, and the transcript is read for one fact
      // alone: whether there was anything in it. An EMPTY one is the engine's
      // own word for "that was a noise", since `call.rs` refuses to hold a
      // wordless transcript. So the row goes at once rather than on a timer.
      //
      // Words mean a row is coming, and the wait is now on the talker deciding
      // what to do with them. That is a model round-trip, so it gets its own
      // bound.
      //
      // An EMPTY one retracts a `landing` utterance and nothing else. The
      // caller may still be speaking audibly when it lands, the provider
      // having endpointed mid-breath. Withdrawing a bubble mid-sentence is the
      // one thing this row must never do, and a noise that has stopped is
      // exactly what `landing` is.
      if (state.utterance === 'none') return unchanged(state);
      if (!frame.transcript.trim()) {
        return state.utterance === 'landing'
          ? { state: { ...state, utterance: 'none' }, effects: [] }
          : unchanged(state);
      }
      return state.utterance === 'transcribed'
        ? unchanged(state)
        : { state: { ...state, utterance: 'transcribed' }, effects: [] };
    }
    case 'talker_transcript': {
      // Read for what it means rather than what it says: the first delta of a
      // reply is how the client learns the talker has taken the floor. Only
      // the FIRST one moves anything, the rest arriving mid-reply.
      //
      // The gate is measured afresh from here, and that is what keeps a
      // barge-in reachable. Only an EDGE reaches this reducer, so a gate left
      // open from before swallows the one a barge-in is made of: the caller
      // would have to stop for a third of a second before they could cut in.
      //
      // Their utterance goes with the floor, and goes as `transcribed`. The
      // talker answering PROVES the provider ended that turn and read it, so
      // the words are on their way. Speech from here is a new turn, and gets a
      // row of its own.
      if (state.phase !== 'listening') return unchanged(state);
      const utterance = state.utterance === 'live' ? 'transcribed' : state.utterance;
      return { state: { ...state, phase: 'speaking', utterance }, effects: [FORGET_SPEECH] };
    }
    case 'talker_turn_ended':
      // The floor comes back with audio still in the air: this says the
      // provider stopped generating, and the speaker plays what it already
      // sent. So the run measured under it is discarded.
      //
      // Only a real flip. On a floor the caller already holds, this follows
      // their own barge-in. Shutting the gate mid-word would take the
      // utterance they are still saying with it.
      return state.phase === 'speaking'
        ? { state: { ...state, phase: 'listening' }, effects: [FORGET_SPEECH] }
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
