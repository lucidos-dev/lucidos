// @vitest-environment jsdom
/**
 * Clicking a GFM task-list checkbox in the rendered markdown preview toggles
 * it in place, for a surface that opts in via `editable`/`onToggleCheckbox`.
 * Everywhere else (no `editable` prop, e.g. a read-only repo-file preview),
 * the checkbox stays exactly as `marked` renders it: disabled.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render } from 'preact';
import { MarkdownDocument } from '../MarkdownDocument';

let host: HTMLElement;

function mount(content: string, props?: { editable?: boolean; onToggleCheckbox?: (i: number) => Promise<boolean> }): HTMLElement {
  host = document.createElement('div');
  document.body.append(host);
  render(<MarkdownDocument content={content} {...props} />, host);
  return host;
}

function checkboxes(el: HTMLElement): HTMLInputElement[] {
  return Array.from(el.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
}

/** `MarkdownDocument`'s checkbox-wiring effect runs on preact's after-paint
 *  queue, which jsdom ticks on a real timer (see `test-setup.ts` and
 *  `useVersionedRefresh.test.tsx`'s own `waitForCalls`). Polls rather than a
 *  fixed delay, so a loaded parallel run never flakes on a short wait. */
async function flushEffects(check: () => boolean): Promise<void> {
  for (let waited = 0; waited < 1000 && !check(); waited += 10) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/** A promise plus its own resolver, so a test can control exactly when the
 *  toggle's save settles. */
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
    // An indented (not fenced) code block: marked renders it as <pre><code>
    // with no checkbox, but taskListToggle.ts's line-scan has no block
    // parser and still counts its "- [ ] ..." line as a task item. Rather
    // than toggle the wrong line on click, every checkbox here must stay
    // disabled.
    const content = '    - [ ] indented, not really a task\n\n- [ ] Real\n';
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const boxes = checkboxes(mount(content, { editable: true, onToggleCheckbox }));
    expect(boxes).toHaveLength(1); // marked renders only the real one
    // Give the effect a chance to run; it must choose NOT to enable it.
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

  it('keeps the browser\'s optimistic check and disables the box during the save', async () => {
    const { promise } = deferred<boolean>();
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].checked).toBe(false);
    boxes[0].click();
    expect(boxes[0].checked).toBe(true); // the browser's own default action
    expect(boxes[0].disabled).toBe(true); // disabled for the round trip
  });

  it('keeps the optimistic state and re-enables on a successful save', async () => {
    const { promise, resolve } = deferred<boolean>();
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    boxes[0].click();
    resolve(true);
    await promise;
    await Promise.resolve(); // flush the handler's own .then()
    expect(boxes[0].checked).toBe(true);
    expect(boxes[0].disabled).toBe(false);
  });

  it('does not re-enable a box mid-save when the parent re-renders with a fresh callback', async () => {
    const { promise } = deferred<boolean>();
    const content = '- [ ] One\n';
    const boxes = checkboxes(mount(content, { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    boxes[0].click();
    expect(boxes[0].disabled).toBe(true); // disabled for the round trip

    // Same content, same host: a parent re-render that hands MarkdownDocument
    // a brand-new (unmemoized) onToggleCheckbox function identity. Only the
    // callback identity differs; `html` is unchanged.
    render(<MarkdownDocument content={content} editable onToggleCheckbox={() => Promise.resolve(true)} />, host);

    expect(boxes[0].disabled).toBe(true); // still mid-flight, not re-enabled
  });

  it('leaves a raw authored <input type=checkbox> alone, and keeps indices correct around it', async () => {
    // Raw HTML in markdown passes through unsanitized for this tag, so a
    // hand-written checkbox renders right alongside real task items. It must
    // never become clickable, and must not shift the real items' indices.
    const content = 'before <input type="checkbox"> after\n\n- [ ] First\n- [ ] Second\n';
    const onToggleCheckbox = vi.fn(() => Promise.resolve(true));
    const host = mount(content, { editable: true, onToggleCheckbox });
    const all = checkboxes(host);
    expect(all).toHaveLength(3);
    await flushEffects(() => !all[1].disabled);

    expect(all[1].disabled).toBe(false); // First
    expect(all[2].disabled).toBe(false); // Second

    // The raw checkbox is left exactly as authored: no `disabled` was added,
    // and (unlike the real task items) clicking it never calls the handler.
    all[0].click();
    expect(onToggleCheckbox).not.toHaveBeenCalled();

    all[2].click();
    expect(onToggleCheckbox).toHaveBeenCalledExactlyOnceWith(1); // Second is task index 1, not 2
  });

  it('reverts the checkbox and re-enables it when the save fails', async () => {
    const { promise, resolve } = deferred<boolean>();
    const boxes = checkboxes(mount('- [x] One\n', { editable: true, onToggleCheckbox: () => promise }));
    await flushEffects(() => !boxes[0].disabled);
    expect(boxes[0].checked).toBe(true);
    boxes[0].click();
    expect(boxes[0].checked).toBe(false); // optimistic uncheck
    resolve(false);
    await promise;
    await Promise.resolve();
    expect(boxes[0].checked).toBe(true); // reverted to the pre-click state
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
    await Promise.resolve(); // the save settles and re-enables the box
    expect(boxes[0].disabled).toBe(false);
    label.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 2 }));
    expect(boxes[0].checked).toBe(true);
    expect(onToggleCheckbox).toHaveBeenCalledOnce();
  });

  it('adds no label where the checkboxes stay disabled', () => {
    const el = mount('- [ ] One\n');
    expect(el.querySelector('label')).toBeNull();
  });

  // A successful save adopts the new text as the `content` prop right away
  // (see `handleToggleCheckbox` in FilePreviewInline.tsx). That swaps this
  // whole subtree for a freshly rendered one, and `marked` renders every
  // checkbox `disabled` by default. The fix is a layout effect, which runs
  // before paint. A plain effect would leave one visible frame where the
  // fresh box is disabled, which read as the checkbox blinking.
  it('never shows a disabled frame when the content prop swaps in the saved text', async () => {
    const onToggleCheckbox = () => Promise.resolve(true);
    const boxes = checkboxes(mount('- [ ] One\n', { editable: true, onToggleCheckbox }));
    await flushEffects(() => !boxes[0].disabled);

    render(<MarkdownDocument content="- [x] One\n" editable onToggleCheckbox={onToggleCheckbox} />, host);
    const freshBoxes = checkboxes(host);

    expect(freshBoxes[0]).not.toBe(boxes[0]); // the subtree really was replaced
    expect(freshBoxes[0].disabled).toBe(false); // never disabled, not even for a frame
    expect(freshBoxes[0].checked).toBe(true);
  });
});
