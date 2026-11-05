/**
 * A toast is an optional TITLE over a MESSAGE, and each is mounted in its own
 * box. Which box a piece lands in is the layout.
 *
 * The title goes in `.toast-heading`, outside the scroll box, so a scroll to
 * the end of the message still shows what the toast is about. The message goes
 * in `.toast-text`, which reserves no gutter and so draws its scrollbar in the
 * card's right rail. With no title, the message itself is the heading. The
 * geometry is pinned by `styles/__tests__/toast-height-cap.test.ts` and
 * `e2e/toast-scroll-shape.spec.ts`. What is pinned HERE is the split.
 *
 * The icon, the actions row and the close X stay OUTSIDE both boxes. For the
 * icon that keeps the rotating spinner clear of a scroll container. For the
 * other two it keeps them reachable under the height cap.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { INLINE_MESSAGE_MAX_CHARS, ToastList } from '../Toast';
import { toasts, showToast } from '../../../store/store';
// Shared vnode walkers. They live under layout/__tests__ because that is where
// they were first needed, and nothing about them is layout-specific.
import { findByClass, textOf } from '../../layout/__tests__/vnodeWalk';

const TITLE = '12 commits come with the new version';
const TEXT = [
  'No release candidate tonight.',
  '',
  '• header: the unread total rides the brand',
  '• gateway: the pairing screen owns its own boot',
].join('\n');

/** Whether the reader has text selected. The card tap stands down while they do. */
let selecting = false;

beforeEach(() => {
  toasts.value = [];
  selecting = false;
  Object.assign(window, { getSelection: () => ({ isCollapsed: !selecting, anchorNode: {} }) });
});

/** A click whose target is (or is not) inside a link or a button. Enough of an
 *  event for the card handler, which asks its target one question. */
function clickOn(inControl: boolean) {
  return {
    target: { closest: (sel: string) => (inControl && sel.includes('a[href]') ? {} : null) },
    currentTarget: { contains: () => true },
  };
}

/** The card element of the first toast. */
function card() {
  return findByClass(ToastList(), 'toast')[0];
}

