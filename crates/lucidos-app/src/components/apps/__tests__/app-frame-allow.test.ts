// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { APP_FRAME_ALLOW } from '../appFrameSandbox';
import { docFamilyText } from '../../../test-doc-family';

const here = dirname(fileURLToPath(import.meta.url));
const APPS_DIR = resolve(here, '..');
const APP_SRC = resolve(here, '../../..');
const REPO = resolve(here, '../../../../../..');

const TOKENS = APP_FRAME_ALLOW.split(';').map((f: string) => f.trim());

const DELEGATED = ['autoplay', 'fullscreen *', 'encrypted-media', 'clipboard-write'];

const featureName = (token: string) => token.split(/\s+/)[0];

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
      TOKENS,
      'Losing a feature denies it in every app: clipboard-write going missing is '
      + 'how every Copy button died in v0.39.0, silently. Gaining one widens what '
      + 'every app may do. Either way, say why beside the constant and update '
      + 'system-knowhow/js-sdk.md in the same change.',
    ).toEqual(DELEGATED);
  });

  it('delegates fullscreen to every origin, or WebKit denies it', () => {
    expect(
      TOKENS,
      'A bare `fullscreen` reads as `fullscreen \'src\'`, which WebKit never '
      + 'matches against the frame\'s opaque origin. Every app, and the video '
      + 'player widget, then fails with "Fullscreen API is disabled by '
      + 'permissions policy" in the desktop app.',
    ).toContain('fullscreen *');
  });

  it('never delegates clipboard-read, whatever the allowlist syntax', () => {
    expect(
      APP_FRAME_ALLOW,
      'A write is gated on a user gesture and puts the app\'s own text out. A '
      + 'read hands the app whatever the user last copied, which may be a '
      + 'password or a token from another app, for no gesture aimed at the app. '
      + 'Nothing in the SDK needs it. Argue the case in an ADR before adding it.',
    ).not.toMatch(/clipboard-read/);
  });

  it('is what the frame actually carries', () => {
    const src: string = readFileSync(resolve(APPS_DIR, 'AppFrame.tsx'), 'utf8');
    expect(
      src,
      'AppFrame.tsx no longer passes APP_FRAME_ALLOW to the app frame, so the '
      + 'constant above is not what ships and every feature in it is denied.',
    ).toContain('allow={APP_FRAME_ALLOW}');
  });

  it('is the only place the attribute is spelled', () => {
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
    const doc = docFamilyText(REPO, 'system-knowhow/js-sdk.md');
    const missing = DELEGATED.map(featureName).filter((feature) => !doc.includes(`\`${feature}\``));

    expect(
      missing,
      'system-knowhow/js-sdk/remote-workspace-client-contract.md § Setup no longer names every delegated feature. '
      + 'App authors read it to know what their frame can do, so a feature added '
      + 'here and not there is undiscoverable.',
    ).toEqual([]);
  });
});
