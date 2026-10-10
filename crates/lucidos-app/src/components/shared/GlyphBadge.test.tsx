// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { GlyphBadge } from './GlyphBadge';

// Only "3" draws off its slot, and a "Wide" font adds a little. The measurement
// itself is pinned in inkCentre.test.ts.
vi.mock('../../utils/inkCentre', () => ({
  measureInkShift: (text: string, style: CSSStyleDeclaration) =>
    (text === '3' ? 0.025 : 0) + (style.fontFamily === 'Wide' ? 0.01 : 0),
}));

describe('GlyphBadge', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  const badge = () => host.querySelector<HTMLElement>('.badge')!;

  it('states the ink shift for a glyph drawn off its slot', () => {
    render(<GlyphBadge class="badge">3</GlyphBadge>, host);
    expect(badge().style.getPropertyValue('--badge-ink-shift')).toBe('0.0250em');
  });

  it('states none for a glyph already centred', () => {
    render(<GlyphBadge class="badge">8</GlyphBadge>, host);
    expect(badge().style.getPropertyValue('--badge-ink-shift')).toBe('');
  });

  it('measures again when the count changes, both ways', () => {
    render(<GlyphBadge class="badge">8</GlyphBadge>, host);
    render(<GlyphBadge class="badge">3</GlyphBadge>, host);
    expect(badge().style.getPropertyValue('--badge-ink-shift')).toBe('0.0250em');
    render(<GlyphBadge class="badge">8</GlyphBadge>, host);
    expect(badge().style.getPropertyValue('--badge-ink-shift')).toBe('');
  });

  it('measures again when its font changes under the same text', () => {
    // A theme card keeps its "Custom" text while the card's own font changes.
    render(<div style={{ fontFamily: 'Narrow' }}><GlyphBadge class="badge">3</GlyphBadge></div>, host);
    render(<div style={{ fontFamily: 'Wide' }}><GlyphBadge class="badge">3</GlyphBadge></div>, host);
    expect(badge().style.getPropertyValue('--badge-ink-shift')).toBe('0.0350em');
  });

  it('wraps text in the span its shift moves, and leaves an icon alone', () => {
    render(<GlyphBadge class="badge">3</GlyphBadge>, host);
    expect(badge().querySelector('.badge-ink')?.textContent).toBe('3');
    render(<GlyphBadge class="badge"><svg /></GlyphBadge>, host);
    expect(badge().querySelector('.badge-ink')).toBeNull();
  });

  it('passes its attributes through to the span', () => {
    render(<GlyphBadge class="badge filter-badge" aria-hidden="true" data-shown="">3</GlyphBadge>, host);
    expect(badge().tagName).toBe('SPAN');
    expect(badge().className).toBe('badge filter-badge');
    expect(badge().getAttribute('aria-hidden')).toBe('true');
    expect(badge().hasAttribute('data-shown')).toBe(true);
  });
});
