/**
 * An empty Home shows the welcome in the compose layout (ADR 0411): the
 * welcome above a centred prompt, until its first message or a dismissal.
 * `homeWelcomeActive` decides it, and `composeViewActive` follows it, so the
 * prompt's slide and the layout agree by construction.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  composeViewActive,
  focusedThreadId,
  homeWelcomeActive,
  preferences,
  threadMap,
} from '../store';
import type { StoredEvent } from '../thread-events';
import { makeThreadState } from '../actions/threads-test-helpers';

const evt = (o: Record<string, unknown>): StoredEvent => o as StoredEvent;

/** A memory call no thread made, recorded on Home (ADR 0381). */
const COST_ROW = evt({
  type: 'ContextCaptured',
  purpose: 'memory',
  producer: 'memory',
  model: 'test-model',
  context_window: 1000,
  estimated_total_tokens: 120,
  sections: [],
  created: '2026-10-10T08:00:00Z',
});

function seedHome(opts: { events?: Map<number, StoredEvent>; eventsLoaded?: boolean } = {}): void {
  threadMap.value = new Map([
    ['home', makeThreadState('home', {
      meta: { title: 'Home', home: true, section: 'inbox' },
      events: opts.events ?? new Map(),
      eventsLoaded: opts.eventsLoaded ?? true,
    })],
    ['other', makeThreadState('other', { eventsLoaded: true })],
  ]);
}

beforeEach(() => {
  preferences.value = { status: 'loaded', data: {} };
  focusedThreadId.value = 'home';
});

afterEach(() => {
  focusedThreadId.value = null;
  threadMap.value = new Map();
  preferences.value = { status: 'not-loaded' };
});

describe('the empty Home layout', () => {
  it('shows the welcome in the compose layout while Home is empty', () => {
    seedHome();
    expect(homeWelcomeActive.value).toBe(true);
    expect(composeViewActive.value).toBe(true);
  });

  it('still counts as empty when Home holds only cost rows', () => {
    seedHome({ events: new Map([[1, COST_ROW]]) });
    expect(homeWelcomeActive.value).toBe(true);
  });

  it('leaves the compose layout once Home has a message', () => {
    seedHome({
      events: new Map([[1, evt({ type: 'MessageReceived', text: 'hi', _eventId: 'm1', created: '2026-10-10T08:01:00Z' })]]),
    });
    expect(homeWelcomeActive.value).toBe(false);
    expect(composeViewActive.value).toBe(false);
  });

  it('waits for Home\'s events, so a Home with history never flashes the welcome', () => {
    seedHome({ eventsLoaded: false });
    expect(homeWelcomeActive.value).toBe(false);
  });

  it('is off once the welcome is dismissed, and while preferences load', () => {
    seedHome();
    preferences.value = { status: 'loaded', data: { welcome_suggestions_dismissed: 'true' } };
    expect(homeWelcomeActive.value).toBe(false);
    preferences.value = { status: 'loading' };
    expect(homeWelcomeActive.value).toBe(false);
  });

  it('never applies to an ordinary thread', () => {
    seedHome();
    focusedThreadId.value = 'other';
    expect(homeWelcomeActive.value).toBe(false);
    expect(composeViewActive.value).toBe(false);
  });
});
