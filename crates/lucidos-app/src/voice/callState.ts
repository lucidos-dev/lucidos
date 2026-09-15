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
 * A call holds every word either side has said, as it is said.
 *
 * ADR 0174 is why the caller's finished words are kept: the engine holds them
 * across the talker's decision, so a client that drops them has nothing to
 * draw for as long as that takes. The two live captions beside them, `hearing`
 * and `said`, carry the same argument further. Both ride frames the client
 * already receives. Dropping them left the transcript empty for the whole of
 * a reply, and for the whole of a sentence.
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
  /**
   * What the provider says utterance `utteranceCount` was, or `null`.
   *
   * The one caption a call keeps, and it captions nothing in flight: it is set
   * only once the provider has ENDED the turn, so the sentence is final. The
   * transcript draws it the moment the speaking bars stop, instead of waiting
   * on the engine's own row for it.
   *
   * A revision replaces it. Two `user_turn_ended` frames for one turn mean the
   * provider corrected itself, and the reader is owed the better text.
   */
  heard: string | null;
  /**
   * What the provider has heard of utterance `utteranceCount` SO FAR.
   *
   * A partial, and the one caption that is not final. It captions the bubble
   * while they speak and settles nothing: `heard` replaces it the moment the
   * turn ends, and the transcript's claim ledger reads `heard` alone.
   *
   * Null when the caller has said nothing yet this utterance, and null for the
   * whole call when the transcriber streams no deltas. The bubble then pulses,
   * exactly as it did before anything here was partial.
   */
  hearing: string | null;
  /**
   * The reply the talker is speaking, built from its deltas as they arrive.
   *
   * Empty between replies. The transcript's row for it is NOT withdrawn when
   * this empties: the engine's own row is on its way. Dropping the words to
   * wait for it is the blank ADR 0174 removed from the other side.
   */
  said: string;
  /**
   * Which reply of this call `said` is, counted from one.
   *
   * The row's identity, exactly as `utteranceCount` is for the caller's. It
   * tells a fresh reply from a revision of the one being spoken, so a reply
   * already on screen does not have its timestamp jump.
   */
  replyCount: number;
}

