/**
 * The engine serves stylesheets that Vite never sees, and nothing else parses
 * them.
 *
 * `crates/lucidos-engine/src/api/sdk_iframe.css` is `include_str!`d by
 * `api/sdk.rs` and served at `/api/v1/sdk-iframe.css` to every app iframe.
 * Because it is embedded as a string rather than imported, no build step reads
 * it: `cargo build` treats it as opaque bytes, `tsc` ignores CSS, and it is
 * outside the Vite graph, so `vite build` never sees it either. A syntax error
 * in it therefore compiles, ships, and only shows up as app chrome that quietly
 * stops being styled (a browser recovers from a bad declaration by skipping to
 * the next `}`, and an unbalanced comment swallows everything up to the next
 * comment terminator).
 *
 * The app's own `src/styles/**` tree is deliberately NOT covered here: it goes
 * through `vite build`, which parses it with the real `postcss-import`
 * pipeline and so also catches things a per-file parse cannot (an `@import`
 * pointing at nothing). Duplicating it would be a weaker check in a second
 * place. `/harden`'s test-selection table routes each surface to its own gate.
 *
 * Precedent for why this is worth a test: on 2026-08-05 a stray comment terminator in
 * `styles/global/host-components.css` closed a comment three paragraphs early,
 * `vite build` failed on it, and the shared build-watch spent the day serving a
 * stale `dist/` while every frontend Apply stranded with "applied but not
 * served yet". That file is inside the Vite tree, so `vite build` now gates it;
 * this test is the same gate for the one stylesheet `vite build` cannot reach.
 */
import { describe, it, expect } from 'vitest';
import postcss from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/styles/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../..');

/** Stylesheets the engine embeds and serves, relative to the repo root. Add a
 *  new one here in the same change that adds the `include_str!`. */
const ENGINE_SERVED_CSS = [
  'crates/lucidos-engine/src/api/sdk_iframe.css',
  'crates/lucidos-engine/src/api/sdk_fonts_fira_code.css',
] as const;

describe('engine-served stylesheets', () => {
  for (const relPath of ENGINE_SERVED_CSS) {
    it(`parses: ${relPath}`, () => {
      const path = resolve(REPO_ROOT, relPath);
      const css = readFileSync(path, 'utf8');
      let error: string | null = null;
      try {
        postcss.parse(css, { from: path });
      } catch (e) {
        // postcss throws CssSyntaxError, whose message already carries
        // file:line:column and the reason. Its stack dump is enormous, so
        // surface just the message.
        error = e instanceof Error ? e.message : String(e);
      }
      expect(
        error,
        `${relPath} is served to every app iframe but is parsed by no build step, `
        + 'so a syntax error here ships silently. Fix the CSS.',
      ).toBeNull();
    });
  }
});
