import { describe, it, expect, vi } from 'vitest';
import { findDropZone, dispatchDrop } from './dropDispatch';

/** Build a minimal element-like object with the surface findDropZone reads.
 *  Stubs `closest` since the test environment has no real DOM — walks the
 *  parent chain and returns the first ancestor whose data-drop-zone matches
 *  one of the values requested by the selector. */
function makeEl(zone?: 'attach' | 'import' | string): any {
  const el: any = {
    parentElement: null,
    getAttribute: (key: string) => (key === 'data-drop-zone' && zone !== undefined ? zone : null),
  };
  el.closest = (selector: string): any => {
    let cur: any = el;
    while (cur) {
      const v = cur.getAttribute?.('data-drop-zone');
      if (v && selector.includes(`"${v}"`)) return cur;
      cur = cur.parentElement;
    }
    return null;
  };
  return el;
}

/** Wire elements into a parent chain — first arg is the innermost (event target). */
function chain(...nodes: any[]): any {
  for (let i = 0; i < nodes.length - 1; i++) {
    nodes[i].parentElement = nodes[i + 1];
  }
  return nodes[0];
}

function makeFakeFile(name: string, type: string): File {
  return { name, type } as unknown as File;
}

function makeFakeFileList(files: File[]): FileList {
  const list: any = { length: files.length, item: (i: number) => files[i] ?? null };
  files.forEach((f, i) => { list[i] = f; });
  list[Symbol.iterator] = function* () { for (const f of files) yield f; };
  return list as FileList;
}

describe('findDropZone', () => {
  it('returns null when no ancestor is a drop zone', () => {
    const target = chain(makeEl(), makeEl(), makeEl());
    expect(findDropZone(target)).toBeNull();
  });

  it('returns null for a null target', () => {
    expect(findDropZone(null)).toBeNull();
  });

  it('matches when the target itself is the zone', () => {
    const target = makeEl('attach');
    expect(findDropZone(target)?.kind).toBe('attach');
  });

  it('walks up to the nearest ancestor with data-drop-zone', () => {
    const inner = makeEl();
    const middle = makeEl();
    const outer = makeEl('import');
    expect(findDropZone(chain(inner, middle, outer))?.kind).toBe('import');
  });

  it('returns the innermost zone when both attach and import are ancestors', () => {
    // Inner zone wins over outer — supports nesting (e.g. files panel inside an attach surface).
    const inner = makeEl('attach');
    const outer = makeEl('import');
    expect(findDropZone(chain(makeEl(), inner, outer))?.kind).toBe('attach');
  });

  it('ignores unknown data-drop-zone values', () => {
    const target = chain(makeEl(), makeEl('something-else'));
    expect(findDropZone(target)).toBeNull();
  });
});

describe('dispatchDrop', () => {
  const oneFile = makeFakeFileList([makeFakeFile('a.png', 'image/png')]);

  it('routes to attach when the target is in an attach zone', async () => {
    const attach = vi.fn().mockResolvedValue(undefined);
    const importFn = vi.fn().mockResolvedValue(undefined);
    const target = chain(makeEl(), makeEl('attach'));
    const kind = await dispatchDrop(target, oneFile, { attach, import: importFn });
    expect(kind).toBe('attach');
    expect(attach).toHaveBeenCalledOnce();
    expect(attach).toHaveBeenCalledWith(oneFile);
    expect(importFn).not.toHaveBeenCalled();
  });

  it('routes to import when the target is in an import zone', async () => {
    const attach = vi.fn().mockResolvedValue(undefined);
    const importFn = vi.fn().mockResolvedValue(undefined);
    const target = chain(makeEl(), makeEl('import'));
    const kind = await dispatchDrop(target, oneFile, { attach, import: importFn });
    expect(kind).toBe('import');
    expect(importFn).toHaveBeenCalledOnce();
    expect(importFn).toHaveBeenCalledWith(oneFile);
    expect(attach).not.toHaveBeenCalled();
  });

  it('does nothing and returns null when the target is outside any zone', async () => {
    const attach = vi.fn();
    const importFn = vi.fn();
    const target = chain(makeEl(), makeEl());
    const kind = await dispatchDrop(target, oneFile, { attach, import: importFn });
    expect(kind).toBeNull();
    expect(attach).not.toHaveBeenCalled();
    expect(importFn).not.toHaveBeenCalled();
  });

  it('does nothing when the file list is null', async () => {
    const attach = vi.fn();
    const importFn = vi.fn();
    const kind = await dispatchDrop(makeEl('attach'), null, { attach, import: importFn });
    expect(kind).toBeNull();
    expect(attach).not.toHaveBeenCalled();
  });

  it('does nothing when the file list is empty', async () => {
    const attach = vi.fn();
    const importFn = vi.fn();
    const kind = await dispatchDrop(makeEl('attach'), makeFakeFileList([]), { attach, import: importFn });
    expect(kind).toBeNull();
    expect(attach).not.toHaveBeenCalled();
  });
});