export const CALL_IDLE: CallState = {
  phase: 'idle',
  threadId: null,
  note: null,
  utterance: 'none',
  utteranceCount: 0,
  heard: null,
  hearing: null,
  said: '',
  replyCount: 0,
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
  /**
   * Send the audio captured before the socket was up, oldest first.
   *
   * The microphone opens ahead of the dial, so a caller who starts talking at
   * once is already being recorded. Their words are held until there is
   * somewhere to send them, and this is that moment.
   */
  | { kind: 'flush-audio' }
  | { kind: 'teardown' };

const HANG_UP: CallEffect = { kind: 'send', control: { type: 'hang_up' } };
const BARGE_IN: CallEffect = { kind: 'send', control: { type: 'barge_in' } };
const STOP_PLAYBACK: CallEffect = { kind: 'stop-playback' };
const FORGET_SPEECH: CallEffect = { kind: 'forget-speech' };
const FLUSH_AUDIO: CallEffect = { kind: 'flush-audio' };
const TEARDOWN: CallEffect = { kind: 'teardown' };

/** True while the socket is up and the call is neither starting nor ending. */
export function isLive(phase: CallPhase): boolean {
  return phase === 'listening' || phase === 'speaking';
}

/**
 * True while the microphone is open and the caller's voice still counts.
 *
 * Wider than [`isLive`] by exactly `connecting`, and that gap is the whole
 * point. The device opens before the socket is dialled, and the engine sends
 * `session_started` only once the PROVIDER session is up. Measured at 3.79
 * seconds on one reported call.
 *
 * Gating the caller's indicator on the socket left that window drawing
 * nothing. Somebody who speaks the moment they press the button then watches
 * an empty transcript. Their audio is held rather than dropped, which is what
 * makes drawing it honest. See
 * `docs/plans/2026-09-14-the-transcript-shows-a-call-as-it-happens.md`.
 */
export function hearsTheCaller(phase: CallPhase): boolean {
  return phase === 'connecting' || isLive(phase);
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
        ? { state: { ...state, utterance: 'none', heard: null, hearing: null }, effects: [] }
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
  if (!hearsTheCaller(state.phase)) return unchanged(state);
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
  // A fresh turn, so the words of the last one are no longer this one's, and
  // neither is what was heard of it. The ROW keeps the finished ones:
  // `store/liveUtterance.ts` copied them out, and only the engine's own row
  // retires that (ADR 0174).
  return {
    ...state,
    utterance: 'live',
    utteranceCount: state.utteranceCount + 1,
    heard: null,
    hearing: null,
  };
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
  //
  // A FINISHED sentence is not mid-way through anything, and its row stays.
  // `call.rs` writes down whatever it is holding for every end reason, so the
  // words are still owed a row (ADR 0174). The bridge keeps that one standing.
  return {
    state: { ...state, phase: 'ending', utterance: 'none', heard: null, hearing: null },
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
      // The gate was already running, so whatever the caller said into the
      // connect window is held and goes up now. The utterance it may have
      // started is left exactly as it is: they are mid-sentence, and the
      // socket coming up is nothing they did.
      return state.phase === 'connecting'
        ? { state: { ...state, phase: 'listening' }, effects: [FLUSH_AUDIO] }
        : unchanged(state);
    case 'user_transcript': {
      // The caller's own words as the provider hears them, mid-sentence. It
      // captions the bubble and decides nothing: the utterance does not move,
      // and `heard` is what the transcript settles a row on.
      //
      // Only while they hold an utterance. A delta landing outside one belongs
      // to a sentence already captioned by `heard`, and drawing it would put
      // the sentence before last in the bubble.
      if (state.utterance === 'none' || state.heard !== null) return unchanged(state);
      // A delta with no words captions nothing. Taken, it would swap the pulse
      // for an empty caption. The engine drops one too; this is the second net.
      if (frame.text === '') return unchanged(state);
      const hearing = (state.hearing ?? '') + frame.text;
      return { state: { ...state, hearing }, effects: [] };
    }
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
          ? { state: { ...state, utterance: 'none', heard: null, hearing: null }, effects: [] }
          : unchanged(state);
      }
      // The words are kept, and this is the only place they enter the state.
      // An already-`transcribed` utterance still takes them: a second frame for
      // one turn is the provider revising itself, and the row is rewritten in
      // place rather than left showing the worse text.
      //
      // A `live` one takes NONE. The frame carries no id and lands well after
      // the caller stopped, so one arriving mid-word describes something they
      // have already finished saying. The state still moves, exactly as
      // before; only the caption is withheld, and the caller's own frame
      // supplies it when they stop. Putting an earlier sentence in the bubble
      // for the one being said is the failure this avoids (ADR 0174).
      const heard = state.utterance === 'live' ? state.heard : frame.transcript;
      // A frame saying nothing new is a no-op, so nothing downstream wakes.
      if (state.utterance === 'transcribed' && state.heard === heard) return unchanged(state);
      // The partial has been superseded by the final text, so it goes. Kept,
      // it would caption the next utterance with the tail of this one.
      const hearing = heard === null ? state.hearing : null;
      return { state: { ...state, utterance: 'transcribed', heard, hearing }, effects: [] };
    }
    case 'talker_transcript': {
      // Read for what it says AND for what it means. The words build the reply
      // the transcript draws, and the first of them is how the client learns
      // the talker has taken the floor.
      //
      // The gate is measured afresh on that first one, and that is what keeps
      // a barge-in reachable. Only an EDGE reaches this reducer, so a gate
      // left open from before swallows the one a barge-in is made of: the
      // caller would have to stop for a third of a second before cutting in.
      //
      // Their utterance goes with the floor, and goes as `transcribed`. The
      // talker answering PROVES the provider ended that turn and read it, so
      // the words are on their way. Speech from here is a new turn, and gets a
      // row of its own.
      if (!isOnCall(state.phase)) return unchanged(state);
      // A delta with no words moves NOTHING, and that is the whole of this
      // line. It opens no reply, so it spends no count. It takes no floor, so
      // it cannot retire the caller's bubble or reset the speech gate under
      // somebody mid-word. The engine really does send one: every provider
      // forwards a blank delta as it arrives.
      if (frame.text === '') return unchanged(state);
      // An empty `said` means this delta OPENS a reply, so the row it draws is
      // a new one and takes a fresh count.
      const opens = state.said === '';
      const said = state.said + frame.text;
      const replyCount = opens ? state.replyCount + 1 : state.replyCount;
      if (state.phase !== 'listening') {
        return { state: { ...state, said, replyCount }, effects: [] };
      }
      const utterance = state.utterance === 'live' ? 'transcribed' : state.utterance;
      return {
        state: { ...state, phase: 'speaking', utterance, said, replyCount },
        effects: [FORGET_SPEECH],
      };
    }
    case 'talker_turn_ended':
      // The floor comes back with audio still in the air: this says the
      // provider stopped generating, and the speaker plays what it already
      // sent. So the run measured under it is discarded.
      //
      // `said` empties here and the ROW it drew does not. The engine writes a
      // `SpokenReplyGenerated` for every reply, and that row is what retires
      // this one. Withdrawing it on the turn's end would blank the reply the
      // caller is still hearing the tail of.
      //
      // The gate is reset on a real flip only. On a floor the caller already
      // holds, this follows their own barge-in. Shutting the gate mid-word
      // takes the utterance they are still saying with it.
      if (state.phase === 'speaking') {
        return { state: { ...state, phase: 'listening', said: '' }, effects: [FORGET_SPEECH] };
      }
      // Nothing to empty and no floor to flip, so nothing downstream wakes.
      return state.said === '' ? unchanged(state) : { state: { ...state, said: '' }, effects: [] };
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
