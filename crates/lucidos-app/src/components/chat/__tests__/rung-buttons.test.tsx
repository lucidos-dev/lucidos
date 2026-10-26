// @vitest-environment jsdom
/**
 * The rung control's boxes: a pill whose middle opens the list and never
 * sends. Compose and a started thread draw the same pill, with ‹ and › at its
 * ends, which step the rung and open no list.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RungButtons } from '../RungButtons';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const shown = { name: 'Opus 5.5', effort: 'medium', position: null, intelligence: null };

function draw(props: Partial<Parameters<typeof RungButtons>[0]> = {}) {
  const onOpen = vi.fn();
  act(() => {
    render(
      <RungButtons shown={shown} expanded={false} onOpen={onOpen} {...props} />,
      host,
    );
  });
  return { onOpen };
}

const steps = () => Array.from(host.querySelectorAll<HTMLButtonElement>('.rung-step'));

describe('the pill', () => {
  it('carries name and effort, and no bars', () => {
    draw();
    const trigger = host.querySelector('.rung-pill .rung-trigger')!;
    expect(trigger.querySelector('svg.rung-menu-icon')).toBeNull();
    expect(trigger.querySelector('.rung-name')!.textContent).toBe('Opus 5.5');
    expect(trigger.querySelector('.rung-effort')!.textContent).toBe(' · medium');
  });

  it('is unseen until it has a face, then drops the mark', () => {
    draw({ shown: null });
    const pill = host.querySelector('.rung-pill')!;
    expect(pill.hasAttribute('data-pending')).toBe(true);
    draw({ shown });
    expect(pill.hasAttribute('data-pending')).toBe(false);
  });

  it('is never the send button', () => {
    draw();
    expect(host.querySelector('.send-cancel-morph')).toBeNull();
    expect(host.querySelector('[aria-label="Send message"]')).toBeNull();
  });

  it('opens the list on a press', () => {
    const { onOpen } = draw();
    act(() => { host.querySelector<HTMLButtonElement>('.rung-trigger')!.click(); });
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('names the pair for a screen reader', () => {
    draw();
    expect(host.querySelector('.rung-trigger')!.getAttribute('aria-label'))
      .toBe('Model and effort: Opus 5.5, medium. Choose another');
  });

  it('holds an empty label box until the commands have loaded, so the pill never widens', () => {
    draw({ shown: null });
    expect(host.querySelector('.rung-label')!.textContent).toBe('');
  });

  it('badges the rung with its intelligence band, and says its place to a screen reader', () => {
    draw({ shown: { ...shown, position: { at: 3, of: 7 }, intelligence: '1.12×' } });
    expect(host.querySelector('.rung-intelligence')!.textContent).toBe('1.12×');
    expect(host.querySelector('.rung-trigger')!.getAttribute('aria-label'))
      .toBe('Model and effort: Opus 5.5, medium, rung 3 of 7. Choose another');
  });

  it('draws no badge for a pair with no estimate', () => {
    draw({ shown: { ...shown, position: { at: 3, of: 7 } } });
    expect(host.querySelector('.rung-intelligence')).toBeNull();
  });

  it('names the pair in full in its tooltip, which the label may cut short', () => {
    draw();
    expect(host.querySelector('.rung-trigger')!.getAttribute('data-tooltip')).toMatch(/^Opus 5\.5 · medium/);
  });


  it('states whether the list is open', () => {
    draw({ expanded: true });
    expect(host.querySelector('.rung-trigger')!.getAttribute('aria-expanded')).toBe('true');
  });

  it('is a measured member of the row, which folds it only when told', () => {
    draw();
    const root = host.querySelector('.rung-control')!;
    expect(root.hasAttribute('data-row-item')).toBe(true);
    expect(root.hasAttribute('data-fold-key')).toBe(false);
  });
});

describe('the steps', () => {
  const stepper = () => {
    const prev = vi.fn();
    const next = vi.fn();
    return { prev, next, step: { prev, next } };
  };

  it('sit at the ends of the pill, ‹ before the trigger and › after it', () => {
    draw();
    const [prev, next] = steps();
    const trigger = host.querySelector('.rung-trigger')!;
    expect(prev.nextElementSibling).toBe(trigger);
    expect(trigger.nextElementSibling).toBe(next);
    expect(prev.getAttribute('aria-label')).toBe('Previous model and effort');
    expect(next.getAttribute('aria-label')).toBe('Next model and effort');
  });

  it('step back from ‹ and on from ›, and open no list', () => {
    const { prev, next, step } = stepper();
    const { onOpen } = draw({ step });
    act(() => { steps()[0].click(); });
    expect(prev).toHaveBeenCalledTimes(1);
    expect(next).not.toHaveBeenCalled();
    act(() => { steps()[1].click(); });
    expect(next).toHaveBeenCalledTimes(1);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('dim a step the list has no row for, keep it focusable, and do nothing on it', () => {
    const next = vi.fn();
    draw({ step: { prev: null, next } });
    const [first, second] = steps();
    expect(first.getAttribute('aria-disabled')).toBe('true');
    expect(first.disabled).toBe(false);
    expect(second.getAttribute('aria-disabled')).toBe('false');
    act(() => { first.click(); });
    expect(next).not.toHaveBeenCalled();
  });

  it('are dimmed until the commands have loaded', () => {
    draw({ step: undefined });
    expect(steps().map((b) => b.getAttribute('aria-disabled'))).toEqual(['true', 'true']);
  });

  it.each([
    ['the trigger', () => host.querySelector<HTMLButtonElement>('.rung-trigger')!],
    ['‹', () => steps()[0]],
    ['›', () => steps()[1]],
  ])('step on ArrowLeft and ArrowRight from %s', (_name, target) => {
    const { prev, next, step } = stepper();
    draw({ step });
    const press = (key: string) => {
      const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
      act(() => { target().dispatchEvent(event); });
      return event;
    };
    expect(press('ArrowLeft').defaultPrevented).toBe(true);
    expect(prev).toHaveBeenCalledTimes(1);
    expect(press('ArrowRight').defaultPrevented).toBe(true);
    expect(next).toHaveBeenCalledTimes(1);
    expect(press('a').defaultPrevented).toBe(false);
  });
});
