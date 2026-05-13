/** Format error detail for toast messages. Handles AbortError, Error, and unknown shapes. */
export function errorDetail(err: unknown): string {
  if (err instanceof DOMException && err.name === 'AbortError') return 'request timed out';
  if (err instanceof Error) return err.message;
  return String(err);
}
