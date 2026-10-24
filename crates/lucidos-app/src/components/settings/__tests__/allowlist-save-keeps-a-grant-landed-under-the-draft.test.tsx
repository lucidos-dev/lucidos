// @vitest-environment jsdom
/**
 * A Save writes the user's edits onto the file as it is NOW, not onto the
 * file as it was when the editor loaded. A permission granted while the draft
 * was dirty must survive the Save, and a pattern the user deleted must stay
 * deleted.
 *
 * Regression: the re-read pauses while the draft is dirty, and Save wrote the
 * whole file from the loaded snapshot. An "Always allow" clicked meanwhile was
 * silently revoked by the next Save.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const showToast = vi.hoisted(() => vi.fn());
vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showToast,
  showConfirm: () => Promise.resolve(true),
}));

const { permissionGrantsVersion } = await import('../../../store/store');
const { AllowlistEditor } = await import('../AllowlistEditor');

/** Lets pending promises land, then flushes Preact's renders and effects. */
function settled(): Promise<void> {
  return act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => { resolve = r; });
  return { promise, resolve };
}

describe('AllowlistEditor save over a file that moved', () => {
  let host: HTMLElement;
  /** What the engine's allowlist file holds. The grant path writes here. */
  let onDisk: string;
  let saved: string[];

  function buttonSaying(text: string): HTMLButtonElement {
    const el = [...host.querySelectorAll<HTMLButtonElement>('button')]
      .find((b) => b.textContent?.trim() === text);
    if (!el) throw new Error(`no "${text}" button`);
    return el;
  }
  function inputs(): HTMLInputElement[] {
    return [...host.querySelectorAll<HTMLInputElement>('.allowlist-row-input')];
  }
  function rows(): string[] {
    return inputs().map((i) => i.value);
  }
  function type(input: HTMLInputElement, value: string): void {
    input.value = value;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }
  function deleteButtonFor(pattern: string): HTMLButtonElement {
    const el = host.querySelector<HTMLButtonElement>(`button[aria-label="Delete pattern ${pattern}"]`);
    if (!el) throw new Error(`no delete button for "${pattern}"`);
    return el;
  }
  /** The engine appends a grant and broadcasts PermissionGrantsChanged. */
  async function grantLands(pattern: string): Promise<void> {
    onDisk += `${pattern}\n`;
    permissionGrantsVersion.value++;
    await settled();
  }

  async function mount(save: (contents: string) => Promise<void>, load = () => Promise.resolve(onDisk)) {
    await act(() => render(
      <AllowlistEditor
        title="Claude Code permissions"
        anchor="permissions:claude-code"
        description="what it is for"
        placeholder="Bash(npm:*)"
        noun="tool permissions"
        load={load}
        save={save}
      />,
      host,
    ));
    await settled();
  }

  /** The user's dirty draft: add one pattern, delete another. */
  async function editDraft(): Promise<void> {
    buttonSaying('Add pattern').click();
    await settled();
    type(inputs()[inputs().length - 1], 'Bash(ls:*)');
    await settled();
    deleteButtonFor('Python').click();
    await settled();
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    onDisk = '# header\nBash(git:*)\nPython\n';
    saved = [];
    showToast.mockReset();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('keeps a grant that landed while the draft was dirty, with the user\'s edits on top', async () => {
    await mount(async (contents) => { saved.push(contents); onDisk = contents; });
    await editDraft();
    await grantLands('Bash(git status)');
    // Paused while dirty: the grant is not on screen yet.
    expect(rows()).toEqual(['Bash(git:*)', 'Bash(ls:*)']);

    buttonSaying('Save').click();
    await settled();

    expect(saved).toEqual(['# header\nBash(git:*)\nBash(ls:*)\nBash(git status)\n']);
    expect(rows()).toEqual(['Bash(git:*)', 'Bash(ls:*)', 'Bash(git status)']);
    expect(buttonSaying('Save').disabled).toBe(true);
  });

  it('refuses to write when the file cannot be re-read', async () => {
    let loads = 0;
    await mount(
      async (contents) => { saved.push(contents); },
      () => (++loads === 1 ? Promise.resolve(onDisk) : Promise.reject(new Error('engine unreachable'))),
    );
    await editDraft();

    buttonSaying('Save').click();
    await settled();

    expect(saved).toEqual([]);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('engine unreachable'), 'error');
    // The draft is still the user's, so they can retry.
    expect(rows()).toEqual(['Bash(git:*)', 'Bash(ls:*)']);
    expect(buttonSaying('Save').disabled).toBe(false);
  });

  it('carries the merged grant into a draft typed while the save was in flight', async () => {
    const pendingSave = deferred();
    await mount(async (contents) => { saved.push(contents); onDisk = contents; await pendingSave.promise; });
    await editDraft();
    await grantLands('Bash(git status)');

    buttonSaying('Save').click();
    await settled();
    // Typed while the save is still on the wire.
    buttonSaying('Add pattern').click();
    await settled();
    type(inputs()[inputs().length - 1], 'Bash(pwd)');
    await settled();
    pendingSave.resolve();
    await settled();

    expect(rows()).toEqual(['Bash(git:*)', 'Bash(ls:*)', 'Bash(pwd)', 'Bash(git status)']);
    // A second Save must not read the grant's absence as a deletion.
    buttonSaying('Save').click();
    await settled();
    expect(saved[1]).toBe('# header\nBash(git:*)\nBash(ls:*)\nBash(pwd)\nBash(git status)\n');
  });
});
