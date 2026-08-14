/**
 * The picker row's fault note: the red dot's reason, on the row it is about.
 *
 * Source scans plus a stylesheet scan, rather than a rendered row. The row is
 * inline markup deep inside `WorkspacePicker`, which owns the gateway control
 * client and eight signals, so standing it up would test the harness. What can
 * go wrong here is not arithmetic, it is three wirings coming apart, and each is
 * visible in the source:
 *
 * 1. The note announces itself. The row carries an `aria-label`, which REPLACES
 *    its content for assistive tech, so a note rendered inside is silent unless
 *    the label folds it in.
 * 2. It announces itself ONCE. The dot's own `aria-label` was the only surface
 *    for the error, and left in place beside a visible note it reads the same
 *    sentence twice.
 * 3. It lands on the name column and cannot squeeze the row's three cells.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../WorkspacePicker.tsx'), 'utf-8');
const pickerCss: string = readFileSync(
  resolve(here, '../../../styles/picker.css'),
  'utf-8',
);

describe('the fault note is wired to the one state that is a fault', () => {
  it('reads the shared predicate rather than testing the state word again', () => {
    // `workspaceFaultNote` is where "which state owes an explanation" is
    // decided, and it is unit-tested in utils/workspaceState.test.ts. A row
    // re-deriving it with its own `=== 'unhealthy'` is how the two drift.
    expect(source).toContain('workspaceFaultNote(w)');
    expect(source).toContain('<p class="ws-picker-row-note">{fault}</p>');
  });
});

describe('the note is announced, exactly once', () => {
  it('the row folds the fault into its own accessible name', () => {
    // An `aria-label` on the row replaces everything inside it, note included.
    expect(source).toMatch(/aria-label=\{fault \? `Retry \$\{w\.name\} · \$\{fault\}`/);
  });

  it('the dot steps aside when the note is on screen', () => {
    // Its label is the fallback for the states that render no note; with the
    // sentence visible in the row it would only be read out a second time.
    expect(source).toContain('aria-label={fault ? undefined : workspaceStateLabel(w)}');
    expect(source).toContain("aria-hidden={fault ? 'true' : undefined}");
  });

  it('the dot keeps its hover tooltip for every state', () => {
    // The tooltip is the only surface a healthy or booting row has for the
    // label, and the note does not replace it.
    expect(source).toContain('data-tooltip={workspaceStateLabel(w)}');
  });
});

describe('the note takes its own line, on the name column', () => {
  it('claims a full line so the row cells keep theirs', () => {
    const note = block(pickerCss, '.ws-picker-row-note {');
    expect(decl(note, 'flex')).toBe('1 0 100%');
    expect(decl(block(pickerCss, '.ws-picker-open {'), 'flex-wrap')).toBe('wrap');
  });

  it('indents to the name column by derivation, not by a guess', () => {
    // The name starts one dot plus one gap in. Copies of either number would
    // put the note off that column the moment the row is retuned.
    const indent = decl(block(pickerCss, '.ws-picker-row-note {'), 'padding-left') ?? '';
    expect(indent).toContain('var(--ws-picker-dot-size)');
    expect(indent).toContain('var(--ws-picker-row-gap)');
    const rootTokens = block(pickerCss, ':root');
    for (const name of ['--ws-picker-dot-size', '--ws-picker-row-gap', '--ws-picker-note-gap']) {
      expect(decl(rootTokens, name), `${name} is read but declared nowhere`).not.toBeNull();
    }
  });

  it('wraps rather than truncating the gateway error', () => {
    // Its length is the gateway's to decide, and half a reason is not a reason.
    const note = block(pickerCss, '.ws-picker-row-note {');
    expect(decl(note, 'overflow-wrap')).toBe('anywhere');
    expect(decl(note, 'text-overflow')).toBeNull();
  });

  it('wears the same coral as the dot it explains, stated once', () => {
    expect(decl(block(pickerCss, '.ws-picker-row-note {'), 'color'))
      .toBe('var(--ws-picker-fault-color)');
    expect(decl(block(pickerCss, '.ws-picker-dot-unhealthy {'), 'background'))
      .toBe('var(--ws-picker-fault-color)');
  });
});
