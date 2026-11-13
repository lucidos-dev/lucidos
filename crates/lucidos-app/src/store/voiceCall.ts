/** The live voice call, for whatever draws or reacts to it.
 *
 *  `voice.ts` places and ends calls. This lives apart from it so the effects
 *  that only read the call do not pull the call runner into the entry chunk
 *  (ADR 0288). */

import { type Signal, signal } from '@preact/signals';
import { CALL_IDLE, type CallState } from '../voice/callState';

export const voiceCall: Signal<CallState> = signal(CALL_IDLE);
