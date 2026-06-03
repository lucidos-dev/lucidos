/** Format error detail for toast messages. Handles AbortError, Error, and unknown shapes. */
export function errorDetail(err: unknown): string {
  if (err instanceof DOMException) {
    // Distinguish "we gave up" (timeout) from "you stopped it" (manual abort).
    if (err.name === 'TimeoutError') return 'request timed out';
    if (err.name === 'AbortError') return 'request cancelled';
  }
  if (err instanceof Error) return err.message;
  return String(err);
}
