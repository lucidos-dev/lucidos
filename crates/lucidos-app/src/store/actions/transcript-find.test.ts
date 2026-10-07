// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

// jsdom lays nothing out, so every turn would read as hidden.
vi.mock('../../components/chat/scrollState', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/scrollState')>()),
  isElementVisible: () => true,
  stopFollowingBottom: vi.fn(),
}));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  ensureWholeThreadLoaded: vi.fn(async () => true),
}));

import { createPainter, type FindPainter } from '@lucidos/find';
import { markdownText, transcriptPassages, transcriptRenderRequest, transcriptTarget } from './transcript-find';
import { ensureWholeThreadLoaded } from './thread-loading';
import { collapsedExchanges, detailsExpanded, threadMap } from '../store';
import { makeThreadState } from '../__tests__/thread-events-helpers';
import type { ThreadEvent } from '../thread-events';

// jsdom has no layout. Every range sits at the top of the viewport.
Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 10, 10);
Element.prototype.scrollIntoView = () => {};

const ID = 'thread-1';

/** A passage's blocks, without the blank text marked leaves between them. */
const blocks = (text: string) => text.split('\u0000').map((b) => b.trim()).filter(Boolean);

function thread(events: Array<[number, ThreadEvent]>, over: { hasOlderEvents?: boolean } = {}) {
  const state = { ...makeThreadState(new Map(events)), ...over };
  threadMap.value = new Map([[ID, state]]);
  return state;
}

const conversation: Array<[number, ThreadEvent]> = [
  [1, { type: 'MessageReceived', text: 'Where is the apple?' } as ThreadEvent],
  [2, { type: 'TextStreamed', text: 'The **apple** is on the table.\n\n- one apple pie' } as ThreadEvent],
  [3, { type: 'MessageReceived', text: 'And the pear?' } as ThreadEvent],
  [4, { type: 'TextStreamed', text: 'No pear here.' } as ThreadEvent],
];

/** A painter that records what it was asked to paint. */
function recordingPainter(): FindPainter & { painted: Array<{ ranges: string[]; current: number }> } {
  const painted: Array<{ ranges: string[]; current: number }> = [];
  return {
    painted,
    paint: (ranges, current) => painted.push({ ranges: ranges.map((r) => r.toString()), current }),
    clear: () => painted.push({ ranges: [], current: -1 }),
  };
}

/** Draw the conversation's turns the way the transcript does. */
function drawTurns() {
  document.body.innerHTML = `<div class="thread-view"><div class="thread-content">
    <div class="chat-exchange" data-thread-id="${ID}" data-user-seq="1">
      <div class="initiator-panel"><div class="initiator-body"><div class="user-bubble"><div class="markdown-content"><p>Where is the apple?</p></div></div></div></div>
      <div class="response-chunk" data-text-seq="2"><p>The <strong>apple</strong> is on the table.</p><ul><li>one apple pie</li></ul></div>
    </div>
    <div class="chat-exchange" data-thread-id="${ID}" data-user-seq="3">
      <div class="initiator-panel"><div class="initiator-body"><div class="markdown-content"><p>And the pear?</p></div></div></div>
      <div class="response-chunk" data-text-seq="4"><p>No pear here.</p></div>
      <div class="side-question"><div class="user-bubble"><div class="markdown-content"><p>an apple aside</p></div></div></div>
    </div>
  </div></div>`;
}

beforeEach(() => {
  document.body.innerHTML = '';
  collapsedExchanges.value = new Set();
  transcriptRenderRequest.value = null;
  vi.mocked(ensureWholeThreadLoaded).mockClear();
});

describe('transcriptPassages', () => {
  it('reads every user message and reply chunk, in order, as drawn text', () => {
    const passages = transcriptPassages(thread(conversation), markdownText);
    expect(passages.map((p) => [p.userSeq, p.part, blocks(p.text)])).toEqual([
      [1, { kind: 'user' }, ['Where is the apple?']],
      [1, { kind: 'reply', seqs: [2], needsDetails: false }, ['The apple is on the table.', 'one apple pie']],
      [3, { kind: 'user' }, ['And the pear?']],
      [3, { kind: 'reply', seqs: [4], needsDetails: false }, ['No pear here.']],
    ]);
  });
});

describe('markdownText', () => {
  it('drops the markup and keeps blocks apart', () => {
    expect(blocks(markdownText('A *b*\n\n# c'))).toEqual(['A b', 'c']);
  });
});

