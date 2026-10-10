/**
 * UUID v4 that also works on insecure origins.
 *
 * `crypto.randomUUID` exists only in secure contexts (https or localhost). A
 * packaged install reached over plain `http://<host>:<port>/` has no secure
 * context, and a bare `crypto.randomUUID()` in the startup path (device
 * registration) threw before the app could render — the whole app was dead on
 * arrival, not just push. `crypto.getRandomValues` IS available in insecure
 * contexts, so fall back to a spec-correct v4 built from it.
 *
 * Always call this instead of `crypto.randomUUID()` directly.
 */
export function generateUuid(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
