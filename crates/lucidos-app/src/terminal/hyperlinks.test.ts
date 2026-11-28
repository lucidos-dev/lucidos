import { describe, it, expect, vi } from 'vitest';
import { Terminal, type ILink, type ILinkProvider } from '@xterm/xterm';
import { hyperlinkTarget, installHyperlinks, labelShowsTarget, type OpenHyperlink } from './hyperlinks';

const COLS = 20;
const URL_TEXT = 'https://example.com/authorize?state=abcdefghijklmnop';

function setup() {
  const term = new Terminal({ cols: COLS, rows: 10, allowProposedApi: true });
  let provider: ILinkProvider | undefined;
  const register = term.registerLinkProvider.bind(term);
  vi.spyOn(term, 'registerLinkProvider').mockImplementation((p) => {
    provider = p;
    return register(p);
  });
  const open = vi.fn<OpenHyperlink>();
  installHyperlinks(term, open);
  const write = (data: string) => new Promise<void>((resolve) => term.write(data, resolve));
  const linksAt = (y: number) =>
    new Promise<ILink[] | undefined>((resolve) => provider!.provideLinks(y, resolve));
  return { term, open, write, linksAt };
}

const hyperlink = (uri: string, text: string) => `\x1b]8;;${uri}\x07${text}\x1b]8;;\x07`;

describe('hyperlinkTarget', () => {
  it('keeps semicolons inside the uri', () => {
    expect(hyperlinkTarget('id=1;https://a.test/?x=1;y=2')).toBe('https://a.test/?x=1;y=2');
  });
});

describe('labelShowsTarget', () => {
  it('is true only when the visible text is the uri', () => {
    expect(labelShowsTarget(` ${URL_TEXT} `, URL_TEXT)).toBe(true);
    expect(labelShowsTarget('https://github.com/login', 'https://evil.test/')).toBe(false);
  });
});

describe('installHyperlinks', () => {
  it('gives every row of a wrapped hyperlink one link spanning all its rows', async () => {
    const { write, linksAt, open } = setup();
    await write(`  ${hyperlink(URL_TEXT, URL_TEXT)}\r\n`);
    const rows = Math.ceil((URL_TEXT.length + 2) / COLS);
    const expected = {
      start: { x: 3, y: 1 },
      end: { x: (URL_TEXT.length + 2) % COLS, y: rows },
    };
    for (let y = 1; y <= rows; y++) {
      const links = await linksAt(y);
      expect(links).toHaveLength(1);
      expect(links![0].range).toEqual(expected);
      expect(links![0].text).toBe(URL_TEXT);
    }
    expect(await linksAt(rows + 1)).toBeUndefined();

    (await linksAt(2))![0].activate({} as MouseEvent, URL_TEXT);
    expect(open).toHaveBeenCalledWith(URL_TEXT, URL_TEXT);
  });

  it('starts on the next row when the link opens at a pending wrap', async () => {
    const { write, linksAt } = setup();
    await write(`${'x'.repeat(COLS)}${hyperlink('https://a.test/', 'link')}`);
    expect(await linksAt(1)).toBeUndefined();
    expect((await linksAt(2))![0].range).toEqual({ start: { x: 1, y: 2 }, end: { x: 4, y: 2 } });
  });

  it('drops a link once its text is overwritten', async () => {
    const { write, linksAt } = setup();
    await write(hyperlink('https://a.test/', 'link'));
    await write('\rnope');
    expect(await linksAt(1)).toBeUndefined();
  });

  it('replaces a link redrawn in the same place', async () => {
    const { write, linksAt } = setup();
    await write(hyperlink('https://a.test/', 'link'));
    await write(`\r${hyperlink('https://a.test/', 'link')}`);
    expect(await linksAt(1)).toHaveLength(1);
  });

  it('ignores a uri that is not http or https', async () => {
    const { write, linksAt } = setup();
    await write(hyperlink('file:///etc/passwd', 'file'));
    expect(await linksAt(1)).toBeUndefined();
  });

  it('passes the visible text to the opener', async () => {
    const { write, linksAt, open } = setup();
    await write(hyperlink('https://evil.test/', 'github.com'));
    (await linksAt(1))![0].activate({} as MouseEvent, 'https://evil.test/');
    expect(open).toHaveBeenCalledWith('https://evil.test/', 'github.com');
  });

  it('keeps a wrapped link through a resize either way', async () => {
    const { term, write, linksAt } = setup();
    await write(`  ${hyperlink(URL_TEXT, URL_TEXT)}\r\n`);
    for (const cols of [30, 12, 80]) {
      term.resize(cols, 10);
      const rows = Math.ceil((URL_TEXT.length + 2) / cols);
      const links = await linksAt(rows);
      expect(links, `${cols} columns`).toHaveLength(1);
      expect(links![0].range).toEqual({
        start: { x: 3, y: 1 },
        end: { x: ((URL_TEXT.length + 1) % cols) + 1, y: rows },
      });
    }
  });

  it('keeps a link through a resize when a wide character wrapped early', async () => {
    const { term, write, linksAt } = setup();
    const lead = `${'x'.repeat(COLS - 1)}你`;
    await write(`${lead}${hyperlink(URL_TEXT, URL_TEXT)}\r\n`);
    expect((await linksAt(2))![0].range.start).toEqual({ x: 3, y: 2 });
    const cols = 30;
    term.resize(cols, 10);
    const startCell = COLS + 1;
    const lastCell = startCell + URL_TEXT.length - 1;
    expect((await linksAt(1))![0].range).toEqual({
      start: { x: (startCell % cols) + 1, y: Math.floor(startCell / cols) + 1 },
      end: { x: (lastCell % cols) + 1, y: Math.floor(lastCell / cols) + 1 },
    });
  });

  it('keeps a link on the cursor line, which a resize does not reflow', async () => {
    const { term, write, linksAt } = setup();
    await write(`${'x'.repeat(COLS + 2)}${hyperlink('https://a.test/', 'link')}`);
    for (const cols of [30, 10, 20]) {
      term.resize(cols, 10);
      expect((await linksAt(2))?.[0].range, `${cols} columns`).toEqual({
        start: { x: 3, y: 2 },
        end: { x: 6, y: 2 },
      });
    }
  });

  it('keeps a link to the buffer it was drawn in', async () => {
    const { write, linksAt } = setup();
    await write(hyperlink('https://a.test/', 'link'));
    await write('\x1b[?1049h\x1b[Hlink');
    expect(await linksAt(1)).toBeUndefined();
    await write(`\x1b[H${hyperlink('https://b.test/', 'link')}`);
    expect((await linksAt(1))![0].text).toBe('https://b.test/');
    await write('\x1b[?1049l');
    expect((await linksAt(1))![0].text).toBe('https://a.test/');
  });

  it('ignores a hyperlink with no text', async () => {
    const { write, linksAt } = setup();
    await write(`abc${hyperlink('https://a.test/', '')}def`);
    expect(await linksAt(1)).toBeUndefined();
  });
});
