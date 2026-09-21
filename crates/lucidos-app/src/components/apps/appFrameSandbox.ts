/**
 * The app iframe's `sandbox`, and the one question the rest of the shell asks
 * about it.
 *
 * It lives alone because three places must agree: the frame that carries the
 * attribute, the navigation that drives the frame, and the capture that reads
 * it. Deriving the answer from the attribute means there is one thing to
 * change, and no second place to forget.
 */

/**
 * What an app frame may do.
 *
 * `allow-same-origin` is absent on purpose, and it is the whole point. With it
 * the frame shares the shell's renderer process, so an app that saturates its
 * main thread freezes the whole Lucidos tab. Measured: 41 heartbeat ticks
 * against 280 due, a 12 second gap. Without it the frame gets its own process
 * and the shell is untouched.
 *
 * It also stops an app reading the shell's DOM, its storage and the device id
 * inside it. See `docs/plans/2026-09-19-an-app-frame-cannot-starve-the-shell.md`.
 */
export const APP_FRAME_SANDBOX =
  'allow-scripts allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox allow-downloads';

/**
 * Is an app frame a foreign realm this document cannot reach into?
 *
 * Derived rather than declared, so it cannot disagree with the attribute above.
 * When true, `contentWindow.location` and `contentWindow.lucidos` both throw,
 * and the host asks the frame over `postMessage` instead.
 */
export const APP_FRAME_ISOLATED = !APP_FRAME_SANDBOX.includes('allow-same-origin');
