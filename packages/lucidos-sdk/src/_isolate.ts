/** Run one listener on its own.
 *
 *  A listener that throws must not cost the others their call, the SDK's own
 *  theme listener among them. Its error is rethrown on a fresh task, so it
 *  still reaches the console as uncaught rather than vanishing. */
export function runIsolated(run: () => void): void {
  try {
    run();
  } catch (err) {
    setTimeout(() => { throw err; });
  }
}
