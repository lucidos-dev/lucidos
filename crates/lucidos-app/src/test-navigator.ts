export function ensureNavigator(target: { navigator?: unknown }): void {
  if (typeof target.navigator === 'undefined') {
    Object.defineProperty(target, 'navigator', {
      value: { userAgent: 'node' },
      writable: true,
      configurable: true,
    });
  }
}