describe('the transcript target', () => {
  it('counts the whole thread, drawn or not, and moves nothing while typing', async () => {
    thread(conversation);
    const painter = recordingPainter();
    const result = await transcriptTarget(ID, painter).run('apple');
    expect(result).toEqual({ total: 3, current: 0, capped: false });
    expect(transcriptRenderRequest.value).toBeNull();
  });

  it('fetches history it has not loaded, once per session', async () => {
    thread(conversation, { hasOlderEvents: true });
    const target = transcriptTarget(ID, recordingPainter());
    await target.run('apple');
    await target.run('apples');
    expect(ensureWholeThreadLoaded).toHaveBeenCalledTimes(1);
  });

  it('highlights the drawn matches as the reader types', async () => {
    thread(conversation);
    drawTurns();
    const painter = recordingPainter();
    await transcriptTarget(ID, painter).run('apple');
    expect(painter.painted[painter.painted.length - 1]).toEqual({ ranges: ['apple', 'apple', 'apple'], current: -1 });
  });

  it('a step asks for its turn, unfolds it, and marks the occurrence it counted', async () => {
    thread(conversation);
    drawTurns();
    collapsedExchanges.value = new Set([`${ID}:1`]);
    const painter = recordingPainter();
    const target = transcriptTarget(ID, painter);
    await target.run('apple');
    const first = await target.run('apple', 1);
    expect(first).toEqual({ total: 3, current: 1, capped: false });
    expect(transcriptRenderRequest.value).toMatchObject({ threadId: ID });
    expect(collapsedExchanges.value.has(`${ID}:1`)).toBe(false);
    expect(painter.painted[painter.painted.length - 1]?.current).toBe(0);

    const third = await target.run('apple', 1).then(() => target.run('apple', 1));
    expect(third?.current).toBe(3);
    expect(painter.painted[painter.painted.length - 1]?.current).toBe(2);
    expect((await target.run('apple', 1))?.current).toBe(1);
  });

  it('finds a message drawn as a plain panel, and leaves uncounted text unpainted', async () => {
    thread(conversation);
    drawTurns();
    const painter = recordingPainter();
    const target = transcriptTarget(ID, painter);
    // Turn 3 draws its message without a bubble, as an API caller's would.
    expect(await target.run('pear', 1)).toEqual({ total: 2, current: 1, capped: false });
    expect(painter.painted[painter.painted.length - 1]).toEqual({ ranges: ['pear', 'pear'], current: 0 });
    // The side question's "apple" is drawn, but no passage counts it.
    await target.run('apple');
    expect(painter.painted[painter.painted.length - 1].ranges).toHaveLength(3);
  });

  it('turns the full response on only for a chunk that needs it, and hands it back on close', async () => {
    const ref = { request_event_id: 'm1' };
    thread([
      [1, { type: 'MessageReceived', text: 'Go' } as ThreadEvent],
      [2, { type: 'TextStreamed', text: 'An early apple.', ...ref } as ThreadEvent],
      [3, { type: 'ToolCalled', name: 'read_file', args: { path: 'a.md' }, ...ref } as ThreadEvent],
      [4, { type: 'ToolResult', name: 'read_file', result: 'ok', ...ref } as ThreadEvent],
      [5, { type: 'TextStreamed', text: 'The last word.', ...ref } as ThreadEvent],
    ]);
    const passages = transcriptPassages(threadMap.value.get(ID)!, markdownText);
    expect(passages.filter((p) => p.part.kind === 'reply').map((p) => p.part))
      .toEqual([{ kind: 'reply', seqs: [2], needsDetails: true }, { kind: 'reply', seqs: [5], needsDetails: false }]);

    document.body.innerHTML = `<div class="chat-exchange" data-thread-id="${ID}" data-user-seq="1">
      <div class="response-chunk" data-text-seq="2"><p>An early apple.</p></div></div>`;
    detailsExpanded.value = false;
    const target = transcriptTarget(ID, recordingPainter());
    await target.run('apple', 1);
    expect(detailsExpanded.value).toBe(true);
    target.clear();
    expect(detailsExpanded.value, 'the reader\'s setting comes back').toBe(false);
  });

  it('never changes the transcript\'s markup', async () => {
    thread(conversation);
    drawTurns();
    const before = document.body.innerHTML;
    const target = transcriptTarget(ID, createPainter(() => false));
    await target.run('apple');
    await target.run('apple', 1);
    target.clear();
    expect(document.body.innerHTML).toBe(before);
  });

  it('reports no matches as zero of zero', async () => {
    thread(conversation);
    expect(await transcriptTarget(ID, recordingPainter()).run('banana')).toEqual({ total: 0, current: 0, capped: false });
  });
});
