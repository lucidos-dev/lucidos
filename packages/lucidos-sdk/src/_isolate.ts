export function runIsolated(run: () => void): void {
  try {
    run();
  } catch (err) {
    setTimeout(() => { throw err; });
  }
}
