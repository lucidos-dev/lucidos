/** Pure: was this page served by the very machine reading it?
 *
 *  Loopback is the one sound implication between the page's two concerns. A
 *  request that never left the machine was answered by the engine host. So the
 *  reader IS the host, and concern 1 is the whole answer for it. Covers all of
 *  `127.0.0.0/8` and the RFC 6761 `.localhost` tree, the latter because Tauri
 *  serves its bundled assets from `tauri.localhost` off macOS. */
export function isLoopbackHostname(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host.endsWith('.localhost')) return true;
  if (host === '::1') return true;
  const v4 = /^(\d{1,3})\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.exec(host);
  return v4 !== null && v4[1] === '127';
}
