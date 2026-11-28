
export const MAX_ATTEMPTS = 6;
export const BASE_DELAY_MS = 800;
export const MAX_DELAY_MS = 15000;

export function isRetryableImageSrc(src: string): boolean {
  return !src.startsWith('blob:') && !src.startsWith('data:');
}

function servedByThisOrigin(src: string): boolean {
  if (typeof location === 'undefined') return false;
  try {
    return new URL(src, location.href).origin === location.origin;
  } catch {
    return false;
  }
}

export function retrySrc(src: string, attempt: number): string {
  if (attempt === 0 || !isRetryableImageSrc(src) || !servedByThisOrigin(src)) return src;
  return `${src}${src.includes('?') ? '&' : '?'}retry=${attempt}`;
}

export function retryDelayMs(attempt: number): number {
  return Math.min(BASE_DELAY_MS * 2 ** attempt, MAX_DELAY_MS);
}
