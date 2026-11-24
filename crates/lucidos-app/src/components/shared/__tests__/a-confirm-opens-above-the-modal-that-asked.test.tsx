// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { ConfirmDialog } from '../ConfirmDialog';
import { Overlay } from '../Overlay';
import { confirmState, showConfirm } from '../../../store/store';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function OverlayGroup() {
  return (
    <>
      <ConfirmDialog />
      <Overlay open onClose={() => {}} panelClass="release-notice" panelRole="dialog" ariaModal>
        <button>Audit my workspace</button>
      </Overlay>
    </>
  );
}

function paintDepth(el: Element): number {
  const z = (el as HTMLElement).style.zIndex || el.getAttribute('style') || '';
  const m = z.match(/var\(--z-modal\)\s*\+\s*(\d+)/);
  expect(m, `no --z-modal offset on "${z}"`).not.toBeNull();
  return parseInt(m![1], 10);
}

describe('a confirm raised by an open modal', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  });

  it('paints above it, though App mounts the confirm first', async () => {
    render(<OverlayGroup />, host);
    await settled();

    void showConfirm('Replace the draft?', 'Replace', { variant: 'danger' });
    await settled();

    const overlays = host.querySelectorAll('.modal-overlay');
    expect(overlays.length).toBe(2);

    const notice = host.querySelector('.release-notice')!.closest('.modal-overlay')!;
    const confirm = host.querySelector('.confirm-dialog')!.closest('.modal-overlay')!;
    expect(paintDepth(confirm)).toBeGreaterThan(paintDepth(notice));
  });

  it('leaves a lone modal on the floor of the band', async () => {
    render(<OverlayGroup />, host);
    await settled();

    const notice = host.querySelector('.release-notice')!.closest('.modal-overlay')!;
    expect(paintDepth(notice)).toBe(0);
  });
});
