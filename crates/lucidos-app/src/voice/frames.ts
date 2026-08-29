/**
 * What travels over `/api/v1/voice`, from this side of it.
 *
 * The mirror of `crates/lucidos-engine/src/voice/wire.rs`, which is the
 * authority. Binary frames are the audio itself, so nothing here carries a
 * sample.
 *
 * **The client stays dumb** (ADR 0149). No provider name, model id, endpoint or
 * credential appears in any type here, and none may be added.
 */
import type { VoiceSessionEndReason } from '../store/thread-events/thread-event-types';

/** The PCM both directions speak, named once at the top of the call. */
export interface AudioSpec {
  sample_rate_hz: number;
  channels: number;
  encoding: string;
}

/** Everything the client may say as text. Its binary frames are microphone
 *  audio, and the engine refuses any other control. */
export type ClientControl = { type: 'barge_in' } | { type: 'hang_up' };

/** Everything the engine may say as text. Its binary frames are talker audio. */
export type ServerFrame =
  | { type: 'session_started'; audio: AudioSpec }
  | { type: 'user_turn_ended'; transcript: string }
  | { type: 'talker_transcript'; text: string }
  | { type: 'talker_turn_ended' }
  | { type: 'interrupted' }
  | { type: 'session_ended'; reason: VoiceSessionEndReason }
  | { type: 'error'; message: string };

/**
 * Every server tag, and the one payload field it carries.
 *
 * Kept exhaustive by `satisfies` rather than by care: adding a union member
 * without a key here fails `tsc`. `null` is a frame that is only its tag.
 */
export const SERVER_FRAME_PAYLOAD = {
  session_started: ['audio', 'object'],
  user_turn_ended: ['transcript', 'string'],
  talker_transcript: ['text', 'string'],
  talker_turn_ended: null,
  interrupted: null,
  session_ended: ['reason', 'string'],
  error: ['message', 'string'],
} as const satisfies Record<ServerFrame['type'], readonly [string, 'string' | 'object'] | null>;

/** The same for the two things the client may say. Neither carries a payload. */
const CLIENT_CONTROL_FLAGS = {
  barge_in: true,
  hang_up: true,
} satisfies Record<ClientControl['type'], true>;

export const SERVER_FRAME_TYPES: ReadonlySet<string> = new Set(Object.keys(SERVER_FRAME_PAYLOAD));

export const CLIENT_CONTROL_TYPES: ReadonlySet<string> = new Set(
  Object.keys(CLIENT_CONTROL_FLAGS),
);

/**
 * Read one text frame, or `null` when it is not one this client knows.
 *
 * Tolerant on purpose. An engine ahead of this bundle may send a newer frame.
 * A call carries on rather than dying on a word it has not learnt. Malformed
 * JSON reads the same way, because both mean the same thing here. There is
 * nothing to act on.
 *
 * The PAYLOAD is checked too, not just the tag. A known tag whose field is
 * missing is the same nothing. Letting one through would put `undefined` in
 * front of the reader as if the talker had said it.
 */
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
