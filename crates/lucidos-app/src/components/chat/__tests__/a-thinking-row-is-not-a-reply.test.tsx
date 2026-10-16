// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { InlineStep } from '../chat-exchange-parts';
import { Disclosure } from '../../shared/Disclosure';
import { DRAWN_ROW_SELECTOR } from '../scrollState';
import type { ResponseEvent } from '../../../store/types';

/** A submit's landing holds the reader on the live edge until the agent draws a
 *  row (ADR 0080). The Thinking row is not one: an answer resumes the agent
 *  with it at once, and the reply can come many seconds later. Counted, it
 *  ended the hold and the reply landed under the composer. Reported.
 *
 *  Rendered through the real row and its real `<Disclosure>`, because what is
 *  pinned is the selector and the marker agreeing. */

type Step = Extract<ResponseEvent, { type: 'step' }>;

const step = (description: string, outcome: Step['outcome']): Step => ({
  type: 'step', description, tool_name: 'Read', outcome,
});

let host: HTMLElement | null = null;

afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

function drawnRows(steps: Step[], text = false): number {
  host = document.createElement('div');
  document.body.appendChild(host);
  render(
    <div class="chat-exchange">
      <div class="response-content">
        <Disclosure open>
          {steps.map((s, i) => <InlineStep key={i} event={s} />)}
        </Disclosure>
        {text && (
          <Disclosure open>
            <div class="response-chunk">The reply.</div>
          </Disclosure>
        )}
      </div>
    </div>,
    host,
  );
  return host.querySelectorAll(DRAWN_ROW_SELECTOR).length;
}

describe('what counts as the agent having started', () => {
  it('a Thinking row alone is nothing drawn yet', () => {
    expect(drawnRows([step('Thinking', 'pending')])).toBe(0);
  });

  it('the reply under it is', () => {
    expect(drawnRows([step('Thinking', 'success')], true)).toBe(1);
  });

  it('so is the tool call the Thinking row names itself after', () => {
    expect(drawnRows([step('Reading src/app.ts', 'pending')])).toBe(1);
  });

  it('counts the finished steps above a new Thinking row, and not the row', () => {
    expect(drawnRows([step('Reading src/app.ts', 'success'), step('Thinking', 'pending')])).toBe(1);
  });
});
