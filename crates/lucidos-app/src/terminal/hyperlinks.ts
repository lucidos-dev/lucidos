import type { IBuffer, IBufferCellPosition, IBufferRange, IDisposable, ILink, IMarker, Terminal } from '@xterm/xterm';

export const MAX_TRACKED_HYPERLINKS = 500;

const OPENABLE_PROTOCOLS = new Set(['http:', 'https:']);

interface Anchor {
  head: IMarker;
  offset: number;
  drawnRow: number;
  drawnCol: number;
}

type BufferType = IBuffer['type'];

interface Placement {
  range: IBufferRange;
  cols: number;
}

interface Hyperlink {
  uri: string;
  buffer: BufferType;
  start: Anchor;
  end: Anchor;
  drawnCols: number;
  text: string;
}

export type OpenHyperlink = (uri: string, label: string) => void;

export function hyperlinkTarget(oscData: string): string {
  const separator = oscData.indexOf(';');
  return separator === -1 ? '' : oscData.slice(separator + 1);
}

export function labelShowsTarget(label: string, uri: string): boolean {
  return label.trim() === uri;
}

function openable(uri: string): boolean {
  try {
    return OPENABLE_PROTOCOLS.has(new URL(uri).protocol);
  } catch {
    return false;
  }
}

function before(a: IBufferCellPosition, b: IBufferCellPosition): boolean {
  return a.y < b.y || (a.y === b.y && a.x < b.x);
}

function overlaps(a: IBufferRange, b: IBufferRange): boolean {
  return !before(a.end, b.start) && !before(b.end, a.start);
}

function alive(link: Hyperlink): boolean {
  return !link.start.head.isDisposed && !link.end.head.isDisposed;
}

export function installHyperlinks(term: Terminal, open: OpenHyperlink): IDisposable {
  let links: Hyperlink[] = [];
  let pending: { uri: string; buffer: BufferType; start: Anchor } | null = null;
  const activeBuffer = (): BufferType => term.buffer.active.type;

  const wrapsIntoNext = (y: number): boolean => term.buffer.active.getLine(y + 1)?.isWrapped ?? false;

  const cellsInRow = (y: number, cols = term.cols): number => {
    const buffer = term.buffer.active;
    if (!wrapsIntoNext(y)) return cols;
    const last = buffer.getLine(y)?.getCell(cols - 1);
    const next = buffer.getLine(y + 1)?.getCell(0);
    const padded = last?.getChars() === '' && last.getWidth() === 1 && next?.getWidth() === 2;
    return padded ? cols - 1 : cols;
  };

  const cellAt = (anchor: Anchor, offset: number): IBufferCellPosition => {
    let y = anchor.head.line;
    if (offset < 0) return { x: term.cols + offset + 1, y };
    while (offset >= cellsInRow(y) && wrapsIntoNext(y)) {
      offset -= cellsInRow(y);
      y++;
    }
    return { x: offset + 1, y: y + 1 };
  };

  const drawnCellAt = (anchor: Anchor, shift: number, cols: number): IBufferCellPosition => {
    const col = anchor.drawnCol + shift;
    const wrap = col >= cols ? 1 : col < 0 ? -1 : 0;
    return { x: col - wrap * cols + 1, y: anchor.head.line + anchor.drawnRow + wrap + 1 };
  };

  const placements = (link: Hyperlink): Placement[] => [
    {
      range: { start: cellAt(link.start, link.start.offset), end: cellAt(link.end, link.end.offset - 1) },
      cols: term.cols,
    },
    {
      range: { start: drawnCellAt(link.start, 0, link.drawnCols), end: drawnCellAt(link.end, -1, link.drawnCols) },
      cols: link.drawnCols,
    },
  ];

  const textIn = ({ range, cols }: Placement): string => {
    const buffer = term.buffer.active;
    let text = '';
    for (let y = range.start.y; y <= range.end.y; y++) {
      const line = buffer.getLine(y - 1);
      if (!line) return '';
      const from = y === range.start.y ? range.start.x - 1 : 0;
      const to = y === range.end.y ? range.end.x : cellsInRow(y - 1, cols);
      text += line.translateToString(false, from, to);
    }
    return text;
  };

  const shownRange = (link: Hyperlink): IBufferRange | undefined =>
    placements(link).find((placement) => textIn(placement) === link.text)?.range;

  const anchorAtCursor = (): Anchor => {
    const buffer = term.buffer.active;
    const row = buffer.baseY + buffer.cursorY;
    let head = row;
    while (head > 0 && buffer.getLine(head)?.isWrapped) head--;
    let offset = buffer.cursorX;
    for (let y = head; y < row; y++) offset += cellsInRow(y);
    return { head: term.registerMarker(head - row), offset, drawnRow: row - head, drawnCol: buffer.cursorX };
  };

  const disposeLink = (link: Hyperlink) => {
    link.start.head.dispose();
    link.end.head.dispose();
  };

  const dropPending = () => {
    pending?.start.head.dispose();
    pending = null;
  };

  const close = () => {
    if (!pending) return;
    const { uri, buffer, start } = pending;
    pending = null;
    const link: Hyperlink = { uri, buffer, start, end: anchorAtCursor(), drawnCols: term.cols, text: '' };
    const placement = alive(link) && buffer === activeBuffer() ? placements(link)[0] : null;
    link.text = placement && !before(placement.range.end, placement.range.start) ? textIn(placement) : '';
    if (!placement || !link.text) {
      disposeLink(link);
      return;
    }
    const { range } = placement;
    links = links.filter((old) => {
      const replaced =
        alive(old) && old.buffer === buffer && placements(old).some((drawn) => overlaps(drawn.range, range));
      if (replaced) disposeLink(old);
      return !replaced;
    });
    links.push(link);
    while (links.length > MAX_TRACKED_HYPERLINKS) disposeLink(links.shift()!);
  };

  const osc = term.parser.registerOscHandler(8, (data) => {
    close();
    dropPending();
    const uri = hyperlinkTarget(data);
    if (uri && openable(uri)) pending = { uri, buffer: activeBuffer(), start: anchorAtCursor() };
    return true;
  });

  const provider = term.registerLinkProvider({
    provideLinks(y, callback) {
      links = links.filter((link) => {
        if (alive(link)) return true;
        disposeLink(link);
        return false;
      });
      const found: ILink[] = [];
      const buffer = activeBuffer();
      for (const link of links) {
        if (link.buffer !== buffer) continue;
        const range = shownRange(link);
        if (!range || y < range.start.y || y > range.end.y) continue;
        found.push({ range, text: link.uri, activate: () => open(link.uri, link.text) });
      }
      callback(found.length ? found : undefined);
    },
  });

  return {
    dispose() {
      osc.dispose();
      provider.dispose();
      dropPending();
      links.forEach(disposeLink);
      links = [];
    },
  };
}
