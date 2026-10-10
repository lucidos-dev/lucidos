/**
 * The app iframe's `sandbox` and `allow`, and the one question the rest of the
 * shell asks about them.
 *
 * They live alone because three places must agree: the frame that carries the
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

/**
 * What the shell delegates to an app frame, as a permissions policy.
 *
 * Every feature here defaults to an allowlist of `self`, and the opaque origin
 * above is not `self`. So each one is DENIED unless this attribute hands it
 * over. That is why the list exists at all: while the frame shared the shell's
 * origin, `self` covered it and the attribute was a formality.
 *
 * - `autoplay`, `fullscreen`, `encrypted-media`: an app playing media.
 * - `clipboard-write`: every Copy button in every app. Without it Chromium
 *   rejects `writeText` with `NotAllowedError` and the text never lands, which
 *   is silent because app code does not await the promise. WebKit allows the
 *   write either way.
 *
 * `clipboard-read` is deliberately absent. A write is gated on a user gesture
 * and puts the app's own text out. A read hands the app whatever the user last
 * copied. That may be a password or a token from another app, for no gesture
 * aimed at the app. Nothing in the SDK needs it.
 */
export const APP_FRAME_ALLOW = 'autoplay; fullscreen; encrypted-media; clipboard-write';
