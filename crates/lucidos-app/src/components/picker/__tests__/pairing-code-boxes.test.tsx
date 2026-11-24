import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { PairingCodeBoxes, applyCodeInput, codeSlots, digitsOnly } from '../PairingGate';
import { PAIRING_CODE_DIGITS } from '../../../utils/pairingCodeSeed';
import { findByClass } from '../../layout/__tests__/vnodeWalk';

const here = dirname(fileURLToPath(import.meta.url));
const gateSrc: string = readFileSync(resolve(here, '../PairingGate.tsx'), 'utf8');

function boxes(code: string) {
  return findByClass(PairingCodeBoxes({ code }), 'pairing-code-box');
}

describe('one box per digit of the code', () => {
  it('draws exactly as many boxes as the gateway mints digits', () => {
    expect(boxes('').length).toBe(PAIRING_CODE_DIGITS);
    expect(boxes('4711').length).toBe(PAIRING_CODE_DIGITS);
  });

  it('starts empty, so the field asks rather than answers', () => {
    expect(codeSlots('').every((s) => s.digit === '')).toBe(true);
  });

  it('fills boxes left to right as the digits arrive', () => {
    const slots = codeSlots('4711');
    expect(slots.map((s) => s.digit)).toEqual(['4', '7', '1', '1', '', '', '', '']);
  });

  it('marks the box the next digit lands in, and draws a caret in it', () => {
    const slots = codeSlots('471');
    expect(slots.findIndex((s) => s.active)).toBe(3);
    expect(slots.filter((s) => s.active).length).toBe(1);
    expect(slots[3].caret).toBe(true);
  });

  it('keeps the caret off a box that holds a digit', () => {
    const full = codeSlots('47118899');
    expect(full[PAIRING_CODE_DIGITS - 1].active).toBe(true);
    expect(full.some((s) => s.caret)).toBe(false);
  });

  it('hides the boxes from assistive tech, which reads the input itself', () => {
    const drawn = PairingCodeBoxes({ code: '' });
    expect(drawn.props['aria-hidden']).toBe('true');
  });
});

describe('what reaches the field', () => {
  it('keeps the digits out of a pasted code and drops the rest', () => {
    expect(digitsOnly('4711 8899')).toBe('47118899');
    expect(digitsOnly('code: 4711-8899.')).toBe('47118899');
  });

  it('never overfills the boxes', () => {
    expect(digitsOnly('471188990000').length).toBe(PAIRING_CODE_DIGITS);
  });

  it('caps the length AFTER the punctuation goes, so a spaced paste survives', () => {
    const el = { value: '4711 8899' };
    expect(applyCodeInput(el)).toBe('47118899');
    expect(el.value).toBe('47118899');
  });

  it('writes the field back, since a rejected key re-renders nothing', () => {
    const el = { value: '4711x' };
    expect(applyCodeInput(el)).toBe('4711');
    expect(el.value).toBe('4711');
  });

  it('leaves a field that is already clean alone', () => {
    const el = { value: '4711' };
    expect(applyCodeInput(el)).toBe('4711');
    expect(el.value).toBe('4711');
  });
});

describe('the code field takes no native length cap', () => {
  it('carries no maxLength, which would truncate before sanitizing', () => {
    expect(gateSrc).not.toMatch(/maxLength=/);
  });
});

describe('the form itself', () => {
  it('offers no placeholder in either field', () => {
    expect(gateSrc).not.toMatch(/placeholder=/);
  });

  it('suggests a device name rather than describing one', () => {
    expect(gateSrc).toMatch(/useState\(\(\) => suggestDeviceLabelHere\(\) \?\? ''\)/);
  });
});
