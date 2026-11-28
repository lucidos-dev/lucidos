let refresh: (() => void) | null = null;

export function provideReleaseCheck(fn: () => void): void {
  refresh = fn;
}

export function refreshReleaseCheckIfShown(): void {
  void refresh?.();
}
