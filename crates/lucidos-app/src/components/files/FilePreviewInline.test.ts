import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return { ...actual, saveDataFile: vi.fn() };
});

import { basename, previewUrl, sourceLinesFor, EditorToolbar, editorToolbarState, toggleDataFileCheckbox } from './FilePreviewInline';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
import { saveDataFile } from '../../api/client';
import { toasts, removeToast } from '../../store/store';

const here: string = dirname(fileURLToPath(import.meta.url));

describe('basename', () => {
  it('returns the last segment of a nested path', () => {
    expect(basename('apps/no-role-playing-0.1.2.lucidos-plugin'))
      .toBe('no-role-playing-0.1.2.lucidos-plugin');
  });

  it('returns the path itself when it has no slashes', () => {
    expect(basename('foo.bin')).toBe('foo.bin');
  });

  it('handles deeply nested paths', () => {
    expect(basename('a/b/c/file.tar.gz')).toBe('file.tar.gz');
  });

  it('returns empty string for empty input', () => {
    expect(basename('')).toBe('');
  });

  it('returns empty string for trailing-slash paths', () => {
    expect(basename('apps/')).toBe('');
  });
});

// This URL is the `src` of the media element the preview renders. Any change
// to it reloads that element and restarts playback. The stamp's path is what
// keeps an unrelated write from doing so.
describe('previewUrl', () => {
  const BASE = '/data/artifacts/clips/demo.mp4';

  it('is the bare URL with no stamp at all', () => {
    expect(previewUrl(BASE, 'artifacts/clips/demo.mp4', null)).toBe(BASE);
  });

  it('cache-busts when the stamp names this file', () => {
    expect(previewUrl(BASE, 'artifacts/clips/demo.mp4', { path: 'artifacts/clips/demo.mp4', rev: 3 }))
      .toBe(`${BASE}?v=3`);
  });

  // The reported bug: an agent writing something else must not touch this URL.
  it('is byte-identical when the stamp names another file', () => {
    expect(previewUrl(BASE, 'artifacts/clips/demo.mp4', { path: 'artifacts/notes.md', rev: 7 }))
      .toBe(BASE);
  });

  // A file never invalidated resolves to no stamp. A `?v=0` would be one more
  // distinct URL than the browser needs to fetch.
  it('leaves a zero revision off the URL', () => {
    expect(previewUrl(BASE, 'artifacts/clips/demo.mp4', { path: 'artifacts/clips/demo.mp4', rev: 0 }))
      .toBe(BASE);
  });
});

// The data-file preview shows the same line-numbered source view the repo
// preview does, so a `navigate('file', { line })` into a workspace file has a
// numbered row to scroll to and highlight. This is the branch that decides
// whether a file has source lines at all.
describe('sourceLinesFor', () => {
  it('numbers a code file line by line', () => {
    const lines = sourceLinesFor('fn main() {\n    let x = 1;\n}', 'rs', false);
    expect(lines).toHaveLength(3);
    expect(lines[1]).toContain('let');
  });

  // Content is asserted by line count, not by text: `escapeHtml` runs through a
  // real `document` and the test environment's stub cannot reproduce it, so the
  // escaping itself belongs to `escapeHtml`, not here.
  it('numbers a file with no known language', () => {
    expect(sourceLinesFor('plain <b>text</b>\nsecond', 'txt', false)).toHaveLength(2);
  });

  // A numbered line must be the file's OWN line. Reformatting valid JSON would
  // renumber it, so a `path:42` citation and the range handed to chat context
  // would both point at code that isn't in the file on disk.
  it('numbers JSON as written, never as reformatted', () => {
    expect(sourceLinesFor('{"a":1,"b":2}', 'json', false)).toHaveLength(1);
    expect(sourceLinesFor('{\n  "a": 1\n}', 'json', false)).toHaveLength(3);
  });

  it('keeps invalid JSON as written rather than failing to render', () => {
    expect(sourceLinesFor('{not json\nat all', 'json', false)).toHaveLength(2);
  });

  it('reports no source lines for a file that renders richly', () => {
    for (const ext of ['md', 'html', 'htm', 'csv', 'slides']) {
      expect(sourceLinesFor('# Title\n\nbody', ext, false), ext).toEqual([]);
    }
  });

  it('numbers a rich format once the source view is on', () => {
    expect(sourceLinesFor('# Title\n\nbody', 'md', true)).toHaveLength(3);
    expect(sourceLinesFor('a,b\n1,2', 'csv', true)).toHaveLength(2);
  });

  it('gives an empty file exactly one line', () => {
    expect(sourceLinesFor('', 'txt', false)).toEqual(['']);
  });
});

