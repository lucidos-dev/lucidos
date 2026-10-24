/** When two spoken rows are one thing said, and how their words join.
 *
 *  A voice provider ends a speaker's turn after a fraction of a second of
 *  silence, so it cuts a sentence wherever the speaker breathes. Every turn is
 *  written down as it finishes, which is what lets the transcript read by the
 *  clock alone (ADR 0201). Putting the pieces back together is a READING of
 *  those rows, done here for the transcript and in
 *  `core/store/messages/spoken_merge.rs` for the doer's conversation history.
 *
 *  **One rule, two languages, proven identical.** `spoken-merge-fixture.json`
 *  is generated from the Rust side and replayed by `spokenMerge.test.ts`, so
 *  the two cannot drift. Add a case there, not here. */

/** How far apart two spoken rows may be and still be one thing said.
 *
 *  The value lives in Rust and travels in the fixture, which the test asserts
 *  against this constant. A reported call held gaps of up to 3.6 s INSIDE
 *  single sentences, so five clears ordinary speech with margin. */
export const MERGE_GAP_SECS = 5;

/** Characters that attach to the word before them, with no space.
 *
 *  The seam is where the transcriber cut, so the next piece often opens with
 *  the punctuation that ended the last one. `Status` and `, please` are one
 *  sentence, and a space between them would be wrong. */
const CLITIC_OPENERS = new Set([
  '.', ',', '!', '?', ';', ':', '…', ')', ']', '}', '%', "'", '’', '"', '”',
]);

/** Should these two rows read as one thing said?
 *
 *  Both halves are required. Adjacency is the caller's job, walking the rows in
 *  clock order: anything from another speaker between them means they are two
 *  things, however close the clock says they are. */
export function isOneUtterance(gapSecs: number, sameSpeaker: boolean): boolean {
  return sameSpeaker && gapSecs >= 0 && gapSecs <= MERGE_GAP_SECS;
}

/** Join one piece of speech onto another.
 *
 *  No space before a clitic, one space otherwise. Empty pieces contribute
 *  nothing, so a merge over a blank row reads as though it were not there. */
export function joinSpoken(first: string, second: string): string {
  const a = first.trim();
  const b = second.trim();
  if (!a) return b;
  if (!b) return a;
  return CLITIC_OPENERS.has(b[0]) ? `${a}${b}` : `${a} ${b}`;
}
