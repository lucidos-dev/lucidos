/** Hash-channel deep link to a thread. Hash (not `?thread=`) is deliberate:
 *  `startClient` strips `?thread=` unconditionally to defuse stale SW deep-
 *  links, so the query channel can't be repurposed for user-initiated
 *  cross-workspace navigation. */
export const THREAD_HASH_RE = /^#thread=([0-9a-f-]+)$/;
