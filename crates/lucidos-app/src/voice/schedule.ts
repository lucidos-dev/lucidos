/**
 * Where the next chunk of talker audio starts, and how much cushion sits ahead
 * of it.
 *
 * Talker audio arrives as a stream of small chunks, and Web Audio plays a
 * buffer at an absolute time on the context clock. So playback is two numbers:
 * the time the last scheduled chunk finishes, and the cushion a fresh start
 * takes. Each new chunk butts against the cursor, or takes the cushion when the
 * queue has drained.
 *
 * **The cushion is the only jitter buffer a call has.** A talker that streams
 * audio at the pace a person hears it hands the client no depth of its own. The
 * queue then holds exactly this much and never more, so every stall longer than
 * the cushion is a hole in the reply. The caller hears a click at each edge.
 *
 * Pure arithmetic over a clock the caller reads, so the whole rule is tested
 * without an `AudioContext`.
 */

/**
 * How far ahead of now playback starts when the queue has drained.
 *
 * The cushion the engine and the network get to deliver the next chunk before
 * the caller hears a gap. Too small and speech stutters on a slow hop, too
 * large and the talker feels laggy.
 *
 * 200 ms is the smallest cushion that survives an engine round trip to
 * Postgres, which the call loop takes inline for every row it writes. The 80 ms
 * this replaced was sized for a talker that sent a whole reply at once. Such a
 * talker builds its own depth on top of the cushion.
 */
export const PLAYBACK_LEAD_SECONDS = 0.2;

/**
 * The most cushion one call will hold.
 *
 * The cushion grows to cover what it has already failed to cover, and this is
 * where growing stops. Past here the reply lands so late that the caller talks
 * over it, which is a worse call than an occasional gap.
 */
export const PLAYBACK_LEAD_MAX_SECONDS = 0.6;

/**
 * A drain longer than this is the talker not speaking.
 *
 * The queue empties for two reasons, and only one of them is a fault. A chunk
 * that is late by a fraction of a second is the cushion failing. A quiet
 * stretch of seconds is the reply being over, and treating that as a fault
 * would grow the cushion at every turn boundary.
 *
 * So a hole past this bound is neither counted nor learned from. A talker that
 * streams continuously can still stall for longer, and no cushion this side of
 * unusable would have absorbed it.
 */
export const QUIET_STREAM_SECONDS = 1;

/** Where playback is, and what this call has learned about its own line. */
export interface Playback {
  /** The context time the last scheduled chunk finishes. Zero before the first. */
  cursor: number;
  /** The cushion a fresh start takes, grown by whatever has gone wrong so far. */
  lead: number;
  /** How many times the speaker ran dry mid-stream. */
  gaps: number;
  /** How long the speaker was silent across those gaps, in seconds.
   *
   *  The whole hole, which is the late chunk PLUS the lead it then takes.
   *  Both are silence the caller sat through, and the second half is the
   *  larger of the two once the lead has grown. */
  silentSeconds: number;
}

export const PLAYBACK_START: Playback = {
  cursor: 0,
  lead: PLAYBACK_LEAD_SECONDS,
  gaps: 0,
  silentSeconds: 0,
};

export interface Placed {
  /** The context time to start this chunk at. */
  startAt: number;
  /** The state to carry into the next chunk. */
  playback: Playback;
}

/**
 * Place one chunk against the cursor, and learn from a queue that ran dry.
 *
 * **Audio still queued is never re-led.** A chunk whose cursor sits ahead of
 * now butts straight against it, however thin the cushion has worn. Pushing it
 * out to the full lead would open a gap where the stream was whole. That is the
 * cushion manufacturing the fault it exists to absorb.
 *
 * A drained queue takes the lead instead, because the cursor it would butt
 * against has already gone. That silence was heard, so it is counted and the
 * cushion grows to cover it next time.
 */
export function placeChunk(state: Playback, now: number, durationSeconds: number): Placed {
  const drained = now - state.cursor;
  // The first chunk of a call drains nothing: there was no cursor to run past.
  const ranDry = state.cursor > 0 && drained > 0 && drained <= QUIET_STREAM_SECONDS;
  const lead = ranDry
    ? Math.min(PLAYBACK_LEAD_MAX_SECONDS, Math.max(state.lead, drained + PLAYBACK_LEAD_SECONDS))
    : state.lead;
  const startAt = state.cursor > now ? state.cursor : now + lead;
  return {
    startAt,
    playback: {
      cursor: startAt + durationSeconds,
      lead,
      gaps: ranDry ? state.gaps + 1 : state.gaps,
      // The whole hole, cursor to restart, not just how late the chunk was.
      silentSeconds: ranDry
        ? state.silentSeconds + (startAt - state.cursor)
        : state.silentSeconds,
    },
  };
}

/**
 * Start playback over, keeping what this call has learned.
 *
 * The cursor goes, because the queue it described has been thrown away. The
 * cushion and the tally stay: a caller cutting the talker off says nothing
 * about how well the line was carrying audio.
 */
export function restartPlayback(state: Playback): Playback {
  return { ...state, cursor: 0 };
}
