/** The entry reaches release checks through a facade registered by startup.
 * The facade starts fresh snapshots after paint and forwards later resumes.
 * See *release check handle* in `docs/glossary.md`. */
let refresh: (() => void) | null = null;

/** Called by client startup with its deferred background refresh. */
export function provideReleaseCheck(fn: () => void): void {
  refresh = fn;
}

/** Re-read the release answer once startup has provided the facade. */
export function refreshReleaseCheckIfShown(): void {
  void refresh?.();
}
