// @vitest-environment jsdom
/**
 * The action slot's one control (ADR 0573). Alone, it is one button. With a
 * menu, it is a split button on the desktop face. On the compact face it is one round
 * button that opens the menu, face included, on every tap.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ActionSlot, type SlotAction, type SlotOccupant } from '../ActionSlot';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const action = (key: string, label: string, invoke: (() => void) | null = vi.fn()): SlotAction => ({
  key, label, tooltip: label, tone: 'neutral', icon: () => null, invoke,
});

function draw(occupant: SlotOccupant, compact = false) {
  act(() => { render(<ActionSlot occupant={occupant} compact={compact} />, host); });
}

const menuLabels = () => Array.from(document.querySelectorAll('.split-button-menu [role="menuitem"]'))
  .map((b) => b.textContent);

describe('a lone action', () => {
  it('is one button that acts on a press, its word beside its icon', () => {
    const face = action('archive', 'Archive');
    draw({ face, menu: [], menuLabel: '' });
    const button = host.querySelector<HTMLButtonElement>('button.action-slot')!;
    expect(button.querySelector('.action-slot-label')!.textContent).toBe('Archive');
    expect(button.hasAttribute('data-thread-action')).toBe(true);
    act(() => { button.click(); });
    expect(face.invoke).toHaveBeenCalledTimes(1);
  });

  it('shows a spinner, disabled, while busy', () => {
    draw({ face: action('applying', 'Apply', null), menu: [], menuLabel: '' });
    const button = host.querySelector<HTMLButtonElement>('button.action-slot')!;
    expect(button.disabled).toBe(true);
    expect(button.querySelector('.action-slot-icon .mini-spinner')).not.toBeNull();
  });
});

describe('a held-back face', () => {
  it('reads disabled on its split face and still takes the press that says why', () => {
    const face = { ...action('archive-blocked', 'Archive'), heldBack: true };
    draw({ face, menu: [action('bring-back', 'Bring back')], menuLabel: 'More close actions' });
    const primary = host.querySelector<HTMLButtonElement>('.split-button-primary')!;
    expect(primary.getAttribute('aria-disabled')).toBe('true');
    act(() => { primary.click(); });
    expect(face.invoke).toHaveBeenCalledTimes(1);
  });
});

describe('an action with a menu', () => {
  const occupant = () => ({
    face: action('apply', 'Apply'),
    menu: [action('archive', 'Archive'), action('discard', 'Discard')],
    menuLabel: 'More change actions',
  });

  it('is a split button on the desktop face: the face acts, the caret opens the rest', () => {
    const o = occupant();
    draw(o);
    act(() => { host.querySelector<HTMLButtonElement>('.split-button-primary')!.click(); });
    expect(o.face.invoke).toHaveBeenCalledTimes(1);
    act(() => { host.querySelector<HTMLButtonElement>('.split-button-caret')!.click(); });
    expect(menuLabels()).toEqual(['Discard', 'Archive']);
  });

  it('opens its menu on a compact tap, face included, and acts on nothing', () => {
    const o = occupant();
    draw(o, true);
    expect(host.querySelector('.split-button-caret')).toBeNull();
    const whole = host.querySelector<HTMLButtonElement>('.split-button-whole')!;
    expect(whole.getAttribute('aria-haspopup')).toBe('menu');
    expect(whole.getAttribute('aria-label')).toBe('Apply. More change actions');
    act(() => { whole.click(); });
    expect(o.face.invoke).not.toHaveBeenCalled();
    // The menu opens upward, so the face sits last, next to the button.
    expect(menuLabels()).toEqual(['Discard', 'Archive', 'Apply']);
    act(() => { document.querySelector<HTMLButtonElement>('.split-button-menu [role="menuitem"]:last-child')!.click(); });
    expect(o.face.invoke).toHaveBeenCalledTimes(1);
  });
});
