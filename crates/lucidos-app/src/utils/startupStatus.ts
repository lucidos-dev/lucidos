/**
 * Narrating the packaged desktop start, on the splash the window shows before
 * `desktop::launch()` has navigated it anywhere.
 *
 * That splash paints on Tauri's bundled asset scheme, where every API call and
 * the service-worker registration throw WebKit's "string did not match the
 * expected pattern", so it can reach no HTTP surface at all. Tauri IPC is the
 * only channel it has, and until this module existed it used it for nothing but
 * a heartbeat: the status line was set once, to a fixed string, and then sat
 * there for however long the start ran. A boot that was slow but recovering
 * (`Starting Lucidos…` for over two minutes while the gateway retried behind a
 * network interface that had not come up yet) was therefore indistinguishable
 * from a hang.
 *
 * The wording is NOT composed here. `desktop::startup_label` builds the whole
 * line in Rust, where the phase, the elapsed time and the last failure actually
 * live, and this module is the pipe that carries it to the DOM. One home for the
 * copy, and it is unit-tested there.
 */

/** How often to ask. A second is cheap over local IPC on a machine that is by
 *  definition sitting idle waiting, and it is frequent enough that the elapsed
 *  counter in the label reads as a clock rather than as a stutter. */
export const STARTUP_STATUS_POLL_MS = 1_000;

/** The seam, so the poller is testable without a Tauri bridge or a real clock. */
export interface StartupStatusDeps {
  /** Tauri `invoke`, narrowed to the one command this needs. */
  invoke: (cmd: 'startup_status') => Promise<unknown>;
  /** Where the answer goes (`setBootStatus` in production). */
  setStatus: (text: string) => void;
  /** `window.setInterval`, injectable so a test can drive the ticks. */
  setInterval: (fn: () => void, ms: number) => number;
}

/**
 * Poll the desktop startup status and push each label onto the boot splash.
 * Asks once immediately, so a start that is ALREADY slow (the window opened
 * late, after the service had been struggling for a minute) says so on the
 * first frame instead of a second later.
 *
 * Returns the interval id, so a caller that ever needs to stop can.
 */
export function startStartupStatusPolling(deps: StartupStatusDeps): number {
  const tick = () => {
    void deps
      .invoke('startup_status')
      .then((label) => {
        // A non-string means an older desktop binary than this bundle, which is
        // possible mid-update. Leave the splash on whatever it already says
        // rather than blanking it or printing "undefined".
        if (typeof label === 'string' && label.length > 0) deps.setStatus(label);
      })
      // Best-effort telemetry the user did not initiate, on a surface where no
      // toast can render (there is no app mounted, only the inline splash) and
      // where the next tick retries a second later anyway. `invoke` already
      // reports a dead IPC bridge to the engine log on its own
      // (utils/ipcHealth), which is where a genuinely broken bridge surfaces.
      .catch(() => {});
  };
  tick();
  return deps.setInterval(tick, STARTUP_STATUS_POLL_MS);
}
