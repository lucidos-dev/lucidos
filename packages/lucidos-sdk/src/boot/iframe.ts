/**
 * Entry point for the app-iframe boot script, served by the engine as
 * `/api/v1/sdk-prefs.js` (`crates/lucidos-engine/src/api/sdk_fonts.rs`'s sibling,
 * `api/sdk_prefs.rs`, `include_str!`s the built bundle).
 *
 * Nothing but the shared program: an iframe has no boot splash to paint and no
 * telemetry channel of its own, and `?style-reset` is the shell's escape hatch,
 * already honoured before the iframe loads.
 */
import { applyAppearanceBoot } from './appearanceBoot';

applyAppearanceBoot({ styleReset: false });
