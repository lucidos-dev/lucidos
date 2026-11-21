/** Node 21 added a global `navigator`; Node 20, which CI runs, has none. Store
 *  modules read `navigator.userAgent` at import, so the suite supplies one
 *  instead of depending on the host's Node version. */
export function ensureNavigator(target: { navigator?: unknown }): void {
  if (typeof target.navigator === 'undefined') {
    Object.defineProperty(target, 'navigator', {
      value: { userAgent: 'node' },
      writable: true,
      configurable: true,
    });
  }
}
