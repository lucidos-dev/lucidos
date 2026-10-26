/**
 * The action slot keeps one width per face set, in every state (ADR 0573).
 * Every control that can stand in the slot wears `.action-slot`, and that class
 * names one width per face set: the button, or the round button under
 * `data-composer-compact`. Every face's word is short enough for the button.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (path: string): string => readFileSync(resolve(here, path), 'utf-8');
const strip = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, '');
const hostCss = strip(read('../../../styles/global/host-components.css'));
const rowCss = strip(read('../../../styles/chat/input-messages.css'));
const prompt = read('../PromptInput.tsx');
const banner = read('../WaitingBanner.tsx');

/** The rules scoped to the compact face, with the attribute prefix dropped. */
const compactRules = (css: string) =>
  [...css.matchAll(/(?:^|\n)((?:\[data-composer-compact\][^{}]*?)) \{([^}]*)\}/g)]
    .map((m) => `${m[1].split('[data-composer-compact] ').join('')} {${m[2]}}`).join('\n');

describe('the action slot keeps one width per face set', () => {
  it('sizes the desktop button from one token, beside the round size', () => {
    expect(rowCss).toMatch(/\.prompt-actions-right \{[^}]*--send-round-size: [\d.]+rem;[^}]*--action-slot-width: [\d.]+rem;/);
    expect(hostCss).toMatch(/\n\.action-slot \{[^}]*width: var\(--action-slot-width\);/);
    expect(hostCss).toMatch(/\.split-button:has\(> \.action-slot\) \{\s*width: var\(--action-slot-width\);/);
  });

  it('draws the round button on the compact face, split or not', () => {
    expect(compactRules(hostCss)).toMatch(
      /\.action-slot,\s*\.split-button:has\(> \.action-slot\) \{\s*width: var\(--send-round-size\);/,
    );
  });

  it('dresses every control that stands in the slot as one', () => {
    // Send and Stop, and the three lone answer faces.
    expect(prompt).toContain("'action-btn send-cancel-morph action-slot'");
    expect(prompt.match(/class="action-btn action-btn-(confirm|danger) action-slot"/g)?.length).toBe(3);
    // Send or Stop wins whenever it stands, so a banner never hides Stop.
    expect(prompt).toContain('sendButton ?? (bannerActions && <ActionSlot occupant={bannerActions.slot} compact={composerCompact} />)');
  });
});

it('keeps the face rules out of the phone media query', () => {
  const phoneBlocks = [...hostCss.matchAll(/@media \(--phone-layout\) \{[\s\S]*?\n\}/g)].map((m) => m[0]).join('\n');
  expect(phoneBlocks).not.toContain('.action-slot');
});

describe('every face fits the button', () => {
  /** The closed set of words a face can show. A new one must fit the button at
   *  every UI scale, which `composer-action-slot-widths-all.spec.ts` checks. */
  const FACE_WORDS = [
    'Send', 'Ask', 'Stop', 'Submit', 'Cancel',
    'Apply', 'Discard', 'Archive', 'Landing', 'Waiting', 'Bring back',
  ];

  it('gives the busy and lane faces their plain word', () => {
    for (const word of ['Apply', 'Discard', 'Archive']) {
      expect(banner).toMatch(new RegExp(`busyAction\\('[a-z]+', '${word}'`));
    }
    expect(banner).toContain("{ 'in-batch': 'Landing', waiting: 'Waiting' }");
    expect(banner).not.toMatch(/'(Apply|Discard|Archive)\.\.\.'/);
  });

  it('labels each answer face with a word from the set', () => {
    const labels = [...prompt.matchAll(/<SlotFace .*?label="([^"]+)"/g)].map((m) => m[1]);
    expect(labels).toEqual(['Cancel', 'Cancel', 'Stop']);
    expect(prompt).toContain("label={submitLabel}");
    expect(prompt).toContain(": 'Submit';");
    for (const label of labels) expect(FACE_WORDS).toContain(label);
    expect(prompt).toContain('`Submit ${submitMultiCount}`');
    expect(prompt).not.toContain('Canceling…');
  });

  it('keeps every word short', () => {
    expect(Math.max(...FACE_WORDS.map((w) => w.length))).toBeLessThanOrEqual(10);
  });
});
