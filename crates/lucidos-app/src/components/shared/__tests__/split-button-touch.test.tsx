// @vitest-environment jsdom
/**
 * A phone's round button opens its menu on a touch. WebKit drops the synthetic
 * click with the keyboard up, so the face takes the touch path.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { SplitButton } from '../SplitButton';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function draw() {
  act(() => {
    render(
      <SplitButton
        primaryLabel="Submit"
        primaryClassName="action-btn"
        onPrimary={() => {}}
        caretClassName="action-btn"
        caretAriaLabel="More"
        faceOpensMenu
        menuItems={[{ key: 'discard', label: 'Discard', className: 'action-btn', onClick: () => {} }]}
      />,
      host,
    );
  });
}

/** A tap as WebKit delivers it: `touchend`, then the twin click. */
function tap(el: Element): void {
  act(() => {
    el.dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
    el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  });
}

const row = () => document.querySelector<HTMLButtonElement>('.split-button-menu [role="menuitem"]');

describe('the phone round button on touch', () => {
  it('opens its menu once per tap', () => {
    draw();
    tap(host.querySelector('.split-button-whole')!);
    expect(row()).not.toBeNull();
  });

});
