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

describe('the follow toggle is always shown', () => {
  function promptArea(): string {
    const match = pane.match(/<div key="prompt" class="prompt-area"[\s\S]*?\n {8}<\/div>/);
    expect(match, '.prompt-area not found in ThreadPane.tsx').not.toBeNull();
    return match![0];
  }

  it('renders inside the composer area, beside the prompt, with no gate', () => {
    const area = promptArea();
    expect(area).toMatch(/\n {10}<FollowLiveEdgeToggle \/>\n/);
    expect(area).toMatch(/<PromptInput \/>/);
    expect(pane.match(/<FollowLiveEdgeToggle/g)).toHaveLength(1);
  });

  it('comes before the composer in tab order', () => {
    const area = promptArea();
    expect(area.indexOf('<FollowLiveEdgeToggle />')).toBeLessThan(area.indexOf('<PromptInput />'));
  });

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

  it('is no longer a member of the composer row', () => {
    expect(row).not.toMatch(/follow/i);
    expect(prompt).not.toMatch(/FOLLOW_LIVE_EDGE_KEY|followLiveEdge/);
  });
});
