// @vitest-environment jsdom
/** The full-response and steps toggles roll their rows, like the fold does.
 *
 *  A roll needs its row to survive the toggle. So a row that stays drawn must
 *  stay the same DOM node, and a hidden row goes through its `<Disclosure>`.
 *  A remounted row cannot roll. See docs/plans/2026-09-27-turn-toggles-roll.md.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { detailsExpanded, stepsExpanded } from '../../../store/store';
import type { Exchange, StoredEvent } from '../../../store/thread-events';

const at = (seq: number, event: Record<string, unknown>) => ({
  seq,
  event: { created: '2026-01-01T12:00:00Z', _eventId: `e${seq}`, ...event } as StoredEvent,
});

const TURN: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'go', created: '2026-01-01T12:00:00Z', _eventId: 'm1' } as StoredEvent,
  userSeq: 1,
  steps: [
    at(2, { type: 'TextStreamed', text: 'First part.' }),
    at(3, { type: 'ToolCalled', name: 'search', args: {} }),
    at(4, { type: 'ToolResult', name: 'search', result: 'ok' }),
    at(5, { type: 'TextStreamed', text: 'Final answer.' }),
    at(6, { type: 'ResponseGenerated' }),
  ],
};

let host: HTMLDivElement;

beforeEach(() => {
  stepsExpanded.value = true;
  detailsExpanded.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => {
    render(
      <ChatExchange
        exchange={TURN}
        revision={0}
        streamingBuffer=""
        isLast={true}
        threadId="t1"
        threadIsCC={false}
        threadCodingAgent="claude-code"
        threadIdle={true}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
});

afterEach(() => {
  render(null, host);
  host.remove();
  stepsExpanded.value = true;
  detailsExpanded.value = true;
});

const chunk = (text: string) =>
  Array.from(host.querySelectorAll('.response-chunk')).find(c => c.textContent?.includes(text));

describe('the turn toggles', () => {
  it('draws each chunk and the step run inside a Disclosure', () => {
    expect(chunk('First part.')?.closest('.response-content > .disclosure')).not.toBeNull();
    expect(host.querySelector('.response-content > .disclosure .inline-step')).not.toBeNull();
  });

  it('keeps the chunk that stays drawn as the same node when steps hide', () => {
    const answer = chunk('Final answer.');
    act(() => { stepsExpanded.value = false; });
    expect(host.querySelector('.inline-step')).toBeNull();
    expect(chunk('Final answer.')).toBe(answer);
    act(() => { stepsExpanded.value = true; });
    expect(host.querySelector('.inline-step')).not.toBeNull();
    expect(chunk('Final answer.')).toBe(answer);
  });

  it('marks hidden steps with a hairline row of their own, which rolls', () => {
    // The hairline rolls in its own Disclosure, so the gap never jumps.
    expect(host.querySelector('.response-elision')).toBeNull();
    act(() => { stepsExpanded.value = false; });
    expect(host.querySelector('.response-content > .disclosure .response-elision')).not.toBeNull();
    act(() => { stepsExpanded.value = true; });
    expect(host.querySelector('.response-elision')).toBeNull();
  });

  it('keeps the latest answer as the same node when the full response hides', () => {
    const answer = chunk('Final answer.');
    act(() => { detailsExpanded.value = false; });
    expect(chunk('First part.')).toBeUndefined();
    expect(chunk('Final answer.')).toBe(answer);
    act(() => { detailsExpanded.value = true; });
    expect(chunk('First part.')).toBeDefined();
    expect(chunk('Final answer.')).toBe(answer);
  });
});

/** Two runs of steps between three chunks, so the head clamp moves a run. */
const TWO_RUNS: Exchange = {
  ...TURN,
  userSeq: 2,
  steps: [
    at(2, { type: 'TextStreamed', text: 'First part.' }),
    at(3, { type: 'ToolCalled', name: 'search', args: {} }),
    at(4, { type: 'ToolResult', name: 'search', result: 'ok' }),
    at(5, { type: 'TextStreamed', text: 'Middle part.' }),
    at(6, { type: 'ToolCalled', name: 'fetch', args: {} }),
    at(7, { type: 'ToolResult', name: 'fetch', result: 'ok' }),
    at(8, { type: 'TextStreamed', text: 'Final answer.' }),
    at(9, { type: 'ResponseGenerated' }),
  ],
};

describe('the head clamp', () => {
  it('keeps every step row it drew when it uncovers the head', () => {
    const draw = (rowsHidden: number) => act(() => {
      render(
        <ChatExchange
          exchange={TWO_RUNS}
          revision={0}
          streamingBuffer=""
          isLast={true}
          threadId="t1"
          threadIsCC={false}
          threadCodingAgent="claude-code"
          threadIdle={true}
          threadAwaitingAnswer={false}
          threadCanceling={false}
          rowsHidden={rowsHidden}
        />,
        host,
      );
    });
    for (const cut of [1, 2, 3]) {
      draw(cut);
      const before = Array.from(host.querySelectorAll('.inline-step'));
      expect(before.length, `clamp ${cut}`).toBeGreaterThan(0);
      draw(0);
      for (const row of before) expect(row.isConnected, `clamp ${cut}`).toBe(true);
    }
  });
});

/** A turn of steps and nothing else. Hiding steps leaves it no body at all. */
const STEPS_ONLY: Exchange = {
  ...TURN,
  userSeq: 3,
  steps: [
    at(2, { type: 'ToolCalled', name: 'search', args: {} }),
    at(3, { type: 'ToolResult', name: 'search', result: 'ok' }),
  ],
};

describe('a turn of steps alone', () => {
  const realAnimate = (HTMLElement.prototype as { animate?: unknown }).animate;
  const realRect = HTMLElement.prototype.getBoundingClientRect;

  beforeEach(() => {
    // jsdom has no Web Animations and lays nothing out. Without both, every
    // Disclosure snaps shut, and a snap is what this case must not do.
    (HTMLElement.prototype as unknown as { animate: unknown }).animate = () => ({
      cancel: () => {},
      finished: new Promise(() => {}),
    });
    HTMLElement.prototype.getBoundingClientRect = () => ({ top: 10, bottom: 50, height: 40 }) as DOMRect;
    // A fresh mount, not a re-render of the turn the file-level setup drew.
    act(() => { render(null, host); });
    act(() => {
      render(
        <ChatExchange
          exchange={STEPS_ONLY}
          revision={0}
          streamingBuffer=""
          isLast={true}
          threadId="t1"
          threadIsCC={false}
          threadCodingAgent="claude-code"
          threadIdle={true}
          threadAwaitingAnswer={false}
          threadCanceling={false}
        />,
        host,
      );
    });
  });

  afterEach(() => {
    (HTMLElement.prototype as unknown as { animate: unknown }).animate = realAnimate;
    HTMLElement.prototype.getBoundingClientRect = realRect;
  });

  it('rolls its steps away when steps hide, rather than dropping the body', () => {
    const step = host.querySelector('.inline-step');
    expect(step).not.toBeNull();
    act(() => { stepsExpanded.value = false; });
    // Mid-roll: the same row is still drawn, inside a body that takes no input.
    expect(host.querySelector('.inline-step')).toBe(step);
    expect(step!.closest('.response-panel > .disclosure.is-rolling')).not.toBeNull();
  });
});
