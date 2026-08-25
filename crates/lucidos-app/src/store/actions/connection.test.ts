import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, activeInlineForm, connectionStatus, focusedThreadId, threadMap, threadsLoaded, toasts, THREAD_EVENTS_FETCH_CONCURRENCY, THREAD_LIST_REFRESH_TOAST_KEY } from '../store';
import type { CredentialRequest, EmailConfirmRequest } from '../types';
import { makeThreadState } from './threads-test-helpers';
import { ApiError } from '../../api/client';

// Mock all external dependencies so handleResume can run in isolation.
vi.mock('../../api/client', async (importActual) => ({
  // `ApiError` and `isTransientFetchError` are pure and are the very thing the
  // thread-list refresh branches on, so they come from the real module rather
  // than a stub that would let the test agree with itself.
  ...(await importActual<typeof import('../../api/client')>()),
  checkHealth: vi.fn().mockResolvedValue({
    status: 'loaded',
    data: { workspace: 'test', workspace_path: '/tmp/test' },
  }),
  API_BASE: 'http://localhost:3000',
}));
vi.mock('./thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  // runResumeSync retries every thread carrying `eventsLoadFailed` through
  // this; the mock proxy throws on an undeclared export the moment that line
  // runs, so omitting it turns a behavioural assertion into a mock error as
  // soon as a fixture sets the flag.
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearThreadFetchGuards: vi.fn(),
  markLoadedThreadsStale: vi.fn(),
}));
vi.mock('./chat-changes', () => ({
  refreshChangesState: vi.fn(),
  clearRestartInFlight: vi.fn(),
  RESTART_LS_KEY: 'restart-required',
}));
vi.mock('./notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));
vi.mock('./preferences', () => ({
  loadPreferences: vi.fn(),
}));

// Import after mocks are set up
const { handleResume } = await import('./connection');
const { loadPreferences } = await import('./preferences');

const emailConfirmForm = {
  type: 'email-confirm' as const,
  request: {
    to: ['test@example.com'],
    subject: 'Test',
    body: 'Hello',
    account: 'work',
    from: 'me@example.com',
  } as EmailConfirmRequest,
};

beforeEach(() => {
  panelOverlay.value = null;
  connectionStatus.value = 'connected';
});

describe('handleResume preserves email-confirm form', () => {
  it('should NOT clear email-confirm form on resume/focus', async () => {
    panelOverlay.value = { type: 'form', form: emailConfirmForm };

    await handleResume();

    expect(activeInlineForm.value).not.toBeNull();
    expect(activeInlineForm.value?.type).toBe('email-confirm');
  });

  it('should preserve the full email draft data on resume', async () => {
    panelOverlay.value = { type: 'form', form: emailConfirmForm };

    await handleResume();

    const form = activeInlineForm.value;
    expect(form?.type).toBe('email-confirm');
    if (form?.type === 'email-confirm') {
      expect(form.request.to).toEqual(['test@example.com']);
      expect(form.request.subject).toBe('Test');
    }
  });
});

const credentialRequestForm = {
  type: 'credential' as const,
  request: {
    service: 'helius',
    base_url: 'https://api.helius.xyz',
    auth_type: 'api_key' as const,
    prompt: 'Paste your Helius API key.\n1. Go to https://dev.helius.xyz/dashboard\n2. Copy API Key',
  } as CredentialRequest,
};

describe('handleResume preserves credential request form', () => {
  it('should NOT clear credential request form on resume/focus', async () => {
    panelOverlay.value = { type: 'form', form: credentialRequestForm };

    await handleResume();

    // User often takes a screenshot, switches tabs, or alt-tabs while filling
    // out credentials — the panel must survive every focus event. The data
    // lives on panelOverlay (and is persisted in the nav stack), so resync
    // does not need to "refetch" it from the original SSE event.
    expect(activeInlineForm.value).not.toBeNull();
    expect(activeInlineForm.value?.type).toBe('credential');
  });

  it('should preserve the full credential request prompt and instructions on resume', async () => {
    panelOverlay.value = { type: 'form', form: credentialRequestForm };

    await handleResume();

    const form = activeInlineForm.value;
    expect(form?.type).toBe('credential');
    if (form?.type === 'credential') {
      expect(form.request?.service).toBe('helius');
      expect(form.request?.prompt).toContain('1. Go to https://dev.helius.xyz/dashboard');
      expect(form.request?.prompt).toContain('2. Copy API Key');
    }
  });
});

/**
 * Preferences have no SSE re-trigger of their own beyond `PreferencesChanged`
 * / `LanguageSet` / `TimezoneSet`, and nothing else re-runs the loader once it
 * fails. A resume must reload them, or a fetch cancelled at startup leaves
 * Settings showing every value at its default for the whole page load.
 */
describe('handleResume reloads preferences', () => {
  it('calls loadPreferences on every resume', async () => {
    await handleResume();

    expect(loadPreferences).toHaveBeenCalled();
  });
});

describe('handleResume thread-events refresh', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  function loadedThreads(ids: string[]): void {
    threadMap.value = new Map(ids.map(id => [id, makeThreadState(id, { eventsLoaded: true })]));
  }

  it('refreshes the focused thread and marks the rest instead of fetching them', async () => {
    loadedThreads(['bg-1', 'bg-2', 'focused', 'bg-3']);
    focusedThreadId.value = 'focused';
    const { refreshThreadEvents, markLoadedThreadsStale } = await import('./thread-loading');
    const refresh = refreshThreadEvents as unknown as ReturnType<typeof vi.fn>;
    const mark = markLoadedThreadsStale as unknown as ReturnType<typeof vi.fn>;
    refresh.mockClear();
    mark.mockClear();

    await handleResume();
    for (let i = 0; i < 5; i++) await Promise.resolve();

    // The focused thread is the one whose staleness the user can see, so it is
    // the one worth a request now. Asserted as the SET of ids rather than a
    // count, because these fixture threads hold no events and so also trip
    // `checkConnection`'s separate empty-focused-thread recovery, which
    // refreshes the same id once more.
    expect(new Set(refresh.mock.calls.map(c => c[0]))).toEqual(new Set(['focused']));
    expect(mark).toHaveBeenCalled();
  });

  it('issues no request at all when the user is on the compose view', async () => {
    loadedThreads(Array.from({ length: 20 }, (_, i) => `t${i}`));
    const { refreshThreadEvents, markLoadedThreadsStale } = await import('./thread-loading');
    const refresh = refreshThreadEvents as unknown as ReturnType<typeof vi.fn>;
    const mark = markLoadedThreadsStale as unknown as ReturnType<typeof vi.fn>;
    refresh.mockClear();
    mark.mockClear();

    await handleResume();
    for (let i = 0; i < 40; i++) await Promise.resolve();

    // A wake used to dump one request per loaded thread onto a link it was
    // still re-establishing, each carrying its own 10s client deadline. Bounding
    // that to four at a time queued the burst; marking removes it.
    expect(refresh).not.toHaveBeenCalled();
    expect(mark).toHaveBeenCalled();
  });

  it('still bounds the failed-load retry, which stays eager', async () => {
    threadMap.value = new Map(
      Array.from({ length: 20 }, (_, i) => [`t${i}`, makeThreadState(`t${i}`, { eventsLoadFailed: true })]),
    );
    const { loadThreadEvents } = await import('./thread-loading');
    const load = loadThreadEvents as unknown as ReturnType<typeof vi.fn>;
    load.mockClear();

    let inFlight = 0;
    let peak = 0;
    load.mockImplementation(async () => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      await Promise.resolve();
      inFlight--;
    });

    await handleResume();
    for (let i = 0; i < 40; i++) await Promise.resolve();

    // These are FULL snapshots, up to three attempts each, and the set is
    // largest at exactly the wrong moment. They stay eager because the retry
    // landing is what retracts the load-failure card.
    expect(load).toHaveBeenCalledTimes(20);
    expect(peak).toBeLessThanOrEqual(THREAD_EVENTS_FETCH_CONCURRENCY);
    load.mockReset();
    load.mockResolvedValue(undefined);
  });
});

