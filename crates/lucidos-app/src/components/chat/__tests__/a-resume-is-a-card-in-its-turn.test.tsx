// @vitest-environment jsdom
/** A resume draws no header of its own. It is a card at the top of the reply it
 *  resumed, under that reply's agent header. Before, the engine's chip and a
 *  summary line sat over the reply as a second header for one turn.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import type { Exchange, StoredEvent } from '../../../store/thread-events';
import { makeExchange, step } from '../../../store/__tests__/fixtures';

const TS = '2026-01-01T12:00:00Z';
const RESUME: StoredEvent = { type: 'ContinuationStarted', reason: 'user_clicked_continue', created: TS, _eventId: 'resume-1' };

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function mount(exchange: Exchange, isLast = false): void {
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={isLast}
        threadId="tid"
        threadIsCC={true}
        threadCodingAgent="claude-code"
        threadIdle={true}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
}

describe('a resume turn', () => {
  it('draws no header of its own, and opens the reply with a resume card', () => {
    mount(makeExchange(RESUME, [
      step(1, { type: 'TextStreamed', text: 'Picking up where I left off.', created: TS, _eventId: 't-1' }),
      step(2, { type: 'ResponseGenerated', text: 'Picking up where I left off.', created: TS, _eventId: 'r-1' }),
    ]));
    expect(host.querySelector('.initiator-panel')).toBeNull();
    const card = host.querySelector('.response-panel .response-body .event-row[data-kind="resume"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Resumed');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('After restart');
    expect(host.querySelector('.response-executor-label')?.textContent).toBe('Claude');
  });

  it('keeps the card when the resumed turn produced nothing', () => {
    mount(makeExchange(RESUME, []));
    expect(host.querySelector('.initiator-panel')).toBeNull();
    const card = host.querySelector('.response-panel .event-row[data-kind="resume"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Resumed');
  });

  it('explains what happened under Details', () => {
    mount(makeExchange(RESUME, []));
    const toggle = host.querySelector<HTMLButtonElement>('.event-row[data-kind="resume"] .event-row-fold-toggle');
    expect(toggle?.textContent).toBe('Details');
    act(() => { toggle!.click(); });
    expect(host.querySelector('.event-row[data-kind="resume"]')?.textContent).toContain('You pressed Continue on the stopped reply');
  });

  it('names you when you pressed Continue', () => {
    const pressed: StoredEvent = { ...RESUME, actor: { kind: 'device', device_id: 'my-mac' } };
    mount(makeExchange(pressed, []));
    const card = host.querySelector('.event-row[data-kind="resume"]');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('You continued');
  });
});
