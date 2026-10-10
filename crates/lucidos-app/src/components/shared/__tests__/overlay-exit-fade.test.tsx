// @vitest-environment jsdom
/**
 * Whatever animates in animates out (`.claude/rules/frontend.md` § Every
 * Expand and Collapse Rolls). A backdrop modal fades in with `modal-in`, so it
 * must fade out however it closes: `open` going false, or its caller
 * unmounting the whole <Overlay>, as `ConfirmDialog` does.
 *
 * The exit is a drawing, never the live dialog held open. So the overlay
 * contracts see an instant close: off the stack, inert-behind released, and
 * nothing in the drawing answers a pointer or a query.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { render } from 'preact';
import { Overlay } from '../Overlay';
import { MAX_DRAWN_ELEMENTS, MODAL_FADE_MS } from '../overlayExitDrawing';
import { MODAL_EXIT_DRAWING_CLASS } from '../overlayExitClass';
import { _resetOverlayStackForTesting, overlayStack } from '../../../store/overlayStack';
import { animationSpeed, motionPreference } from '../../../utils/motion';

const here: string = dirname(fileURLToPath(import.meta.url));

function Dialog({ open = true, keepMounted = false, backdrop = true }: {
  open?: boolean; keepMounted?: boolean; backdrop?: boolean;
}) {
  return (
    <Overlay
      open={open}
      onClose={() => {}}
      keepMounted={keepMounted}
      backdrop={backdrop}
      overlayClass="test-overlay"
      panelClass="test-dialog"
      panelRole="dialog"
      dataRole="test-dialog"
    >
      <input data-role="test-input" />
      <iframe src="about:blank" />
    </Overlay>
  );
}

const drawings = () => document.querySelectorAll<HTMLElement>(`.${MODAL_EXIT_DRAWING_CLASS}`);

describe('a backdrop modal fades out', () => {
  let host: HTMLDivElement;
  const motionBefore = motionPreference.value;

  beforeEach(() => {
    vi.useFakeTimers();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    vi.runAllTimers();
    vi.useRealTimers();
    _resetOverlayStackForTesting();
    animationSpeed.value = 0;
    motionPreference.value = motionBefore;
  });

  it('when its caller unmounts it, as ConfirmDialog does', () => {
    render(<Dialog />, host);
    render(null, host);
    const [drawing] = drawings();
    expect(drawing.className).toBe(`modal-overlay test-overlay ${MODAL_EXIT_DRAWING_CLASS}`);
    expect(drawing.querySelector('.test-dialog')).not.toBeNull();
  });

  it('when `open` goes false', () => {
    render(<Dialog />, host);
    render(<Dialog open={false} />, host);
    expect(drawings()).toHaveLength(1);
  });

  it('when `open` goes false on a modal kept mounted', () => {
    render(<Dialog keepMounted />, host);
    render(<Dialog keepMounted open={false} />, host);
    expect(drawings()).toHaveLength(1);
  });

  it('closes the live overlay at once, so the UI behind is live again', () => {
    render(<Dialog />, host);
    expect(overlayStack.value).toHaveLength(1);
    render(null, host);
    expect(overlayStack.value).toHaveLength(0);
    expect(document.documentElement.hasAttribute('data-overlay-open')).toBe(false);
  });

  it('draws a copy nothing can reach: inert, hidden, and unmatched by a lookup', () => {
    render(<Dialog />, host);
    render(null, host);
    const [drawing] = drawings();
    expect(drawing.hasAttribute('inert')).toBe(true);
    expect(drawing.getAttribute('aria-hidden')).toBe('true');
    expect(drawing.querySelector('[data-role], [data-overlay-panel]')).toBeNull();
  });

  it('keeps what the user typed', () => {
    render(<Dialog />, host);
    host.querySelector<HTMLInputElement>('input')!.value = 'Weekly report';
    render(null, host);
    expect(drawings()[0].querySelector('input')!.value).toBe('Weekly report');
  });

  it('never copies an iframe, which would load its page again', () => {
    render(<Dialog />, host);
    render(null, host);
    expect(drawings()[0].querySelector('iframe')).toBeNull();
  });

  it('keeps its paint level', () => {
    render(<Dialog />, host);
    render(null, host);
    expect(drawings()[0].getAttribute('style')).toMatch(/z-index:\s*calc\(var\(--z-modal\)/);
  });

  it('is gone once the fade has run, at the scaled length', () => {
    animationSpeed.value = -10; // 10x slower
    render(<Dialog />, host);
    render(null, host);
    vi.advanceTimersByTime(MODAL_FADE_MS * 5);
    expect(drawings()).toHaveLength(1);
    vi.advanceTimersByTime(MODAL_FADE_MS * 6);
    expect(drawings()).toHaveLength(0);
  });

  it('closes at once under reduced motion', () => {
    motionPreference.value = 'reduce';
    render(<Dialog />, host);
    render(null, host);
    expect(drawings()).toHaveLength(0);
  });

  it('closes a very large modal at once, rather than stall copying it', () => {
    const rows = Array.from({ length: MAX_DRAWN_ELEMENTS + 1 }, (_, i) => <span key={i} />);
    const Big = ({ open }: { open: boolean }) => (
      <Overlay open={open} onClose={() => {}} panelClass="test-dialog">{rows}</Overlay>
    );
    render(<Big open />, host);
    render(<Big open={false} />, host);
    expect(drawings()).toHaveLength(0);
  });

  it('leaves an anchored popover alone', () => {
    render(<Dialog backdrop={false} />, host);
    render(null, host);
    expect(drawings()).toHaveLength(0);
  });

  it('mirrors the length of the entrance it reverses', () => {
    // A pin, since the CSS token and this timer cannot share one definition.
    const base = readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8');
    const seconds = Number(/--duration-fast:\s*calc\(([\d.]+)s/.exec(base)![1]);
    expect(MODAL_FADE_MS).toBe(seconds * 1000);
  });

  it('is styled click-through by the class it wears', () => {
    // A pin, since CSS cannot import the class name. Without the rule the
    // drawing would take the clicks meant for the UI behind it.
    const css = readFileSync(resolve(here, '../../../styles/global/modal-overlay.css'), 'utf-8');
    const rule = new RegExp(`\\.modal-overlay\\.${MODAL_EXIT_DRAWING_CLASS}\\s*\\{([^}]*)\\}`).exec(css);
    expect(rule?.[1]).toMatch(/pointer-events:\s*none/);
  });
});
