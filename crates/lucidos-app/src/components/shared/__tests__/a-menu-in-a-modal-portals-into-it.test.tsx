// @vitest-environment jsdom
/**
 * A menu opened inside a backdrop modal mounts in that modal's panel.
 *
 * A portaled menu mounted in `<body>` sits below `--z-modal`. Inside a modal it
 * opened behind the dialog, and a tap on it dismissed the modal. The Tree
 * memory confirm's model picker was the first menu inside a modal.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { Overlay } from '../Overlay';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function Menu() {
  return (
    <Overlay open onClose={() => {}} backdrop={false} portal panelClass="test-menu">
      <button>Option</button>
    </Overlay>
  );
}

describe('a portaled menu', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('mounts in the panel of the modal it opens inside', async () => {
    render(
      <Overlay open onClose={() => {}} panelClass="test-modal" panelRole="dialog" ariaModal>
        <Menu />
      </Overlay>,
      host,
    );
    await settled();
    const menu = document.querySelector('.test-menu');
    expect(menu?.parentElement).toBe(document.querySelector('.test-modal'));
  });

  it('mounts in <body> outside a modal', async () => {
    render(<Menu />, host);
    await settled();
    expect(document.querySelector('.test-menu')?.parentElement).toBe(document.body);
  });
});
