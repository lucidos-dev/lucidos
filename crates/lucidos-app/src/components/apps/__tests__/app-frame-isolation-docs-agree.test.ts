/**
 * The docs that describe an isolated app frame, pinned against the sandbox.
 *
 * `.claude/rules/system-knowhow.md` asks a change to this sandbox to carry the
 * docs with it, and a rule is a judgment nothing runs. This is the half a
 * script can hold: the corpus now tells app authors their own `fetch`,
 * `localStorage` and `EventSource` do not work, and the workspace audit hunts
 * for code that uses them. Put `allow-same-origin` back and every one of those
 * becomes wrong in the other direction, silently, because the apps start
 * working again.
 *
 * It asserts agreement rather than the attribute's spelling. Adding a token
 * (`allow-presentation`, say) is free. Adding the one that un-isolates the
 * frame is what has to come with the corpus.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { APP_FRAME_ISOLATED } from '../appFrameSandbox';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '../../../../../..');

/** Each doc, and the claim it makes that only isolation makes true. */
const CLAIMS: Array<{ path: string; claim: RegExp; what: string }> = [
  {
    path: 'system-knowhow/js-sdk.md',
    claim: /opaque origin/,
    what: 'the app-author contract, which says a direct fetch is refused',
  },
  {
    path: 'system-knowhow/workspace-audit.md',
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
      const text: string = readFileSync(resolve(REPO, path), 'utf8');
      expect(
        claim.test(text),
        `${path} no longer carries ${what}. Either the sandbox changed and the `
        + 'docs have not, or the docs were reworded past the claim.',
      ).toBe(true);
    });
  }
});
