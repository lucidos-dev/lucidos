/**
 * A toast message is mounted in TWO boxes, and which box a piece lands in is
 * the layout.
 *
 * Line 1 goes in `.toast-heading`, outside the scroll box, so a scroll to the
 * last bullet still shows what the toast is about. The rest goes in
 * `.toast-sections`, which reserves no gutter and so draws its scrollbar in the
 * card's right rail. The geometry that follows is pinned by the CSS scan in
 * `styles/__tests__/toast-height-cap.test.ts` and by
 * `e2e/toast-scroll-shape.spec.ts`. What is pinned HERE is the split itself.
 *
 * The icon, the actions row and the close X stay OUTSIDE both boxes. For the
 * icon that keeps the rotating spinner clear of a scroll container. For the
 * other two it keeps them reachable under the height cap.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { ToastList } from '../Toast';
import { toasts, showToast } from '../../../store/store';
// Shared vnode walkers. They live under layout/__tests__ because that is where
// they were first needed, and nothing about them is layout-specific.
import { findByClass, textOf } from '../../layout/__tests__/vnodeWalk';

/** The shape `composeToastMessage` produces for a structured body: a title
 *  line, then a section title, then bullets. */
const SECTIONED = [
  '12 commits since your running version',
  'New',
  '• header: the unread total rides the brand',
  '• gateway: the pairing screen owns its own boot',
].join('\n');

beforeEach(() => {
  toasts.value = [];
});

describe('the toast message splits into a heading that stays and sections that scroll', () => {
  it('puts line 1 in the heading and the rest in the scroll box', () => {
    showToast(SECTIONED, 'info');
    const tree = ToastList();

    const heading = findByClass(tree, 'toast-heading');
    expect(heading).toHaveLength(1);
    expect(textOf(heading[0])).toBe('12 commits since your running version');

    const sections = findByClass(tree, 'toast-sections');
    expect(sections).toHaveLength(1);
    expect(textOf(sections[0])).toContain('the unread total rides the brand');
    // The heading is not repeated inside the box that scrolls away.
    expect(textOf(sections[0])).not.toContain('commits since');
  });

  it('renders no scroll box for a message that is all heading', () => {
    showToast('Applied 1 change', 'success');
    const tree = ToastList();

    expect(findByClass(tree, 'toast-heading')).toHaveLength(1);
    // A section-less message has nothing to put in it, and an empty box would
    // still take the column's gap. The heading scrolls on its own instead.
    expect(findByClass(tree, 'toast-sections')).toHaveLength(0);
  });

  it('keeps the icon, the actions and the close X out of both boxes', () => {
    showToast(SECTIONED, 'info', { action: { label: 'Open', onClick: () => {} } });
    const tree = ToastList();

    const body = findByClass(tree, 'toast-body');
    expect(body).toHaveLength(1);
    for (const cls of ['toast-heading', 'toast-sections']) {
      expect(findByClass(body[0], cls), `${cls} belongs inside the message column`).toHaveLength(1);
    }
    for (const cls of ['toast-icon', 'toast-actions', 'toast-close']) {
      expect(findByClass(tree, cls), `${cls} is rendered`).toHaveLength(1);
      expect(findByClass(body[0], cls), `${cls} must stay outside the message column`).toHaveLength(0);
    }
  });

  it('makes the whole message the click target on a clickable toast', () => {
    const clicked: string[] = [];
    showToast(SECTIONED, 'info', { onClick: () => clicked.push('hit') });
    const body = findByClass(ToastList(), 'toast-body')[0];

    // Both boxes are inside it. So a tap on a bullet acts like a tap on the
    // heading, and the hover underline covers the whole message.
    expect((body.props.class as string).split(' ')).toContain('toast-clickable');
    (body.props.onClick as () => void)();
    expect(clicked).toEqual(['hit']);
  });
});