describe('handleResume thread-list refresh', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    toasts.value = [];
    connectionStatus.value = 'connected';
    // A resume happens on a client that already has its list. Load-bearing for
    // these two: with it false, `checkConnection`'s separate cold-start retry
    // (`!threadsLoaded`) issues its OWN `loadAllThreads` first, deliberately
    // swallows the rejection, and the resume refresh below then gets the mock's
    // default resolve, so both cases would pass without exercising anything.
    threadsLoaded.value = true;
  });

  /** Drive one resume with `loadAllThreads` rejecting, and let the fire-and-forget
   *  refresh settle (`runResumeSync` does not await it). */
  async function resumeWithFailingThreadList(err: unknown): Promise<void> {
    const { loadAllThreads } = await import('./thread-loading');
    const load = loadAllThreads as unknown as ReturnType<typeof vi.fn>;
    load.mockRejectedValueOnce(err);
    await handleResume();
    for (let i = 0; i < 10; i++) await Promise.resolve();
    load.mockResolvedValue(undefined);
  }

  // The reported iOS PWA card. Over a dropped tunnel the GET hangs rather than
  // refusing, so the 10s client deadline fires while the engine is answering
  // this endpoint in milliseconds. The dot owns a sustained outage; this site
  // must not report the link as a refusal. The rules themselves are covered in
  // thread-list-refresh.test.ts; this pins that the resume site routes through
  // them rather than keeping a catch block of its own.
  it('raises no card when the refresh times out', async () => {
    await resumeWithFailingThreadList(new DOMException('Request timed out after 10000ms', 'TimeoutError'));
    expect(toasts.value.find(t => t.key === THREAD_LIST_REFRESH_TOAST_KEY)).toBeUndefined();
  });

  it('still raises one card when the engine answers and refuses', async () => {
    await resumeWithFailingThreadList(new ApiError(500, 'Failed to get saved threads'));
    const card = toasts.value.find(t => t.key === THREAD_LIST_REFRESH_TOAST_KEY);
    expect(card).toBeDefined();
    expect(card!.message).toContain('Failed to get saved threads');
  });
});