describe('a toast is a title that stays over a message that scrolls', () => {
  it('puts the title in the heading and the whole message in the scroll box', () => {
    showToast(TEXT, 'info', { title: TITLE });
    const tree = ToastList();

    const heading = findByClass(tree, 'toast-heading');
    expect(heading).toHaveLength(1);
    expect(textOf(heading[0])).toBe(TITLE);

    const text = findByClass(tree, 'toast-text');
    expect(text).toHaveLength(1);
    // The message arrives whole: its blank line and bullets are text, not
    // structure, so nothing is split out of it.
    expect(textOf(text[0])).toBe(TEXT);
  });

  it('marks only the title as the bold part', () => {
    showToast(TEXT, 'info', { title: TITLE });
    const tree = ToastList();

    expect(findByClass(tree, 'toast-title')).toHaveLength(1);
    expect(textOf(findByClass(tree, 'toast-title')[0])).toBe(TITLE);
  });

  it('draws an untitled one-line toast with no title either', () => {
    showToast('Applied 1 change', 'success');
    expect(findByClass(ToastList(), 'toast-title')).toHaveLength(0);
  });

  it('makes an untitled message the heading, in one box, and not a title', () => {
    showToast(TEXT, 'info');
    const tree = ToastList();

    const heading = findByClass(tree, 'toast-heading');
    expect(heading).toHaveLength(1);
    expect(textOf(heading[0])).toBe(TEXT);
    // Its newlines make no title: an untitled toast is never bold.
    expect(findByClass(tree, 'toast-title')).toHaveLength(0);
    // Nothing is left for a second box, and an empty one would still take the
    // column's gap. The heading scrolls on its own instead.
    expect(findByClass(tree, 'toast-text')).toHaveLength(0);
  });

  it('keeps the icon, the actions and the close X out of both boxes', () => {
    showToast(TEXT, 'info', {
      title: TITLE,
      action: { label: 'Refresh', onClick: () => {} },
      secondaryAction: { label: 'Later', onClick: () => {} },
    });
    const tree = ToastList();

    const body = findByClass(tree, 'toast-body');
    expect(body).toHaveLength(1);
    for (const cls of ['toast-heading', 'toast-text']) {
      expect(findByClass(body[0], cls), `${cls} belongs inside the message column`).toHaveLength(1);
    }
    for (const cls of ['toast-icon', 'toast-actions', 'toast-close']) {
      expect(findByClass(tree, cls), `${cls} is rendered`).toHaveLength(1);
      expect(findByClass(body[0], cls), `${cls} must stay outside the message column`).toHaveLength(0);
    }
  });

  it('makes the whole card the click target on a clickable toast', () => {
    const clicked: string[] = [];
    showToast(TEXT, 'info', { title: TITLE, onClick: () => clicked.push('hit') });
    const c = card();

    // The card holds the icon, the padding and both boxes, so a tap anywhere on
    // it acts. It is no role="button" itself, which would hide the X from a
    // screen reader. A hidden real button stands in for the keyboard.
    expect(c.props['data-toast-tap']).toBe('click');
    expect(c.props.role).toBeUndefined();
    const key = findByClass(ToastList(), 'toast-tap-control');
    expect(key).toHaveLength(1);
    expect(textOf(key[0])).toBe('Open');
    (c.props.onClick as (e: unknown) => void)(clickOn(false));
    expect(clicked).toEqual(['hit']);
  });

  it('stands down for a link or a button, each its own destination', () => {
    // The anchor carries no handler (see `linkifyText`), so the toast's action
    // is the only thing that could eat the click and leave the URL unopened.
    const clicked: string[] = [];
    showToast('Read https://example.com/notes', 'info', { onClick: () => clicked.push('hit') });

    (card().props.onClick as (e: unknown) => void)(clickOn(true));
    expect(clicked).toEqual([]);
  });

  it('stands down for the mouseup that ends a text selection', () => {
    const clicked: string[] = [];
    showToast('Build failed: cargo exited 101', 'info', { onClick: () => clicked.push('hit') });
    selecting = true;

    (card().props.onClick as (e: unknown) => void)(clickOn(false));
    expect(clicked).toEqual([]);
  });

  it('still acts when the selection lies elsewhere on the page', () => {
    const clicked: string[] = [];
    showToast('Applied 1 change', 'success', { onClick: () => clicked.push('hit') });
    selecting = true;

    const outside = { ...clickOn(false), currentTarget: { contains: () => false } };
    (card().props.onClick as (e: unknown) => void)(outside);
    expect(clicked).toEqual(['hit']);
  });

  it('turns a lone neutral action into the card tap and draws no button for it', () => {
    const clicked: string[] = [];
    showToast('Habit Tracker wants to open a page', 'info', {
      action: { label: 'Open', onClick: () => clicked.push('open') },
    });
    const tree = ToastList();

    expect(findByClass(tree, 'toast-actions')).toHaveLength(0);
    // The keyboard still reaches it, under the action's own label.
    expect(textOf(findByClass(tree, 'toast-tap-control')[0])).toBe('Open');
    // The X stays, so Escape and a defer still have a way out.
    expect(findByClass(tree, 'toast-close')).toHaveLength(1);
    (card().props.onClick as (e: unknown) => void)(clickOn(false));
    expect(clicked).toEqual(['open']);
  });

  it('keeps a lone danger action as a button, off the card tap', () => {
    showToast('Applying changes...', 'info', {
      action: { label: 'Cancel', variant: 'danger', onClick: () => {} },
      dismissable: false,
    });
    const c = card();

    expect(findByClass(ToastList(), 'toast-actions')).toHaveLength(1);
    expect(c.props['data-toast-tap']).toBeUndefined();
    expect(c.props.onClick).toBeUndefined();
  });

  it('leaves a passive info toast up on a tap, with no keyboard control', () => {
    showToast('Copied', 'info', { key: 'copied' });
    const c = card();

    expect(c.props['data-toast-tap']).toBeUndefined();
    expect(c.props.onClick).toBeUndefined();
    expect(findByClass(ToastList(), 'toast-tap-control')).toHaveLength(0);
    expect(toasts.value.find((t) => t.key === 'copied')).toBeDefined();
  });
});

/** A long headline in the one-line row is squeezed into a narrow column, with
 *  its scroll box mid-card beside the action. So only a short one gets the row. */
describe('Toast layout: one line only for a short headline', () => {
  const OPEN = { label: 'Open', onClick: () => {} };

  beforeEach(() => { toasts.value = []; });

  it('lays a short headline with one action out on one line', () => {
    showToast('Trigger saved', 'success', { action: OPEN });
    expect(findByClass(ToastList(), 'toast-inline')).toHaveLength(1);
  });

  it('gives a headline too long for the row the block layout', () => {
    showToast('x'.repeat(INLINE_MESSAGE_MAX_CHARS + 1), 'info', { action: OPEN });
    expect(findByClass(ToastList(), 'toast-inline')).toHaveLength(0);
  });

  it('gives a titled toast the block layout, however short', () => {
    showToast('Saved', 'success', { title: 'Trigger', action: OPEN });
    expect(findByClass(ToastList(), 'toast-inline')).toHaveLength(0);
  });

  it('gives a message with a line break the block layout', () => {
    showToast('Saved\nAgain', 'success', { action: OPEN });
    expect(findByClass(ToastList(), 'toast-inline')).toHaveLength(0);
  });
});
