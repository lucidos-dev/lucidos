/**
 * The finder the host stamps into an HTML artifact preview.
 *
 * The preview is an opaque srcdoc frame that loads no SDK (ADR 0322), so the
 * matcher arrives as text. `previewBundles.build.mjs` bundles this entry into a
 * committed IIFE. The host's preview bridge runs it inside its own closure, so
 * the artifact never sees it as a global.
 */
export { serveFind as serve } from '../find';
