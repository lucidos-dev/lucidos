// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render } from 'preact';
import { MarkdownDocument } from '../MarkdownDocument';
import { toggleTaskListCheckbox } from '../../../utils/taskListToggle';

const NOTES = { kind: 'workspace', path: 'artifacts/notes.md' } as const;

let host: HTMLElement;

function mount(content: string, props?: { editable?: boolean; onToggleCheckbox?: (i: number) => Promise<boolean> }): HTMLElement {
  host = document.createElement('div');
  document.body.append(host);
  render(<MarkdownDocument content={content} location={NOTES} {...props} />, host);
  return host;
}

function checkboxes(el: HTMLElement): HTMLInputElement[] {
  return Array.from(el.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
}

async function flushEffects(check: () => boolean): Promise<void> {
  for (let waited = 0; waited < 1000 && !check(); waited += 10) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('MarkdownDocument task-list checkboxes', () => {
  it('renders disabled when editable is left unset', () => {
    const boxes = checkboxes(mount('- [ ] One\n- [x] Two\n'));
    expect(boxes).toHaveLength(2);
    expect(boxes.every((b) => b.disabled)).toBe(true);
  });

  it('renders disabled when editable but given no handler', () => {
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true }));
    expect(boxes[0].disabled).toBe(true);
  });

  it('fails closed when the rendered count disagrees with the write-side scan', async () => {
    const content = '    - [ ] indented, not really a task\n\n- [ ] Real\n';
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const boxes = checkboxes(mount(content, { editable: true, onToggleCheckbox }));
    expect(boxes).toHaveLength(1);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(boxes[0].disabled).toBe(true);
    boxes[0].click();
    expect(onToggleCheckbox).not.toHaveBeenCalled();
  });

  it('enables every checkbox when editable with a handler', async () => {
    const boxes = checkboxes(mount('- [ ] One\n- [x] Two\n', { editable: true, onToggleCheckbox: () => Promise.resolve(true) }));
    await flushEffects(() => boxes.every((b) => !b.disabled));
    expect(boxes.every((b) => !b.disabled)).toBe(true);
  });

  it('calls onToggleCheckbox with the clicked box\'s 0-based index', async () => {
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const boxes = checkboxes(mount('- [ ] One\n- [ ] Two\n- [ ] Three\n', { editable: true, onToggleCheckbox }));
    await flushEffects(() => !boxes[0].disabled);
    boxes[1].click();
    expect(onToggleCheckbox).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('writes to the rendered task in document order across a fenced block', () => {
    const content = '- [ ] Parent\n  - [ ] Child\n\n```md\n- [ ] Example\n```\n\n- [ ] Last\n';
    const onToggleCheckbox = vi.fn(async (_index: number) => true);
    const boxes = checkboxes(mount(content, { editable: true, onToggleCheckbox }));
    expect(boxes).toHaveLength(3);
    boxes[2].click();
    expect(onToggleCheckbox).toHaveBeenCalledExactlyOnceWith(2);
    expect(toggleTaskListCheckbox(content, onToggleCheckbox.mock.calls[0][0]))
      .toBe(content.replace('- [ ] Last', '- [x] Last'));
  });

  it('keeps the browser\'s optimistic check and disables the box during the save', async () => {
    const { promise } = deferred<boolean>();
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].checked).toBe(false);
    boxes[0].click();
    expect(boxes[0].checked).toBe(true);
    expect(boxes[0].disabled).toBe(true);
  });

  it('keeps the optimistic state and re-enables on a successful save', async () => {
    const { promise, resolve } = deferred<boolean>();
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    boxes[0].click();
    resolve(true);
    await promise;
    await Promise.resolve();
    expect(boxes[0].checked).toBe(true);
    expect(boxes[0].disabled).toBe(false);
  });

  it('does not re-enable a box mid-save when the parent re-renders with a fresh callback', async () => {
    const { promise, resolve } = deferred<boolean>();
    const content = '- [ ] One\n';
    const original = vi.fn(() => promise);
    const replacement = vi.fn(async () => true);
    const boxes = checkboxes(mount(content, { editable: true, onToggleCheckbox: original }));
    await flushEffects(() => !boxes[0].disabled);
    boxes[0].click();
    expect(boxes[0].disabled).toBe(true);

    render(<MarkdownDocument content={content} location={NOTES} editable onToggleCheckbox={replacement} />, host);

    expect(boxes[0].disabled).toBe(true);
    boxes[0].click();
    expect(original).toHaveBeenCalledExactlyOnceWith(0);
    expect(replacement).not.toHaveBeenCalled();

    resolve(true);
    await promise;
    expect(boxes[0].disabled).toBe(false);
    boxes[0].click();
    expect(original).toHaveBeenCalledExactlyOnceWith(0);
    expect(replacement).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('shows a raw authored <input type=checkbox> as text, and keeps indices correct around it', async () => {
    const content = 'before <input type="checkbox"> after\n\n- [ ] First\n- [ ] Second\n';
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const host = mount(content, { editable: true, onToggleCheckbox });
    const all = checkboxes(host);
    expect(all).toHaveLength(2);
    expect(host.textContent).toContain('before <input type="checkbox"> after');
    await flushEffects(() => !all[0].disabled);

    expect(all[0].disabled).toBe(false);
    expect(all[1].disabled).toBe(false);

    all[1].click();
    expect(onToggleCheckbox).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('reverts the checkbox and re-enables it when the save fails', async () => {
    const { promise, resolve } = deferred<boolean>();
    const boxes = checkboxes(mount('- [x] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].checked).toBe(true);
    boxes[0].click();
    expect(boxes[0].checked).toBe(false);
    resolve(false);
    await promise;
    await Promise.resolve();
    expect(boxes[0].checked).toBe(true);
    expect(boxes[0].disabled).toBe(false);
  });

  it('toggles the box when the reader clicks its label text', async () => {
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const el = mount('- [ ] One\n- [ ] Two\n', { editable: true, onToggleCheckbox });
    const boxes = checkboxes(el);
    await flushEffects(() => !boxes[1].disabled);
    const label = boxes[1].closest('label');
    expect(label?.textContent?.trim()).toBe('Two');
    label!.click();
    expect(boxes[1].checked).toBe(true);
    expect(onToggleCheckbox).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('keeps a nested list out of its parent item\'s label', async () => {
    const boxes = checkboxes(mount('- [ ] Parent\n  - [ ] Child\n', { editable: true, onToggleCheckbox: () => Promise.resolve(true) }));
    await flushEffects(() => !boxes[0].disabled);
    const parentLabel = boxes[0].closest('label');
    expect(parentLabel?.textContent?.trim()).toBe('Parent');
    expect(parentLabel?.contains(boxes[1])).toBe(false);
    expect(boxes[1].closest('label')?.textContent?.trim()).toBe('Child');
  });

  it('labels the text of a loose list item, whose box sits in a paragraph', async () => {
    const boxes = checkboxes(mount('- [ ] One\n\n- [ ] Two\n', { editable: true, onToggleCheckbox: () => Promise.resolve(true) }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].closest('label')?.textContent?.trim()).toBe('One');
  });

  it('keeps a heading nested in the item out of its label', async () => {
    const boxes = checkboxes(mount('- [ ] Task\n  # Heading\n', { editable: true, onToggleCheckbox: () => Promise.resolve(true) }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].closest('label')?.textContent?.trim()).toBe('Task');
  });

  it('toggles once, not twice, on a double click that selects a word', async () => {
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox }));
    await flushEffects(() => !boxes[0].disabled);
    const label = boxes[0].closest('label')!;
    label.click();
    await Promise.resolve();
    await Promise.resolve();
    expect(boxes[0].disabled).toBe(false);
    label.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 2 }));
    expect(boxes[0].checked).toBe(true);
    expect(onToggleCheckbox).toHaveBeenCalledOnce();
  });

  it('adds no label where the checkboxes stay disabled', () => {
    const el = mount('- [ ] One\n');
    expect(el.querySelector('label')).toBeNull();
  });

  it('never shows a disabled frame when the content prop swaps in the saved text', async () => {
    const onToggleCheckbox = () => Promise.resolve(true);
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox }));
    await flushEffects(() => !boxes[0].disabled);

    render(<MarkdownDocument content="- [x] One\n" location={NOTES} editable onToggleCheckbox={onToggleCheckbox} />, host);
    const freshBoxes = checkboxes(host);

    expect(freshBoxes[0]).not.toBe(boxes[0]);
    expect(freshBoxes[0].disabled).toBe(false);
    expect(freshBoxes[0].checked).toBe(true);
  });
});
