import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../FollowToggle.tsx'), 'utf-8');
const pane = readFileSync(resolve(here, '../../layout/ThreadPane.tsx'), 'utf-8');
const row = readFileSync(resolve(here, '../PromptRowControls.tsx'), 'utf-8');
const prompt = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

/**
 * **The follow toggle is shown wherever the composer is, the compose view
 * included, and shows the FOLLOW SEED there.**
 *
 * It shipped hidden in the compose view on the reasoning that there is no
 * transcript to ride. That is true, and is exactly why it had to be there. A
 * new thread is where a reader most reliably wants carrying through the answer.
 * It was also the one place the follow could not be armed.
 *
 * The other half is what keeps the button honest. Over a mounted transcript it
 * must render the LIVE flag, never the seed. The behaviour is pinned in
 * `follow-toggle.test.tsx`. This file pins the render site, which a
 * behavioural test cannot see: a gate added back around it.
 */
describe('the follow toggle is always shown', () => {
  /** The `.prompt-area` element, from its opening tag to its close. */
  function promptArea(): string {
    const match = pane.match(/<div key="prompt" class="prompt-area"[\s\S]*?\n {8}<\/div>/);
    expect(match, '.prompt-area not found in ThreadPane.tsx').not.toBeNull();
    return match![0];
  }

  it('renders inside the composer area, beside the prompt, with no gate', () => {
    // Its offset parent is `.prompt-area`, so it rides a growing prompt and
    // the phone's dynamic bars, and it stands in the compose view too.
    const area = promptArea();
    expect(area).toMatch(/\n {10}<FollowLiveEdgeToggle \/>\n/);
    expect(area).toMatch(/<PromptInput \/>/);
    expect(pane.match(/<FollowLiveEdgeToggle/g)).toHaveLength(1);
  });

  /** It floats above the composer, so forward Tab meets it first. */
  it('comes before the composer in tab order', () => {
    const area = promptArea();
    expect(area.indexOf('<FollowLiveEdgeToggle />')).toBeLessThan(area.indexOf('<PromptInput />'));
  });

  /** Shift+Tab from the prompt walks up the composer in document order: the
   *  compose destination dropdowns first, then the toggle. */
  it('leaves Shift+Tab from the prompt to document order, dropdowns before it', () => {
    expect(prompt).not.toMatch(/shiftKey/);
    expect(prompt.indexOf('<ComposeDestinationRow')).toBeLessThan(prompt.indexOf('class="prompt-textarea"'));
  });

  it('is not gated on being outside the compose view', () => {
    expect(source).not.toMatch(/\{\s*!?\s*composeContext\s*&&/);
    expect(source).toMatch(/data-role="follow-live-edge"/);
  });

  it('renders the seed in compose context and the live follow everywhere else', () => {
    expect(source).toMatch(
      /return isComposeContext\(focusedThread\(\)\) \? followLiveEdgeSeed\.value : followingLiveEdge\.value;/,
    );
  });

  it('imports the seed from scrollState rather than reaching for storage itself', () => {
    expect(source).toMatch(/import\s*\{[^}]*\bfollowLiveEdgeSeed\b[^}]*\}\s*from\s*['"]\.\/scrollState['"]/);
    expect(source).not.toContain('localStorage');
  });

  /** It takes no box in the composer row and never folds into its ⋯. */
  it('is no longer a member of the composer row', () => {
    expect(row).not.toMatch(/follow/i);
    expect(prompt).not.toMatch(/FOLLOW_LIVE_EDGE_KEY|followLiveEdge/);
  });
});
