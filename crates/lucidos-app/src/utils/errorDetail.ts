export function errorDetail(err: unknown): string {
  if (err instanceof DOMException) {
    if (err.name === 'TimeoutError') return 'request timed out';
    if (err.name === 'AbortError') return 'request cancelled';
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

export function isAbortError(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError';
}
