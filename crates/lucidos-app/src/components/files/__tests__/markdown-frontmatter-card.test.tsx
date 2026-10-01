// @vitest-environment jsdom
/**
 * A markdown preview draws the file's frontmatter as a properties card. Left to
 * marked, the raw block renders as a horizontal rule over one big heading.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { MarkdownDocument } from '../MarkdownDocument';

const DAILY_NOTE = [
  '---',
  'type: daily-note',
  'status: open',
  'tags: [review, testing]',
  'brief_refreshed_at: 2026-09-29T22:59:20+03:00',
  '---',
  '# Today',
  '',
].join('\n');

let host: HTMLElement;

function mount(content: string): HTMLElement {
  host = document.createElement('div');
  document.body.append(host);
  render(<MarkdownDocument content={content} />, host);
  return host;
}

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('MarkdownDocument frontmatter', () => {
  it('renders each field as a key/value row and list items as chips', () => {
    const el = mount(DAILY_NOTE);
    const rows = [...el.querySelectorAll('.frontmatter-card .frontmatter-row')].map((row) => [
      row.querySelector('dt')?.textContent,
      row.querySelector('dd')?.textContent,
    ]);
    expect(rows).toEqual([
      ['type', 'daily-note'],
      ['status', 'open'],
      ['tags', 'reviewtesting'],
      ['brief_refreshed_at', '2026-09-29T22:59:20+03:00'],
    ]);
    expect([...el.querySelectorAll('.frontmatter-chip')].map((c) => c.textContent)).toEqual(['review', 'testing']);
  });

  it('renders the body without the frontmatter leaking into it', () => {
    const body = mount(DAILY_NOTE).querySelector('.markdown-content')!;
    expect(body.querySelector('h1')?.textContent).toBe('Today');
    expect(body.querySelector('hr')).toBeNull();
    expect(body.textContent).not.toContain('daily-note');
  });

  it('draws no card for a file without frontmatter', () => {
    const el = mount('# Plain\n\nText.');
    expect(el.querySelector('.frontmatter-card')).toBeNull();
    expect(el.querySelector('.markdown-content h1')?.textContent).toBe('Plain');
  });
});
