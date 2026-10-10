/** Cut `text` down to `maxChars`, ending in an ellipsis when it had to cut.
 *
 *  Every user-facing string built from a server response passes through this.
 *  A response body is bounded by nothing, and one that reaches the screen
 *  unbounded stops being a message and becomes a wall of text: the workspace
 *  gateway's boot splash is an 8 KB HTML page, and it once rendered as a toast
 *  covering the transcript.
 *
 *  Counts and slices by CODE POINT rather than by UTF-16 unit. A cut can then
 *  never strand half of a surrogate pair (an emoji) as a replacement glyph.
 */
export function clampText(text: string, maxChars: number): string {
  // A string's UTF-16 length is never below its code-point count, so this
  // answers the common case without building the array. Every toast in the app
  // takes this path.
  if (text.length <= maxChars) return text;
  const points = [...text];
  if (points.length <= maxChars) return text;
  return `${points.slice(0, maxChars - 1).join('').trimEnd()}…`;
}
