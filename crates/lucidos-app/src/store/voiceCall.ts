
import { type Signal, signal } from '@preact/signals';
import { CALL_IDLE } from '../voice/callIdle';
import type { CallState } from '../voice/callState';

export const voiceCall: Signal<CallState> = signal(CALL_IDLE);
