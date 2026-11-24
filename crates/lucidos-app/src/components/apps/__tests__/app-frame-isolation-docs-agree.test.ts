// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { APP_FRAME_ISOLATED } from '../appFrameSandbox';
import { docFamilyText } from '../../../test-doc-family';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '../../../../../..');

const CLAIMS: Array<{ path: string; claim: RegExp; what: string }> = [
  {
    path: 'system-knowhow/js-sdk.md',
    claim: /opaque origin/,
    what: 'the app-author contract, which says a direct fetch is refused',
  },
  {
    path: 'system-knowhow/workspace-audit/apps.md',
    claim: /What an isolated app frame can no longer do/,
    what: 'the audit check that hunts for storage and host-realm reads',
  },
  {
    path: 'docs/glossary.md',
    claim: /### App frame/,
    what: 'the glossary entry the other two point at',
  },
];

describe('the app frame and the docs about it', () => {
  it('is isolated, which is what the corpus tells app authors', () => {
    expect(APP_FRAME_ISOLATED).toBe(true);
  });

  for (const { path, claim, what } of CLAIMS) {
    it(`keeps ${path} saying so`, () => {
      const text = docFamilyText(REPO, path);
      expect(
        claim.test(text),
        `${path} no longer carries ${what}. Either the sandbox changed and the `
        + 'docs have not, or the docs were reworded past the claim.',
      ).toBe(true);
    });
  }
});
