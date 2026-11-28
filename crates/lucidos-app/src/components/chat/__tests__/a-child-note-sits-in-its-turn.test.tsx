// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { groupIntoExchanges, type StoredEvent } from '../../../store/thread-events';
import { FILED_REPORT_LINE } from '../ChildCompletionRow';

const host = document.createElement('div');
afterEach(() => { act(() => { render(null, host); }); });

it.each(['ChildReportFiled', 'ChildThreadStopped', 'ChildThreadDetached'] as const)(
  'renders %s between the surrounding replies',
  (type) => {
    const child = { child_thread_id: 'child', child_thread_title: 'Check the venue' };
    const note: StoredEvent = type === 'ChildReportFiled'
      ? { ...child, type, status: 'success', summary: 'It opens at nine.' }
      : { ...child, type };
    const [exchange] = groupIntoExchanges(new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Check on the child' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Checking now.' }],
      [3, note],
      [4, { type: 'CodingAgentTextStreamed', text: 'Back to work.' }],
    ]));
    act(() => {
      render(
        <ChatExchange
          exchange={exchange}
          revision={0}
          streamingBuffer=""
          isLast={true}
          threadId="parent"
          threadIsCC={true}
          sessionCodingAgent="claude-code"
          threadIdle={false}
          threadAwaitingAnswer={false}
          threadCanceling={false}
        />,
        host,
      );
    });

    const body = host.querySelector('.response-body');
    expect(body?.textContent).toMatch(/Checking now\..*Check the venue.*Back to work\./s);
    const row = body?.querySelector('.event-row[data-kind="child"]');
    expect(row?.querySelector('button[data-thread-id="child"]')?.textContent).toBe('Check the venue');
    if (type === 'ChildReportFiled') expect(row?.textContent).toContain(FILED_REPORT_LINE);
  },
);

it('renders live chat text after the note', () => {
  const [exchange] = groupIntoExchanges(new Map<number, StoredEvent>([
    [1, { type: 'MessageReceived', text: 'Check on the child' }],
    [2, { type: 'ChildThreadStopped', child_thread_id: 'child', child_thread_title: 'Check the venue' }],
  ]));
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer="Back to work."
        isLast={true}
        threadId="parent"
        threadIsCC={false}
        sessionCodingAgent="claude-code"
        threadIdle={false}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
  expect(host.querySelector('.response-body')?.textContent).toMatch(/Check the venue.*Back to work\./s);
});
