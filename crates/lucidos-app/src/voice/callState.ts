import type { ClientControl, ServerFrame } from './frames';
import { CALL_IDLE } from './callIdle';

export { CALL_IDLE };

export type CallPhase = 'idle' | 'connecting' | 'listening' | 'speaking' | 'ending';

export type CallerUtterance = 'none' | 'live' | 'landing' | 'transcribed';

export const LANDING_BOUND_MS = 4_000;

export const WORDS_BOUND_MS = 10_000;

export const HEARING_YOU = 'Hearing you';

export const BARGE_IN_QUIET_MS = 1_000;

export interface CallState {
  phase: CallPhase;
  threadId: string | null;
  note: string | null;
  utterance: CallerUtterance;
  utteranceCount: number;
  heard: string | null;
  hearing: string | null;
  said: string;
  replyCount: number;
}

export type CallInput =
  | { kind: 'toggle'; threadId: string }
  | { kind: 'leave' }
  | { kind: 'frame'; frame: ServerFrame }
  | { kind: 'speech'; open: boolean; quietMs: number }
  | { kind: 'utterance-timeout' }
  | { kind: 'socket-closed' }
  | { kind: 'failed'; message: string }
  | { kind: 'refused' };

export type CallEffect =
  | { kind: 'open'; threadId: string }
  | { kind: 'send'; control: ClientControl }
  | { kind: 'stop-playback' }
  | { kind: 'forget-speech' }
  | { kind: 'flush-audio' }
  | { kind: 'teardown' };

const HANG_UP: CallEffect = { kind: 'send', control: { type: 'hang_up' } };
const BARGE_IN: CallEffect = { kind: 'send', control: { type: 'barge_in' } };
const STARTED_SPEAKING: CallEffect = {
  kind: 'send',
  control: { type: 'caller_started_speaking' },
};
const STOP_PLAYBACK: CallEffect = { kind: 'stop-playback' };
const FORGET_SPEECH: CallEffect = { kind: 'forget-speech' };
const FLUSH_AUDIO: CallEffect = { kind: 'flush-audio' };
const TEARDOWN: CallEffect = { kind: 'teardown' };

export function isLive(phase: CallPhase): boolean {
  return phase === 'listening' || phase === 'speaking';
}

export function hearsTheCaller(phase: CallPhase): boolean {
  return phase === 'connecting' || isLive(phase);
}

export function isOnCall(phase: CallPhase): boolean {
  return phase !== 'idle';
}

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
      return onSpeech(state, input.open, input.quietMs);
    case 'utterance-timeout':
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

function spokeIntoTheConnectWindow(state: CallState): boolean {
  return state.utterance === 'live' || state.utterance === 'landing';
}

function onSpeech(
  state: CallState,
  open: boolean,
  quietMs: number,
): { state: CallState; effects: CallEffect[] } {
  if (!hearsTheCaller(state.phase)) return unchanged(state);
  if (!open) {
    return state.utterance === 'live'
      ? { state: { ...state, utterance: 'landing' }, effects: [] }
      : unchanged(state);
  }
  const heard = startUtterance(state);
  const started = isLive(state.phase) ? [STARTED_SPEAKING] : [];
  return state.phase === 'speaking' && quietMs >= BARGE_IN_QUIET_MS
    ? {
        state: { ...heard, phase: 'listening' },
        effects: [...started, STOP_PLAYBACK, BARGE_IN],
      }
    : { state: heard, effects: started };
}

function startUtterance(state: CallState): CallState {
  if (state.utterance === 'live') return state;
  if (state.utterance === 'landing') return { ...state, utterance: 'live' };
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

function ringOff(state: CallState): { state: CallState; effects: CallEffect[] } {
  if (state.phase === 'idle' || state.phase === 'ending') return unchanged(state);
  if (state.phase === 'connecting') return { state: CALL_IDLE, effects: [TEARDOWN] };
  return {
    state: { ...state, phase: 'ending', utterance: 'none', heard: null, hearing: null },
    effects: [HANG_UP, STOP_PLAYBACK],
  };
}

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
        ? {
            state: { ...state, phase: 'listening' },
            effects: spokeIntoTheConnectWindow(state)
              ? [FLUSH_AUDIO, STARTED_SPEAKING]
              : [FLUSH_AUDIO],
          }
        : unchanged(state);
    case 'user_transcript': {
      if (state.utterance === 'none' || state.heard !== null) return unchanged(state);
      if (frame.text === '') return unchanged(state);
      const hearing = (state.hearing ?? '') + frame.text;
      return { state: { ...state, hearing }, effects: [] };
    }
    case 'user_turn_ended': {
      if (state.utterance === 'none') return unchanged(state);
      if (!frame.transcript.trim()) {
        return state.utterance === 'landing'
          ? { state: { ...state, utterance: 'none', heard: null, hearing: null }, effects: [] }
          : unchanged(state);
      }
      const heard = state.utterance === 'live' ? state.heard : frame.transcript;
      if (state.utterance === 'transcribed' && state.heard === heard) return unchanged(state);
      const hearing = heard === null ? state.hearing : null;
      return { state: { ...state, utterance: 'transcribed', heard, hearing }, effects: [] };
    }
    case 'talker_transcript': {
      if (!isOnCall(state.phase)) return unchanged(state);
      if (frame.text === '') return unchanged(state);
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
      if (state.phase === 'speaking') {
        return { state: { ...state, phase: 'listening', said: '' }, effects: [FORGET_SPEECH] };
      }
      return state.said === '' ? unchanged(state) : { state: { ...state, said: '' }, effects: [] };
    case 'interrupted':
      return isLive(state.phase)
        ? { state: { ...state, phase: 'listening' }, effects: [STOP_PLAYBACK] }
        : unchanged(state);
    case 'session_ended':
      return state.phase === 'idle' ? unchanged(state) : hungUp(state);
    case 'error':
      return { state: { ...state, note: frame.message }, effects: [] };
  }
}
