import { describe, it, expect } from 'vitest';
import { normalizeCss, isCssWedge, type CssBuildSnapshot } from './cssWedgeDetect';

describe('normalizeCss', () => {
  it('strips block comments', () => {
    expect(normalizeCss('/* a comment */ .x { color: red }')).toBe('.x { color: red }');
  });

  it('strips multi-line comments', () => {
    expect(normalizeCss('.x{}\n/* multi\n line\n comment */\n.y{}')).toBe('.x{} .y{}');
  });

  it('collapses whitespace runs to a single space and trims', () => {
    expect(normalizeCss('   .x   {\n  color:   red;\n}   ')).toBe('.x { color: red; }');
  });

  it('makes comment-only and reformatting edits hash-equal', () => {
    const a = normalizeCss('.control-dropdown { position: absolute; }');
    const b = normalizeCss('/* reordered note */\n.control-dropdown {\n    position: absolute;\n}\n');
    expect(a).toBe(b);
  });

  it('keeps a real class rename distinct', () => {
    const a = normalizeCss('.cc-control-dropdown { position: absolute; }');
    const b = normalizeCss('.control-dropdown { position: absolute; }');
    expect(a).not.toBe(b);
  });
});

describe('isCssWedge', () => {
  const snap = (cssSourceHash: string, cssOutputFingerprint: string): CssBuildSnapshot => ({
    cssSourceHash,
    cssOutputFingerprint,
  });

  it('never flags the first build (no prior generation)', () => {
    expect(isCssWedge(null, snap('s1', 'out1'))).toBe(false);
  });

  it('flags the wedge: source changed but emitted CSS did not', () => {
    // New JS from fresh source, frozen stale CSS output — the exact desync.
    expect(isCssWedge(snap('s1', 'out1'), snap('s2', 'out1'))).toBe(true);
  });

  it('does not flag a healthy CSS change (source and output both moved)', () => {
    expect(isCssWedge(snap('s1', 'out1'), snap('s2', 'out2'))).toBe(false);
  });

  it('does not flag a JS-only rebuild (CSS source unchanged)', () => {
    expect(isCssWedge(snap('s1', 'out1'), snap('s1', 'out1'))).toBe(false);
  });

  it('does not flag when output changes without a source change', () => {
    expect(isCssWedge(snap('s1', 'out1'), snap('s1', 'out2'))).toBe(false);
  });
});
