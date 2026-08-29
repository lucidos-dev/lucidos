/**
 * Where the next chunk of talker audio starts playing.
 *
 * Talker audio arrives as a stream of small chunks, and Web Audio plays a
 * buffer at an absolute time on the context clock. So the whole of playback is
 * one number: the time the last scheduled chunk finishes. Each new chunk starts
 * there, or a little ahead of now when the queue has drained.
 *
 * Pure arithmetic over a clock the caller reads, so the scheduling rule is
 * tested without an `AudioContext`.
 */

/**
 * How far ahead of now a chunk starts when the queue has drained.
 *
 * The cushion the network gets to deliver the next chunk before the caller
 * hears a gap. Too small and speech stutters on a slow hop, too large and the
 * talker feels laggy. 80 ms is about two chunks of the size the engine sends.
 */
export const PLAYBACK_LEAD_SECONDS = 0.08;

export interface Scheduled {
  /** The context time to start this chunk at. */
  startAt: number;
  /** The cursor to carry into the next chunk. */
  cursor: number;
}

/**
 * Place one chunk against the cursor.
 *
 * A cursor in the past means playback ran dry. The chunk then takes the lead
 * rather than a time that has already gone. A chunk scheduled in the past
 * plays at once, stacked on whatever else is due. That is heard as a garble
 * rather than as a late word.
 */
export function scheduleChunk(
  cursor: number,
  now: number,
  durationSeconds: number,
  lead: number = PLAYBACK_LEAD_SECONDS,
): Scheduled {
  const startAt = Math.max(cursor, now + lead);
  return { startAt, cursor: startAt + durationSeconds };
}
