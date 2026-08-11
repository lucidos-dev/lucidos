import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../PromptRowControls.tsx'), 'utf-8');

/**
 * **The follow toggle renders wherever the composer does, the compose view
 * included, and shows the FOLLOW SEED there.**
 *
 * It shipped hidden in the compose view on the reasoning that there is no
 * transcript to ride, which is true and is exactly why it had to be there: a
 * brand-new thread is where a reader most reliably knows they want to be carried
 * through the answer, and it was the one place the follow could not be armed at
 * all. The compose press records the seed, and the thread that compose becomes
 * starts from it by having no *reading position* of its own.
 *
 * The other half is what keeps the button honest. Over a mounted transcript it
 * must render the LIVE flag, never the seed, or it would sit lit on a thread
 * whose follow the reader's own scroll had already retired.
 *
 * A source scan, like the rest of this component's tests: rendering the prompt
 * row pulls in the whole compose surface, and the failure this guards against is
 * an edit to these few lines rather than a behaviour the store can produce.
 */
describe('the follow toggle is always shown', () => {
  /** The button and the line that resolves its state. */
  function toggleBlock(): string {
    const match = source.match(/const followOn =[\s\S]*?<\/button>/);
    expect(match, 'follow-live-edge button not found in PromptRowControls.tsx').not.toBeNull();
    return match![0];
  }

  it('is not gated on being outside the compose view', () => {
    // The exact shape that hid it. A behavioural test cannot fail for a gate
    // somebody adds back, so the gate is what is checked.
    expect(source).not.toMatch(/\{\s*!?\s*composeContext\s*&&\s*\(?\s*<button/);
    expect(toggleBlock()).toContain('data-role="follow-live-edge"');
  });

  it('renders the seed in compose context and the live follow everywhere else', () => {
    expect(toggleBlock()).toMatch(
      /const followOn = composeContext \? followLiveEdgeSeed\.value : followingLiveEdge\.value;/,
    );
  });

  // Its fixed SECOND slot in the row is pinned behaviourally next door, in
  // `prompt-row-controls.test.tsx`, which walks the rendered cluster rather
  // than the source.

  it('drives every visual and accessible affordance off that one resolved state', () => {
    // Three places said `followingLiveEdge.value` before, and a partial edit
    // would leave the fill, the tooltip and the pressed state disagreeing.
    const block = toggleBlock();
    expect(block).toMatch(/\$\{followOn \? ' active' : ''\}/);
    expect(block).toMatch(/aria-pressed=\{followOn\}/);
    expect(block).toMatch(/aria-label=\{followOn \?/);
    expect(block).toMatch(/data-tooltip=\{followOn$/m);
    expect(block).toMatch(/onClick=\{\(\) => setFollowLiveEdge\(!followOn\)\}/);
  });

  it('imports the seed from scrollState rather than reaching for storage itself', () => {
    expect(source).toMatch(/import\s*\{[^}]*\bfollowLiveEdgeSeed\b[^}]*\}\s*from\s*['"]\.\/scrollState['"]/);
    expect(toggleBlock()).not.toContain('localStorage');
  });
});
