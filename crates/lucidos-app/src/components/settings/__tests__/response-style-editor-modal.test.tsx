// @vitest-environment jsdom
/**
 * Editing a response style happens in a MODAL, not in a row that opens in
 * place.
 *
 * The reported surface was a phone. An instruction runs to a thousand
 * characters, and the inline form put it in the leftover half of a settings
 * row: too narrow to read, and grown to its own content, so there was no
 * height left to scroll it in either. The modal answers all three, and these
 * are the structural halves of that. The geometry is pinned beside the rules
 * it lives in, `styles/__tests__/response-style-editor-modal.test.ts`.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { ResponseStylesSection } from '../ResponseStylesSection';
import { responseStyles } from '../../../store/store';
import type { ResponseStyle } from '../../../api/types';

function style(over: Partial<ResponseStyle> & { id: string }): ResponseStyle {
  return {
    label: over.id,
    description: 'what it does',
    instruction: '- do a thing',
    source: 'builtin',
    editable: true,
    ...over,
  };
}

const LIBRARY: ResponseStyle[] = [
  style({ id: 'standard', label: 'Standard', instruction: '', editable: false }),
  style({ id: 'concise', label: 'Concise', instruction: '- Lead with the answer.' }),
  style({ id: 'minimal', label: 'Minimal', instruction: '- Answer and stop.' }),
];

/** Preact batches a state write into a microtask, so a click's re-render has
 *  not happened when `click()` returns. */
function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('the response style editor is a modal', () => {
  let host: HTMLElement;

  function panel(): HTMLElement | null {
    return document.body.querySelector<HTMLElement>('[data-role="response-style-editor"]');
  }
  function instruction(): HTMLTextAreaElement {
    const el = panel()?.querySelector<HTMLTextAreaElement>('textarea');
    if (!el) throw new Error('the instruction field is not rendered');
    return el;
  }
  function buttonSaying(text: string, root: ParentNode): HTMLButtonElement {
    const el = [...root.querySelectorAll<HTMLButtonElement>('button')]
      .find((b) => b.textContent?.trim() === text);
    if (!el) throw new Error(`no "${text}" button`);
    return el;
  }
  function addCard(): HTMLButtonElement {
    const el = host.querySelector<HTMLButtonElement>('.list-row-add-card');
    if (!el) throw new Error('the Add Style card is not rendered');
    return el;
  }
  function editRows(): HTMLButtonElement[] {
    return [...host.querySelectorAll<HTMLButtonElement>('.list-row .action-btn')]
      .filter((b) => b.textContent?.trim() === 'Edit');
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    responseStyles.value = { status: 'loaded', data: LIBRARY };
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    responseStyles.value = { status: 'not-loaded' };
  });

  it('draws no form until a style is opened', () => {
    render(<ResponseStylesSection />, host);
    expect(panel()).toBeNull();
    expect(host.querySelector('textarea')).toBeNull();
  });

  it('opens an overlay panel on Edit, and keeps every row in the list', async () => {
    render(<ResponseStylesSection />, host);
    expect(editRows()).toHaveLength(2);

    editRows()[0].click();
    await settled();

    const open = panel();
    expect(open, 'Edit did not open the modal').not.toBeNull();
    // `<Overlay>` owns the dismiss contract, and marks the panel it owns. A
    // form rendered outside one answers neither Escape nor an outside tap.
    expect(open!.hasAttribute('data-overlay-panel')).toBe(true);
    expect(open!.getAttribute('role')).toBe('dialog');
    // The row it was opened from stays put: the editor no longer replaces one,
    // so the list does not reshuffle under the modal.
    expect(editRows()).toHaveLength(2);
  });

  it('loads the style being edited, in a field that scrolls rather than grows', async () => {
    render(<ResponseStylesSection />, host);
    editRows()[0].click();
    await settled();

    expect(instruction().value).toBe('- Lead with the answer.');
    // `AutoTextarea` sizes itself to its content, which leaves nothing to
    // scroll. The class it would carry is the tripwire.
    expect(instruction().className).toBe('style-editor-instruction');
  });

  it('opens empty for Add, under its own title', async () => {
    render(<ResponseStylesSection />, host);
    addCard().click();
    await settled();

    expect(panel()).not.toBeNull();
    expect(instruction().value).toBe('');
    expect(panel()!.querySelector('.style-editor-title')?.textContent).toBe('Add a style');
  });

  it('says what each field is for, and marks its sample text as an example', async () => {
    render(<ResponseStylesSection />, host);
    addCard().click();
    await settled();

    // A bare sample in an empty field read as text already filled in.
    const fields = [...panel()!.querySelectorAll('.style-editor-field')];
    expect(fields).toHaveLength(2);
    for (const field of fields) {
      expect(field.querySelector('.style-editor-field-hint')?.textContent).toBeTruthy();
      const control = field.querySelector('input, textarea');
      expect(control?.getAttribute('placeholder')).toMatch(/^For example:/);
    }
  });

  it('marks an edited shipped style in the list, and no other row', () => {
    responseStyles.value = {
      status: 'loaded',
      data: [
        ...LIBRARY.map((s) => (s.id === 'concise' ? { ...s, source: 'overridden' as const } : s)),
        style({ id: 'board-report', label: 'Board report', source: 'user' }),
      ],
    };
    render(<ResponseStylesSection />, host);

    const marked = [...host.querySelectorAll('.list-row')]
      .filter((row) => row.querySelector('.style-row-edited'))
      .map((row) => row.querySelector('.title')?.firstChild?.textContent);
    expect(marked).toEqual(['Concise']);
  });

  it('closes on Cancel', async () => {
    render(<ResponseStylesSection />, host);
    editRows()[0].click();
    await settled();
    expect(panel()).not.toBeNull();

    buttonSaying('Cancel', panel()!).click();
    await settled();
    expect(panel()).toBeNull();
  });

  it('says why Save is refused, so an unnamed style cannot be stored', async () => {
    render(<ResponseStylesSection />, host);
    addCard().click();
    await settled();

    expect(buttonSaying('Save', panel()!).disabled, 'an unnamed style saved').toBe(true);
    expect(panel()!.querySelector('.style-editor-problem')?.textContent).toContain('name');
  });
});
