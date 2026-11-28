// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { Overlay } from '../Overlay';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function Pair({ anchor, first, second }: { anchor: HTMLElement; first: boolean; second: boolean }) {
  return (
    <>
      <Overlay open={first} onClose={() => {}} anchor={anchor} backdrop={false} panelClass="first">menu</Overlay>
      <Overlay open={second} onClose={() => {}} anchor={anchor} backdrop={false} panelClass="second">detail</Overlay>
    </>
  );
}

describe('overlays sharing an anchor', () => {
  let host: HTMLDivElement | null = null;
  const anchor = document.createElement('button');

  afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    _resetOverlayStackForTesting();
  });

  it('keeps the anchor interactive while the other overlay is still open', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<Pair anchor={anchor} first second={false} />, host);
    render(<Pair anchor={anchor} first second />, host);
    render(<Pair anchor={anchor} first={false} second />, host);
    expect(anchor.hasAttribute('data-overlay-anchor')).toBe(true);
    render(<Pair anchor={anchor} first={false} second={false} />, host);
    expect(anchor.hasAttribute('data-overlay-anchor')).toBe(false);
  });
});
