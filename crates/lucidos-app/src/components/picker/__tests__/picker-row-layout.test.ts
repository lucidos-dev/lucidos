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

const NOTE = `<p class={w.delete_interrupted ? 'ws-picker-row-note ws-picker-row-note-soft' : 'ws-picker-row-note'}>`;

describe('the fault note is wired to the one state that is a fault', () => {
  it('reads the shared predicate rather than testing the state word again', () => {
    expect(source).toContain('workspaceFaultNote(w)');
    expect(source).toContain(`${NOTE}{fault}</p>`);
  });
});

describe('the note is announced, exactly once', () => {
  it('the row folds the fault into its own accessible name', () => {
    expect(source).toMatch(/const rowLabel = \[\s*w\.health === 'restoring' \? `Restoring \$\{w\.name\}` : fault \? `Retry \$\{w\.name\} · \$\{fault\}`/);
    expect(source).toContain('aria-label={rowLabel}');
  });

  it('the dot steps aside when the note is on screen', () => {
    expect(source).toContain('aria-label={fault ? undefined : workspaceStateLabel(w)}');
    expect(source).toContain("aria-hidden={fault ? 'true' : undefined}");
  });

  it('the dot keeps its hover tooltip for every state', () => {
    expect(source).toContain('data-tooltip={workspaceStateLabel(w)}');
  });
});

describe('the note takes its own line, on the name column', () => {
  it('is the second box in the row column, never an item in the line', () => {
    expect(decl(block(pickerCss, '.ws-picker-open {'), 'flex-direction')).toBe('column');

    const to = source.indexOf(`{fault && ${NOTE}`);
    expect(to, 'the note is gone').toBeGreaterThanOrEqual(0);
    const from = source.lastIndexOf('<div class="ws-picker-line">', to);
    expect(from, 'the row line is gone').toBeGreaterThanOrEqual(0);
    const between = source.slice(from, to);
    expect(between.match(/<\/div>/g)?.length).toBe(between.match(/<div\b/g)?.length);
  });

  it('indents to the name column by derivation, not by a guess', () => {
    const indent = decl(block(pickerCss, '.ws-picker-row-note {'), 'padding-left') ?? '';
    expect(indent).toContain('var(--ws-picker-dot-size)');
    expect(indent).toContain('var(--ws-picker-row-gap)');
    const rootTokens = block(pickerCss, ':root');
    for (const name of ['--ws-picker-dot-size', '--ws-picker-row-gap', '--ws-picker-note-gap']) {
      expect(decl(rootTokens, name), `${name} is read but declared nowhere`).not.toBeNull();
    }
  });

  it('wraps rather than truncating the gateway error', () => {
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
    const from = source.lastIndexOf('<div class="ws-picker-id">');
    expect(from, 'the id cell is gone').toBeGreaterThanOrEqual(0);
    const at = source.indexOf('class={`ws-picker-backup', from);
    expect(at, 'the backup line is gone').toBeGreaterThan(from);
    const between = source.slice(from, at);
    expect(between.match(/<\/div>/g)?.length ?? 0).toBe(
      (between.match(/<div\b/g)?.length ?? 0) - 1,
    );
  });

  it('reads the shared rule rather than re-deriving what stale means', () => {
    expect(source).toContain('backupNote(w)');
    expect(source).not.toMatch(/24 \* 60 \* 60|86400/);
  });

  it('is announced by the row and not a second time by itself', () => {
    const at = source.indexOf('class={`ws-picker-backup');
    expect(source.slice(at, at + 200)).toContain('aria-hidden="true"');
  });

  it('holds its line open even with nothing to say', () => {
    expect(source).not.toContain('{backup && (');
    expect(source).toContain("{backup?.text ?? ''}");
    const line = block(pickerCss, '.ws-picker-backup {');
    expect(decl(line, 'display')).toBe('block');
    expect(decl(line, 'line-height')).toBe('var(--ws-picker-sub-line-height)');
    const reserved = decl(line, 'min-height') ?? '';
    expect(reserved).toContain('var(--font-size-sm)');
    expect(reserved).toContain('var(--ws-picker-sub-line-height)');
    expect(decl(line, 'font-size')).toBe('var(--font-size-sm)');
    expect(decl(block(pickerCss, ':root'), '--ws-picker-sub-line-height')).not.toBeNull();
  });

  it('warns in the row\'s own coral, quiet otherwise', () => {
    expect(decl(block(pickerCss, '.ws-picker-backup-warn {'), 'color'))
      .toBe('var(--ws-picker-fault-color)');
    expect(decl(block(pickerCss, '.ws-picker-backup {'), 'color'))
      .not.toBe('var(--ws-picker-fault-color)');
  });
});

describe('the row line holds its three cells on one line', () => {
  it('never wraps, so a long name cannot push the actions off the row', () => {
    const rules = rulesTargeting(pickerCss, 'ws-picker-line');
    expect(rules.length, 'the row line has no rule at all').toBeGreaterThan(0);
    for (const rule of rules) {
      const wrap = rule.props.get('flex-wrap');
      expect(wrap ?? 'nowrap', `${rule.selector} ${rule.atRules}`).toBe('nowrap');
    }
  });

  it('spells the name whole rather than truncating it', () => {
    const name = block(pickerCss, '.ws-picker-name {');
    expect(decl(name, 'text-overflow')).toBeNull();
    expect(decl(name, 'white-space')).toBeNull();
    expect(decl(name, 'overflow-wrap')).toBe('anywhere');

    const id = block(pickerCss, '.ws-picker-id {');
    expect(decl(id, 'min-width')).toBe('0');
    expect(decl(id, 'flex-direction')).toBe('column');
    expect(decl(block(pickerCss, '.ws-picker-actions {'), 'flex')).toBe('0 0 auto');
  });

  it('keeps the address whole too, since it is the other tiebreaker', () => {
    const addr = block(pickerCss, '.ws-picker-address {');
    expect(decl(addr, 'text-overflow')).toBeNull();
    expect(decl(addr, 'white-space')).toBeNull();
  });

  it('lands the dot on the first line of the name, by derivation', () => {
    const scoped = block(pickerCss, '.ws-picker-line .ws-picker-dot {');
    expect(decl(scoped, 'align-self')).toBe('flex-start');
    const offset = decl(scoped, 'margin-top') ?? '';
    expect(offset).toContain('var(--ws-picker-name-line-height)');
    expect(offset).toContain('var(--ws-picker-dot-size)');
    expect(decl(block(pickerCss, ':root'), '--ws-picker-name-line-height')).not.toBeNull();
  });

  it('tops the name cell out with the dot, so the offset has one origin', () => {
    expect(decl(block(pickerCss, '.ws-picker-id {'), 'align-self')).toBe('flex-start');
  });

  it('leaves the SHARED dot centred, for the in-app switcher', () => {
    for (const rule of rulesTargeting(pickerCss, 'ws-picker-dot')) {
      if (!rule.props.has('align-self') && !rule.props.has('margin-top')) continue;
      expect(rule.selector, 'an unscoped rule moves the shared dot')
        .toContain('.ws-picker-line ');
    }
  });

  it('is what the skeleton row draws too', () => {
    expect(source.match(/<div class="ws-picker-line">/g)?.length).toBe(2);
    expect(source.match(/<div class="ws-picker-id">/g)?.length).toBe(2);
  });
});
