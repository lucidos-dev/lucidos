/**
 * What the app frame delegates, pinned so the list cannot drift unseen.
 *
 * The frame has an opaque origin, so a permissions-policy feature defaults to
 * an allowlist of `self` that the frame does not match. Each one is denied
 * unless `APP_FRAME_ALLOW` hands it over, in every app and every workspace.
 *
 * That is not theoretical. `clipboard-write` shipped missing in v0.39.0, so
 * every Copy button in every app was dead, and silent: app code does not await
 * `writeText`, so the app toasted "Copied" on a copy that never happened.
 *
 * Both directions are pinned. A feature LOST is that bug again, and a feature
 * GAINED widens what every app may do.
 *
 * The browser half is `e2e/app-frame-can-copy-desktop.spec.ts`, a real copy in
 * a real frame. This is the floor under it, and runs in the per-change gate.
 * Background: `docs/plans/2026-09-21-the-app-frame-delegates-clipboard-write.md`.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { APP_FRAME_ALLOW } from '../appFrameSandbox';

const here = dirname(fileURLToPath(import.meta.url));
/** `crates/lucidos-app/src/components/apps/`, which holds the frame. */
const APPS_DIR = resolve(here, '..');
/** `crates/lucidos-app/src`, the whole client. */
const APP_SRC = resolve(here, '../../..');
/** The repo root, for the app-author doc this list is published in. */
const REPO = resolve(here, '../../../../../..');

/** The `allow` attribute is semicolon-separated, so these are its tokens. */
const FEATURES = APP_FRAME_ALLOW.split(';').map((f: string) => f.trim());

/** What the shell delegates, in order. Adding a row here is the decision. */
const DELEGATED = ['autoplay', 'fullscreen', 'encrypted-media', 'clipboard-write'];

/** Every `.tsx` under `src`, so a frame that moves cannot escape the scan. */
function tsxFiles(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) found.push(...tsxFiles(full));
    else if (entry.name.endsWith('.tsx')) found.push(full);
  }
  return found;
}

describe('what the app frame delegates', () => {
  it('is exactly this list, no more and no less', () => {
    expect(
      FEATURES,
      'Losing a feature denies it in every app: clipboard-write going missing is '
      + 'how every Copy button died in v0.39.0, silently. Gaining one widens what '
      + 'every app may do. Either way, say why beside the constant and update '
      + 'system-knowhow/js-sdk.md in the same change.',
    ).toEqual(DELEGATED);
  });

  it('never delegates clipboard-read, whatever the allowlist syntax', () => {
    // Matched against the whole string, not the token list: `clipboard-read *`
    // is one token and would slip past an exact-element check.
    expect(
      APP_FRAME_ALLOW,
      'A write is gated on a user gesture and puts the app\'s own text out. A '
      + 'read hands the app whatever the user last copied, which may be a '
      + 'password or a token from another app, for no gesture aimed at the app. '
      + 'Nothing in the SDK needs it. Argue the case in an ADR before adding it.',
    ).not.toMatch(/clipboard-read/);
  });

  it('is what the frame actually carries', () => {
    // The scan below catches a SECOND spelling. Nothing but this catches the
    // attribute being dropped, which denies every feature just as completely.
    const src: string = readFileSync(resolve(APPS_DIR, 'AppUiInline.tsx'), 'utf8');
    expect(
      src,
      'AppUiInline.tsx no longer passes APP_FRAME_ALLOW to the app frame, so the '
      + 'constant above is not what ships and every feature in it is denied.',
    ).toContain('allow={APP_FRAME_ALLOW}');
  });

  it('is the only place the attribute is spelled', () => {
    // A bare inline string on a JSX element is how the missing feature went
    // unreviewed: there was nowhere for the invariant to live. The scan covers
    // the whole client, not just this directory, so moving the frame does not
    // move it out of range.
    const offenders = tsxFiles(APP_SRC)
      .filter((path: string) => /\ballow\s*=\s*["'`]/.test(readFileSync(path, 'utf8')))
      .map((path: string) => relative(APP_SRC, path));

    expect(
      offenders,
      'A permissions-policy attribute is written as a literal here. Import '
      + 'APP_FRAME_ALLOW from components/apps/appFrameSandbox.ts instead, so one '
      + 'edit reaches the frame, this test and the docs together.',
    ).toEqual([]);
  });

  it('is what system-knowhow tells app authors', () => {
    // js-sdk.md is the one home for this list, and the engine LLM reads it as
    // fact. `.claude/rules/system-knowhow.md` asks a change here to carry that
    // file, and a rule is a judgment nothing runs. This is the half a script
    // can hold.
    const doc: string = readFileSync(resolve(REPO, 'system-knowhow/js-sdk.md'), 'utf8');
    const missing = DELEGATED.filter((feature) => !doc.includes(`\`${feature}\``));

    expect(
      missing,
      'system-knowhow/js-sdk.md § Setup no longer names every delegated feature. '
      + 'App authors read it to know what their frame can do, so a feature added '
      + 'here and not there is undiscoverable.',
    ).toEqual([]);
  });
});
