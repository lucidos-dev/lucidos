// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
// @ts-expect-error: same
import { tmpdir } from 'node:os';
// @ts-expect-error: same
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { docFamilyText } from './test-doc-family';

describe('docFamilyText', () => {
  it('reads a split doc and every part under its folder, in order', () => {
    const repo = mkdtempSync(join(tmpdir(), 'doc-family-'));
    mkdirSync(join(repo, 'kh/doc/deep'), { recursive: true });
    writeFileSync(join(repo, 'kh/doc.md'), 'index');
    writeFileSync(join(repo, 'kh/doc/b.md'), 'part b');
    writeFileSync(join(repo, 'kh/doc/a.md'), 'part a');
    writeFileSync(join(repo, 'kh/doc/deep/c.md'), 'part c');
    writeFileSync(join(repo, 'kh/doc/notes.txt'), 'not markdown');
    expect(docFamilyText(repo, 'kh/doc.md')).toBe('index\npart a\npart b\npart c');
  });

  it('reads a doc with no folder as itself', () => {
    const repo = mkdtempSync(join(tmpdir(), 'doc-family-'));
    writeFileSync(join(repo, 'one.md'), 'whole');
    expect(docFamilyText(repo, 'one.md')).toBe('whole');
  });
});