// The file editor's button set hinges on the dirty flag: a clean draft offers
// one neutral Close, a dirty draft offers Cancel plus Save. This is what makes
// a successful save (which clears dirty but stays in edit mode) show Close
// again instead of dropping to the read view.
describe('editorToolbarState', () => {
  it('opens the Cancel slot and offers Save while the draft is dirty', () => {
    expect(editorToolbarState(true, false))
      .toEqual({ cancelOpen: true, label: 'save', action: 'save' });
  });

  // Only once the save runs past the delay gate. A save that beats it keeps the
  // Save label, so the text never slides to Saving… and back inside the frozen
  // label box.
  it('reports progress on the primary button once the save is worth showing', () => {
    expect(editorToolbarState(true, true))
      .toEqual({ cancelOpen: true, label: 'saving', action: 'save' });
  });

  it('shuts the Cancel slot and turns the primary button into Close when clean', () => {
    expect(editorToolbarState(false, false))
      .toEqual({ cancelOpen: false, label: 'close', action: 'close' });
  });
});

// One tree in both states is the whole fix for the save-time jank: an element
// preact rebuilds gives a CSS transition no two ends to run between. So these
// assert that both buttons survive every state, and that the state changes
// only the class and the label on them.
describe('EditorToolbar', () => {
  const noop = () => {};
  type Props = Parameters<typeof EditorToolbar>[0];
  const props = (over: Partial<Props>): Props =>
    ({ dirty: false, saving: false, showSaving: false, onClose: noop, onCancel: noop, onSave: noop, ...over });
  const render = (over: Partial<Props>) => vnodeToText(EditorToolbar(props(over)));
  /** The trailing child of the actions row: Save / Saving… / Close. */
  const primary = (over: Partial<Props>) => {
    const actions = EditorToolbar(props(over)) as {
      props: { children: { props: { onClick: () => void } }[] };
    };
    return actions.props.children[1];
  };

  it('keeps Cancel mounted in both states, so the flip has two ends', () => {
    expect(render({ dirty: true })).toContain('action-btn-danger');
    expect(render({ dirty: false })).toContain('action-btn-danger');
  });

  it('opens the Cancel slot only while there are unsaved changes', () => {
    expect(render({ dirty: true })).toContain('file-editor-cancel-slot is-open');
    expect(render({ dirty: false })).not.toContain('is-open');
  });

  // Exactly one label, never a hidden stack of them: a spare label sits in the
  // button's text content, where a text-matching selector reads it as the live
  // one. The CSS reserves the width instead.
  it('renders only the label the state asks for', () => {
    const label = (over: Partial<Props>) =>
      /<span class="file-editor-primary-label">([^<]*)<\/span>/.exec(render(over))?.[1];
    expect(label({ dirty: false })).toBe('Close');
    expect(label({ dirty: true })).toBe('Save');
    expect(label({ dirty: true, showSaving: true })).toBe('Saving…');
  });

  // Inert on the RAW flag. Waiting for the delayed one leaves Cancel live while
  // the write is already on the wire. Exiting there reads as a discard of a
  // file that just got written.
  it('disables every button the instant the save goes out', () => {
    const disabledCount = (render({ dirty: true, saving: true }).match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(2);
  });

  // Inert and dimmed are separate: `.is-saving` is what carries the dim, and it
  // arrives with the Saving… label rather than with the click.
  it('holds the dim back until the saving state is worth showing', () => {
    expect(render({ dirty: true, saving: true })).not.toContain('is-saving');
    expect(render({ dirty: true, saving: true, showSaving: true }))
      .toContain('file-editor-actions is-saving');
  });

  it('disables the shut Cancel button, which is invisible but still mounted', () => {
    const disabledCount = (render({ dirty: false }).match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(1);
  });

  it('wires the primary button to onClose when clean and onSave when dirty', () => {
    const onClose = vi.fn();
    const onCancel = vi.fn();
    const onSave = vi.fn();
    primary({ dirty: false, onClose, onCancel, onSave }).props.onClick();
    expect(onClose).toHaveBeenCalledOnce();
    expect(onSave).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();

    primary({ dirty: true, onClose, onCancel, onSave }).props.onClick();
    expect(onSave).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });
});

// FileEditor holds hooks, so `vnodeToText` cannot render it and these read the
// source instead, the same shape as the prompt's morph tripwires. They pin a
// wiring choice that looks like a detail and is the whole bug. A save to a
// local file returns in tens of milliseconds, so its in-flight state is a
// flash rather than feedback.
describe('FileEditor shows the saving state only when it is worth showing', () => {
  const source = readFileSync(resolve(here, './FilePreviewInline.tsx'), 'utf-8');
  const toolbarCall = /<EditorToolbar[\s\S]*?\/>/.exec(source)?.[0] ?? '';

  it('feeds the toolbar both flags, so inert and dimmed can differ', () => {
    expect(toolbarCall, 'no EditorToolbar call found').toMatch(/\bsaving=\{saving\}/);
    expect(toolbarCall).toMatch(/\bshowSaving=\{showSaving\}/);
  });

  // The delayed flag drops in an effect, one paint late. Without the and, the
  // frame that ends a slow save still says Saving… on a live button.
  it('retires the shown state with the raw one, not a paint later', () => {
    expect(source).toMatch(/const delayElapsed = useDelayedFlag\(saving\)/);
    expect(source).toMatch(/const showSaving = saving && delayElapsed/);
  });

  it('never disables the textarea, which would dim the file for that instant', () => {
    const textarea = /<textarea[\s\S]*?\/>/.exec(source)?.[0] ?? '';
    expect(textarea, 'no textarea found in FilePreviewInline.tsx').toContain('file-editor-textarea');
    expect(textarea).not.toContain('disabled');
  });
});

// The checkbox click's save path: re-read, toggle, write. The returned
// text lets the caller adopt the saved content right away. That is the fix
// for the checkbox blink: the write's own later file-change refresh then
// finds nothing new to show. Only a failure toasts, and the toast key stops
// a burst of failing clicks piling up a wall of error cards.
describe('toggleDataFileCheckbox', () => {
  const PATH = 'artifacts/plan.md';
  const URL = '/data/artifacts/plan.md';
  const TOAST_KEY = `task-toggle:${PATH}`;
  const mockSave = vi.mocked(saveDataFile);

  function stubFetch(body: string) {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve({ ok: true, text: () => Promise.resolve(body) })));
  }

  beforeEach(() => {
    mockSave.mockReset();
    mockSave.mockResolvedValue({ success: true, path: PATH });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    removeToast(TOAST_KEY);
  });

  it('writes the toggled text and returns it, so the caller can adopt it', async () => {
    stubFetch('- [ ] One\n');
    const result = await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(result).toEqual({ content: '- [x] One\n', isLatest: true });
    expect(mockSave).toHaveBeenCalledExactlyOnceWith(PATH, '- [x] One\n');
  });

  it('shows no toast when the save succeeds', async () => {
    stubFetch('- [ ] One\n');
    await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(toasts.value.find((t) => t.key === TOAST_KEY)).toBeUndefined();
  });

  it('clears an earlier failure toast once a later save succeeds', async () => {
    stubFetch('- [ ] One\n');
    mockSave.mockRejectedValueOnce(new Error('disk full'));
    await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(toasts.value.find((t) => t.key === TOAST_KEY)?.type).toBe('error');
    await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(toasts.value.find((t) => t.key === TOAST_KEY)).toBeUndefined();
  });

  it('refuses and toasts an error when the file changed under the click', async () => {
    stubFetch('- [ ] One\n- [ ] Two\n'); // one more task item than expected
    const result = await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(result).toBeNull();
    expect(mockSave).not.toHaveBeenCalled();
    expect(toasts.value.find((t) => t.key === TOAST_KEY)?.type).toBe('error');
  });

  it('toasts an error and returns null when the write itself fails', async () => {
    stubFetch('- [ ] One\n');
    mockSave.mockRejectedValue(new Error('disk full'));
    const result = await toggleDataFileCheckbox(PATH, URL, 0, 1);
    expect(result).toBeNull();
    expect(toasts.value.find((t) => t.key === TOAST_KEY)?.type).toBe('error');
  });

  it('collapses a second failure on the same file into the same toast', async () => {
    stubFetch('- [ ] One\n- [ ] Two\n');
    mockSave.mockRejectedValue(new Error('disk full'));
    await toggleDataFileCheckbox(PATH, URL, 0, 2);
    await toggleDataFileCheckbox(PATH, URL, 1, 2);
    const matching = toasts.value.filter((t) => t.key === TOAST_KEY);
    expect(matching).toHaveLength(1);
    expect(matching[0].type).toBe('error');
  });

  // Two different checkboxes in the same file, toggled close enough together
  // to overlap, must not race each other's read-modify-write. A disk stub
  // that saveDataFile actually mutates: an unserialized pair would both read
  // the pre-toggle text and the second save would discard the first toggle.
  //
  // `isLatest` also pins a second-order bug a code review found: only the
  // toggle with nothing queued behind it may adopt its own text, or an
  // earlier one would replace the whole subtree mid-save.
  it('serializes two toggles on the same file so neither discards the other', async () => {
    let disk = '- [ ] One\n- [ ] Two\n';
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve({ ok: true, text: () => Promise.resolve(disk) })));
    mockSave.mockImplementation(async (_path: string, content: string) => {
      disk = content;
      return { success: true, path: PATH };
    });

    const [resultA, resultB] = await Promise.all([
      toggleDataFileCheckbox(PATH, URL, 0, 2),
      toggleDataFileCheckbox(PATH, URL, 1, 2),
    ]);

    expect(resultA).toEqual({ content: '- [x] One\n- [ ] Two\n', isLatest: false });
    expect(resultB).toEqual({ content: '- [x] One\n- [x] Two\n', isLatest: true });
    expect(disk).toBe('- [x] One\n- [x] Two\n');
  });
});
