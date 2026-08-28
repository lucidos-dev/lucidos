/**
 * The picker row's layout: one line of three cells, and the fault note under it.
 *
 * Source scans plus a stylesheet scan, rather than a rendered row. The row is
 * inline markup deep inside `WorkspacePicker`, which owns the gateway control
 * client and eight signals, so standing it up would test the harness. What can
 * go wrong here is not arithmetic, it is four wirings coming apart, and each is
 * visible in the source:
 *
 * 1. The note announces itself. The row carries an `aria-label`, which REPLACES
 *    its content for assistive tech, so a note rendered inside is silent unless
 *    the label folds it in.
 * 2. It announces itself ONCE. The dot's own `aria-label` was the only surface
 *    for the error, and left in place beside a visible note it reads the same
 *    sentence twice.
 * 3. It lands on the name column and cannot squeeze the row's three cells.
 * 4. The line holding those cells never wraps, whatever a workspace is called.
 *
 * The backup line under the name is pinned the same four ways, and for the same
 * reasons. See `docs/plans/2026-08-27-picker-last-successful-backup.md`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from '../../../styles/__tests__/css-rule-helpers';

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
    // Composed once into `rowLabel`, because there are now two sentences to
    // fold in and an inline ternary could carry only the fault.
    expect(source).toMatch(/const rowLabel = \[\s*fault \? `Retry \$\{w\.name\} · \$\{fault\}`/);
    expect(source).toContain('aria-label={rowLabel}');
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
  it('is the second box in the row column, never an item in the line', () => {
    // The row is a column of two boxes: `.ws-picker-line`, and the note under
    // it. The note was once a wrapping item INSIDE the line, which is the bug
    // this pins. A wrapping flex line collects its items by content size,
    // before any shrinking, so a long name pushed the buttons off the line.
    expect(decl(block(pickerCss, '.ws-picker-open {'), 'flex-direction')).toBe('column');

    // Siblings rather than nested, counted from the tags instead of read off
    // the indentation, which a reformat would move. Every `<div>` opened after
    // the line's own is closed again before the note, so the note is outside it.
    // Backwards from the note, since the skeleton draws a row line too and a
    // forward search would start the slice inside that one.
    const to = source.indexOf('{fault && <p class="ws-picker-row-note">');
    expect(to, 'the note is gone').toBeGreaterThanOrEqual(0);
    const from = source.lastIndexOf('<div class="ws-picker-line">', to);
    expect(from, 'the row line is gone').toBeGreaterThanOrEqual(0);
    const between = source.slice(from, to);
    expect(between.match(/<\/div>/g)?.length).toBe(between.match(/<div\b/g)?.length);
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

describe('the backup line rides the stacked name column', () => {
  it('is inside the id cell, so the row keeps its three cells', () => {
    // The line holds dot, id and actions, and stays that way. A fourth item
    // there pushes the buttons off the row. That is the same bug the fault
    // note above was moved out of the line to fix.
    const from = source.lastIndexOf('<div class="ws-picker-id">');
    expect(from, 'the id cell is gone').toBeGreaterThanOrEqual(0);
    const at = source.indexOf('class={`ws-picker-backup', from);
    expect(at, 'the backup line is gone').toBeGreaterThan(from);
    // Every `<div>` opened between the cell and the line closes again. So the
    // line is still inside that cell rather than a sibling of it.
    const between = source.slice(from, at);
    expect(between.match(/<\/div>/g)?.length ?? 0).toBe(
      (between.match(/<div\b/g)?.length ?? 0) - 1,
    );
  });

  it('reads the shared rule rather than re-deriving what stale means', () => {
    // `backupNote` decides the sentence and the level, and is unit-tested in
    // workspace-forms.test.ts. It reads the ENGINE's `stale`, so the 24h
    // threshold lives once, in core::backup.
    expect(source).toContain('backupNote(w)');
    expect(source).not.toMatch(/24 \* 60 \* 60|86400/);
  });

  it('is announced by the row and not a second time by itself', () => {
    // The row's `aria-label` already carries the sentence (see `rowLabel`), so
    // the visible copy is decorative to a screen reader.
    const at = source.indexOf('class={`ws-picker-backup');
    expect(source.slice(at, at + 200)).toContain('aria-hidden="true"');
  });

  it('holds its line open even with nothing to say', () => {
    // The fact lands on the 2s poll, and a workspace the gateway could not ask
    // never gets one. A slot that came and went grew the row under the pointer
    // and pushed every row below it down.
    expect(source).not.toContain('{backup && (');
    expect(source).toContain("{backup?.text ?? ''}");
    const line = block(pickerCss, '.ws-picker-backup {');
    // `min-height` says nothing to a non-replaced inline, which a <span> is.
    expect(decl(line, 'display')).toBe('block');
    expect(decl(line, 'line-height')).toBe('var(--ws-picker-sub-line-height)');
    // Reserved from the line it holds open, rather than from a second copy of
    // the same two numbers.
    const reserved = decl(line, 'min-height') ?? '';
    expect(reserved).toContain('var(--font-size-sm)');
    expect(reserved).toContain('var(--ws-picker-sub-line-height)');
    expect(decl(line, 'font-size')).toBe('var(--font-size-sm)');
    expect(decl(block(pickerCss, ':root'), '--ws-picker-sub-line-height')).not.toBeNull();
  });

  it('warns in the row\'s own coral, quiet otherwise', () => {
    // One colour for "needs attention" on this surface, stated once on :root
    // and worn by the dot, the fault note and this.
    expect(decl(block(pickerCss, '.ws-picker-backup-warn {'), 'color'))
      .toBe('var(--ws-picker-fault-color)');
    expect(decl(block(pickerCss, '.ws-picker-backup {'), 'color'))
      .not.toBe('var(--ws-picker-fault-color)');
  });
});

describe('the row line holds its three cells on one line', () => {
  it('never wraps, so a long name cannot push the actions off the row', () => {
    // A workspace can carry a long name AND a long `/slug/` address, and a
    // wrapping line puts the buttons under them. Read every rule that styles
    // the line, not just the first: an `@media` copy below it would re-enable
    // wrapping where a first-match scan never looks.
    const rules = rulesTargeting(pickerCss, 'ws-picker-line');
    expect(rules.length, 'the row line has no rule at all').toBeGreaterThan(0);
    for (const rule of rules) {
      const wrap = rule.props.get('flex-wrap');
      expect(wrap ?? 'nowrap', `${rule.selector} ${rule.atRules}`).toBe('nowrap');
    }
  });

  it('spells the name whole rather than truncating it', () => {
    // The name is the only thing telling two workspaces apart. Truncating it
    // made rows sharing a prefix read as the same row, so it wraps instead.
    const name = block(pickerCss, '.ws-picker-name {');
    expect(decl(name, 'text-overflow')).toBeNull();
    expect(decl(name, 'white-space')).toBeNull();
    expect(decl(name, 'overflow-wrap')).toBe('anywhere');

    // What shrinks in the line is the name+address CELL, never the name.
    const id = block(pickerCss, '.ws-picker-id {');
    expect(decl(id, 'min-width')).toBe('0');
    expect(decl(id, 'flex-direction')).toBe('column');
    expect(decl(block(pickerCss, '.ws-picker-actions {'), 'flex')).toBe('0 0 auto');
  });

  it('keeps the address whole too, since it is the other tiebreaker', () => {
    // `showsAddress` puts it on screen exactly when the name does not settle
    // the question, so an ellipsised address answers nothing.
    const addr = block(pickerCss, '.ws-picker-address {');
    expect(decl(addr, 'text-overflow')).toBeNull();
    expect(decl(addr, 'white-space')).toBeNull();
  });

  it('lands the dot on the first line of the name, by derivation', () => {
    // Centred, it drifts down beside line two of a wrapped name. The offset is
    // computed from the name's own line-height so the two cannot part.
    const scoped = block(pickerCss, '.ws-picker-line .ws-picker-dot {');
    expect(decl(scoped, 'align-self')).toBe('flex-start');
    const offset = decl(scoped, 'margin-top') ?? '';
    expect(offset).toContain('var(--ws-picker-name-line-height)');
    expect(offset).toContain('var(--ws-picker-dot-size)');
    expect(decl(block(pickerCss, ':root'), '--ws-picker-name-line-height')).not.toBeNull();
  });

  it('tops the name cell out with the dot, so the offset has one origin', () => {
    // The dot's offset above measures from the LINE's top. Centred, this cell
    // drops down the line whenever the actions are the taller item, so the dot
    // floats above the name it marks. That is what a row with nothing under
    // its name looked like.
    expect(decl(block(pickerCss, '.ws-picker-id {'), 'align-self')).toBe('flex-start');
  });

  it('leaves the SHARED dot centred, for the in-app switcher', () => {
    // The workspace switcher in the Lucidos menu wears `.ws-picker-dot` too,
    // and its rows are one centred line. Only a rule scoped to the picker's row
    // line may move the dot, or every menu row goes askew.
    for (const rule of rulesTargeting(pickerCss, 'ws-picker-dot')) {
      if (!rule.props.has('align-self') && !rule.props.has('margin-top')) continue;
      expect(rule.selector, 'an unscoped rule moves the shared dot')
        .toContain('.ws-picker-line ');
    }
  });

  it('is what the skeleton row draws too', () => {
    // The placeholder renders the real markup, so a skeleton that skipped
    // either box would stack its cells wrongly and mis-size every row.
    expect(source.match(/<div class="ws-picker-line">/g)?.length).toBe(2);
    expect(source.match(/<div class="ws-picker-id">/g)?.length).toBe(2);
  });
});
