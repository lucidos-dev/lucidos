// @vitest-environment jsdom
/** Only agent output wears a header. A turn the agent did not write (the
 *  engine's, the system's, or your own control press) draws like a change card:
 *  the time above a card, and the time opens the route popover.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import type { Exchange, StoredEvent } from '../../../store/thread-events';
import { makeExchange } from '../../../store/__tests__/fixtures';

const TS = '2026-01-01T12:00:00Z';
const ENGINE = (kind: string) => ({ kind: 'engine' as const, reason: { kind } as never });

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function mount(exchange: Exchange, live = false, isContinuableAbort = false): void {
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={live}
        isContinuableAbort={isContinuableAbort}
        threadId="tid"
        threadIsCC={true}
        threadCodingAgent="claude-code"
        threadIdle={!live}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
}

function starter(event: Record<string, unknown>): Exchange {
  return makeExchange({ created: TS, _eventId: 'e-1', ...event } as StoredEvent);
}

describe('a turn the agent did not write', () => {
  it('draws no actor chip, and keeps its time as the popover trigger', () => {
    mount(starter({ type: 'MissingHardeningDetected', origin: ENGINE('missing_hardening') }));
    expect(host.querySelector('.initiator-actor')).toBeNull();
    expect(host.textContent).not.toContain('Lucidos Engine');
    expect(host.querySelector('.initiator-timestamp-button')).not.toBeNull();
  });

  it('turns a one-line starter into a pill, a state and Details', () => {
    mount(starter({ type: 'MissingHardeningDetected', origin: ENGINE('missing_hardening') }));
    expect(host.querySelector('.initiator-summary')).toBeNull();
    const card = host.querySelector('.initiator-body .event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject .event-name')?.textContent).toBe('Hardening missing');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('Done');
    const toggle = card?.querySelector<HTMLButtonElement>('.event-row-fold-toggle');
    expect(toggle?.textContent).toBe('Details');
    act(() => { toggle!.click(); });
    expect(card?.textContent).toContain('Hardening (`/harden`) must run before changes are applied');
  });

  it('folds what the starter carried under Details', () => {
    mount(starter({ type: 'MergeConflictDetected', files: ['src/a.ts'], origin: ENGINE('merge_conflict') }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Merge conflict');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('Done');
    const toggle = card?.querySelector<HTMLButtonElement>('.event-row-fold-toggle');
    expect(toggle?.textContent).toBe('Details');
    act(() => { toggle!.click(); });
    expect(card?.textContent).toContain('src/a.ts');
  });

  it('keeps a card starter as it is, minus the header', () => {
    mount(starter({ type: 'TriggerStarted', trigger_id: 't', trigger_name: 'Daily digest', invocation: { kind: 'Schedule' } }));
    expect(host.querySelector('.initiator-actor')).toBeNull();
    const trigger = host.querySelector('.event-row[data-kind="trigger"]');
    expect(trigger?.querySelector('.event-row-subject')?.textContent).toBe('Trigger fired');
    expect(trigger?.querySelector('.event-row-meta')?.textContent).toContain('Daily digest');
    expect(host.querySelector('.event-row[data-kind="boundary"]')).toBeNull();
  });

  it('draws a system abort as a card, keeping its Continue button', () => {
    mount(starter({ type: 'ResponseAborted', cause: 'process_killed', actor: { kind: 'system' } }));
    expect(host.querySelector('.initiator-actor')).toBeNull();
    expect(host.textContent).not.toContain('System');
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Response interrupted');
  });

  it('draws your own cancel as a card', () => {
    mount(starter({ type: 'ResponseCanceled', cause: 'user_stop', actor: { kind: 'device', device_id: 'my-mac' } }));
    expect(host.querySelector('.initiator-actor')).toBeNull();
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Response canceled');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('You stopped it');
  });

  it('reports the work it asked for while the turn runs, then when it ends', () => {
    const hardening = starter({ type: 'MissingHardeningDetected', origin: ENGINE('missing_hardening') });
    mount(hardening, true);
    expect(host.querySelector('.event-row[data-kind="boundary"] .event-row-state')?.textContent).toBe('Running hardening');
    mount(hardening);
    expect(host.querySelector('.event-row[data-kind="boundary"] .event-row-state')?.textContent).toBe('Done');
  });

  it('names an auto-prompt by what the engine asked for', () => {
    mount(starter({ type: 'UserPromptInjected', text: 'Run /harden before finishing.', mode: 'engine', origin: ENGINE('harden_retrigger') }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Hardening missing');
  });

  it('puts an interrupted response\'s Continue inside its card', () => {
    mount(starter({ type: 'ResponseAborted', cause: 'process_killed', actor: { kind: 'system' } }), false, true);
    const button = host.querySelector('.event-row[data-kind="boundary"] .event-row-actions button');
    expect(button?.textContent).toBe('Continue');
  });

  it('colours a reply replaced by a follow-up as lapsed, like a replaced form', () => {
    mount(starter({ type: 'ResponseCanceled', cause: 'superseded_by_followup', actor: { kind: 'device', device_id: 'my-mac' } }));
    const state = host.querySelector('.event-row[data-kind="boundary"] .event-row-state');
    expect(state?.textContent).toBe('Replaced');
    expect(state?.getAttribute('data-tone')).toBe('lapsed');
  });
});
