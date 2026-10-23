// @vitest-environment jsdom
/** A disabled split-button face normally takes nothing. Given press handlers,
 *  such as the composer's side-question hold, it still takes presses, reads
 *  disabled through aria-disabled, and never runs its action. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { SplitButton } from '../SplitButton';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
});

function face(): HTMLButtonElement {
  return host.querySelector<HTMLButtonElement>('.split-button-primary')!;
}

function show(props: { disabled: boolean; onPress?: () => void; onPrimary: () => void }) {
  act(() => {
    render(
      <SplitButton
        primaryLabel="Submit"
        primaryClassName="action-btn action-btn-confirm"
        primaryDisabled={props.disabled}
        primaryPressHandlers={props.onPress ? { onPointerDown: props.onPress } : undefined}
        onPrimary={props.onPrimary}
        caretClassName="action-btn action-btn-confirm"
        caretAriaLabel="More"
        menuItems={[]}
      />,
      host,
    );
  });
}

describe('a disabled split-button face with press handlers', () => {
  it('stays pressable, reads disabled, and never runs its action', () => {
    const onPress = vi.fn();
    const onPrimary = vi.fn();
    show({ disabled: true, onPress, onPrimary });
    expect(face().disabled).toBe(false);
    expect(face().getAttribute('aria-disabled')).toBe('true');
    act(() => {
      face().dispatchEvent(new Event('pointerdown', { bubbles: true }));
      face().click();
    });
    expect(onPress).toHaveBeenCalledTimes(1);
    expect(onPrimary).not.toHaveBeenCalled();
  });

  it('runs its action once enabled, with no aria-disabled left behind', () => {
    const onPrimary = vi.fn();
    show({ disabled: false, onPress: () => {}, onPrimary });
    expect(face().hasAttribute('aria-disabled')).toBe(false);
    act(() => face().click());
    expect(onPrimary).toHaveBeenCalledTimes(1);
  });
});

describe('a disabled split-button face without press handlers', () => {
  it('stays natively disabled, as before', () => {
    show({ disabled: true, onPrimary: () => {} });
    expect(face().disabled).toBe(true);
    expect(face().hasAttribute('aria-disabled')).toBe(false);
  });
});
