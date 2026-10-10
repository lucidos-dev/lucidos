// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { capture } from './capture';
import { StyleSheetBuilder } from './rasterize';

describe('capture', () => {
  it('degrades to the DOM snapshot when the screenshot fails, never rejecting', async () => {
    // jsdom paints nothing, so the rasterizer fails here exactly as it would
    // on an image that will not decode.
    document.body.innerHTML = '<div class="card"><h1>Hello</h1></div>';
    const result = await capture();
    expect(result.screenshot).toBe('');
    expect(result.dom).toMatch(/^\[screenshot unavailable: /);
    expect(result.dom).toContain('<div class="card"');
    expect(result.dom).toContain('Hello');
  });
});

describe('StyleSheetBuilder', () => {
  it('gives identical declarations one class, and a pseudo-element its own rule', () => {
    const sheet = new StyleSheetBuilder();
    const a = sheet.classFor('color:red;');
    expect(sheet.classFor('color:red;')).toBe(a);
    const b = sheet.classFor('color:blue;');
    const before = sheet.classFor('color:red;', '::before');
    expect(new Set([a, b, before]).size).toBe(3);
    expect(sheet.css()).toBe(`.${a}{color:red;}\n.${b}{color:blue;}\n.${before}::before{color:red;}`);
  });
});
