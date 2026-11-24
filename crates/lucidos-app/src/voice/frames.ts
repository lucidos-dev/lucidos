import type { VoiceSessionEndReason } from '../store/thread-events/thread-event-types';

export interface AudioSpec {
  sample_rate_hz: number;
  channels: number;
  encoding: string;
}

export type ClientControl =
  | { type: 'barge_in' }
  | { type: 'caller_started_speaking' }
  | { type: 'hang_up' };

export type ServerFrame =
  | { type: 'session_started'; audio: AudioSpec }
  | { type: 'user_transcript'; text: string }
  | { type: 'user_turn_ended'; transcript: string }
  | { type: 'talker_transcript'; text: string }
  | { type: 'talker_turn_ended' }
  | { type: 'interrupted' }
  | { type: 'session_ended'; reason: VoiceSessionEndReason }
  | { type: 'error'; message: string };

export const SERVER_FRAME_PAYLOAD = {
  session_started: ['audio', 'object'],
  user_transcript: ['text', 'string'],
  user_turn_ended: ['transcript', 'string'],
  talker_transcript: ['text', 'string'],
  talker_turn_ended: null,
  interrupted: null,
  session_ended: ['reason', 'string'],
  error: ['message', 'string'],
} as const satisfies Record<ServerFrame['type'], readonly [string, 'string' | 'object'] | null>;

const CLIENT_CONTROL_FLAGS = {
  barge_in: true,
  caller_started_speaking: true,
  hang_up: true,
} satisfies Record<ClientControl['type'], true>;

export const SERVER_FRAME_TYPES: ReadonlySet<string> = new Set(Object.keys(SERVER_FRAME_PAYLOAD));

export const CLIENT_CONTROL_TYPES: ReadonlySet<string> = new Set(
  Object.keys(CLIENT_CONTROL_FLAGS),
);

export function parseServerFrame(text: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== 'object' || value === null) return null;
  const frame = value as Record<string, unknown>;
  const type = frame.type;
  if (typeof type !== 'string' || !SERVER_FRAME_TYPES.has(type)) return null;
  const payload = SERVER_FRAME_PAYLOAD[type as ServerFrame['type']];
  if (payload === null) return frame as unknown as ServerFrame;
  const [field, kind] = payload;
  const carried = frame[field];
  if (kind === 'string' && typeof carried !== 'string') return null;
  if (kind === 'object' && (typeof carried !== 'object' || carried === null)) return null;
  return frame as unknown as ServerFrame;
}
