// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { capture } from './capture';
import {
  CAPTURE_MAX_HEIGHT, CAPTURE_MAX_WIDTH, CAPTURE_QUALITY, CAPTURE_TYPE, SAVED_MAX_EDGE, SAVED_MAX_PIXEL_RATIO,
  SAVED_QUALITY, StyleSheetBuilder, renderingFor,
} from './rasterize';

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

describe('renderingFor', () => {
  it('keeps the agent picture small, 1x and JPEG, whatever the pixel ratio', () => {
    expect(renderingFor(390, 844, 3)).toEqual({
      scale: 1, width: 390, height: 844, type: CAPTURE_TYPE, quality: CAPTURE_QUALITY,
    });
    const wide = renderingFor(2 * CAPTURE_MAX_WIDTH, 4000, 2);
    expect(wide.width).toBe(CAPTURE_MAX_WIDTH);
    expect(wide.height).toBe(CAPTURE_MAX_HEIGHT);
  });

  it('renders a saved picture at the device pixel ratio, capped, in the format asked for', () => {
    expect(renderingFor(390, 844, 3, 'png')).toEqual({
      scale: SAVED_MAX_PIXEL_RATIO, width: 780, height: 1688, type: 'image/png', quality: SAVED_QUALITY,
    });
    expect(renderingFor(390, 844, 1, 'webp')).toMatchObject({ scale: 1, width: 390, type: 'image/webp' });
  });

  it('keeps a saved picture\'s long edge within the cap, and never below 1x on a low ratio', () => {
    const desktop = renderingFor(1440, 900, 2, 'jpeg');
    expect(Math.max(desktop.width, desktop.height)).toBe(SAVED_MAX_EDGE);
    expect(desktop.type).toBe(CAPTURE_TYPE);
    expect(renderingFor(400, 300, 0.5, 'png').scale).toBe(1);
  });
});
