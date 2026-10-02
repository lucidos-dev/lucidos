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
    expect(card?.querySelector('.event-row-subject .event-name')?.textContent).toBe('Hardening needed');
    expect(card?.querySelector('.event-row-state')?.textContent).toBe('Done');
    const toggle = card?.querySelector<HTMLButtonElement>('.event-row-fold-toggle');
    expect(toggle?.textContent).toBe('Details');
    act(() => { toggle!.click(); });
    expect(card?.textContent).toContain('Changes must pass hardening, an automatic review and test run, before they are applied.');
    // Plain text, so markdown would show raw: the explainer carries none.
    expect(card?.textContent).not.toContain('`');
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

  // The reported card: "Prompt to the agent", a raw `UserPromptInjected`
  // tooltip, and the model's instructions as the only explanation. It now
  // tells the story, and the model's words sit one fold deeper.
  it('tells a timed-out wait as a story, with the model\'s words folded deeper', () => {
    mount(starter({
      type: 'UserPromptInjected',
      text: 'A subscription you registered has timed out.\n\nTimed out. Report what you were waiting for.',
      mode: 'agent',
      origin: {
        kind: 'engine',
        reason: { kind: 'event_wait', outcome: 'expired', watched: ['BenchSlotReleased'], wait_reason: 'waiting for the bench slot' },
      },
    }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    const pill = card?.querySelector('.event-row-subject .event-name');
    expect(pill?.textContent).toBe('Wait timed out');
    expect(pill?.getAttribute('data-tooltip')).toBe('Lucidos told the agent its wait ran out of time.');
    expect(card?.querySelector('.event-row-meta')?.textContent).toBe('bench slot released');
    expect(card?.textContent).not.toContain('UserPromptInjected');
    expect(card?.textContent).not.toMatch(/prompt to the agent/i);

    act(() => { card!.querySelector<HTMLButtonElement>('.event-row-fold-toggle')!.click(); });
    expect(card?.textContent).toContain(
      'The agent asked Lucidos to tell it when “bench slot released” happened, because: waiting for the bench slot.',
    );
    const toggles = card!.querySelectorAll<HTMLButtonElement>('.event-row-fold-toggle');
    expect(toggles[1]?.textContent).toBe('What the agent was told');
    expect(toggles[1]?.getAttribute('aria-expanded')).toBe('false');
  });

  // An older row has no origin, so its frozen opening sentence names it.
  it('names an older timed-out wait by its shape', () => {
    mount(starter({ type: 'UserPromptInjected', text: 'A subscription you registered has timed out.\n\nTimed out.', mode: 'agent' }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Wait timed out');
    expect(card?.querySelector('.event-row-meta')).toBeNull();
  });

  // A delivery whose matched event scrolled out of the loaded window has no
  // structured card to draw, so it falls back to this one: still a story.
  it('names an older arrival whose matched event is not loaded', () => {
    mount(starter({
      type: 'UserPromptInjected',
      text: 'An event you subscribed to has arrived (you were waiting because: x).\n\n{}',
      mode: 'agent',
      delivered_event_id: 'evt-gone',
    }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Event arrived');
    act(() => { card!.querySelector<HTMLButtonElement>('.event-row-fold-toggle')!.click(); });
    expect(card?.textContent).toContain('It happened, so Lucidos told the agent.');
  });

  it('names an auto-prompt by what the engine asked for', () => {
    mount(starter({ type: 'UserPromptInjected', text: 'Run /harden before finishing.', mode: 'engine', origin: ENGINE('harden_retrigger') }));
    const card = host.querySelector('.event-row[data-kind="boundary"]');
    expect(card?.querySelector('.event-row-subject')?.textContent).toBe('Hardening needed');
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
