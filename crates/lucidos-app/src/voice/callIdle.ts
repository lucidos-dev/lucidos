/** The state of no call. A leaf apart from `callState.ts`, because the entry
 *  chunk's `voiceCall` signal needs it and a chunk holds a module whole: the
 *  state machine would ride along (ADR 0288). */
import type { CallState } from './callState';

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
