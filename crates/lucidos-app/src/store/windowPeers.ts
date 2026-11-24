
import { startNativeWindowShownTracking } from '../utils/nativeWindow';
import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { reassignWindowId, touchWindowSeen, windowId } from '../utils/windowStorage';

export interface PeerState {
  id: string;
  nativeShown?: boolean;
  focusedThreadId: string | null;
  appId: string | null;
  authorizing: { provider: string | null } | null;
  activeAt: number;
}

type PeerMessage =
  | {
    type: 'hello' | 'state';
    state: PeerState;
    instance?: string;
    bootedAt?: number;
    alive?: boolean;
  }
  | { type: 'bye'; id: string; instance?: string };

export function chooseWindow(
  windows: readonly PeerState[],
  qualifies: (window: PeerState) => boolean,
): string | null {
  let best: PeerState | null = null;
  for (const window of windows) {
    if (!qualifies(window)) continue;
    if (!best || window.activeAt > best.activeAt
      || (window.activeAt === best.activeAt && window.id < best.id)) {
      best = window;
    }
  }
  return best?.id ?? null;
}

export const PEER_CHANGE_GRACE_MS = 2_000;

interface PeerEntry {
  state: PeerState;
  instance: string;
  previous: PeerState | null;
  changedAt: number;
}

const instance = generateUuid();
const bootedAt = Date.now();
const peers = new Map<string, PeerEntry>();
const watchedInstances = new Set<string>();
const seenListeners = new Set<(id: string) => void>();
let readSelf: () => Omit<PeerState, 'id' | 'activeAt'> = () => ({
  focusedThreadId: null, appId: null, authorizing: null,
});
let activeAt = 0;
let nativeShown: boolean | undefined;
type NativeShownState =
  | { status: 'initializing'; ready: Promise<void> }
  | { status: 'supported' | 'unsupported' }
  | { status: 'failed'; error: unknown };
let nativeShownState: NativeShownState = { status: 'unsupported' };
let initializeNativeShown: () => Promise<void> = async () => {};

export function waitForNativeWindowShown(): Promise<void> { return initializeNativeShown(); }

export function assertNativeWindowShownReady(): void {
  if (nativeShownState.status === 'failed') throw nativeShownState.error;
  if (nativeShownState.status === 'initializing') {
    throw new Error('Native window visibility is still initializing. Retry the interactive request.');
  }
}
let channel: BroadcastChannel | null = null;
let lifetime: AbortController | null = null;
let announced: PeerState | null = null;
let selfPrevious: PeerState | null = null;
let selfChangedAt = 0;
let holdsLifeLock = false;

function selfState(): PeerState {
  return { id: windowId(), activeAt, ...readSelf(), nativeShown };
}

const selfListeners = new Set<() => void>();

export function currentWindowView() {
  return { state: selfState(), previous: selfPrevious, changedAt: selfChangedAt };
}

export function observeWindowState(listener: () => void): () => void {
  selfListeners.add(listener);
  return () => { selfListeners.delete(listener); };
}

function sameView(a: PeerState, b: PeerState): boolean {
  return a.focusedThreadId === b.focusedThreadId && a.appId === b.appId
    && (a.authorizing === null) === (b.authorizing === null)
    && a.authorizing?.provider === b.authorizing?.provider;
}

export function knownWindows(): PeerState[] {
  return [selfState(), ...[...peers.values()].map((entry) => entry.state)];
}

function recentPrevious(id: string, now: number): PeerState | null {
  const [previous, changedAt] = id === windowId()
    ? [selfPrevious, selfChangedAt]
    : [peers.get(id)?.previous ?? null, peers.get(id)?.changedAt ?? 0];
  return previous !== null && now - changedAt < PEER_CHANGE_GRACE_MS ? previous : null;
}

function qualifiesOrJustDid(qualifies: (window: PeerState) => boolean) {
  const now = Date.now();
  return (window: PeerState) => {
    if (qualifies(window)) return true;
    const previous = recentPrevious(window.id, now);
    return previous !== null && qualifies(previous);
  };
}

export function isChosenWindow(qualifies: (window: PeerState) => boolean): boolean {
  return chooseWindow(knownWindows(), qualifiesOrJustDid(qualifies)) === windowId();
}

export function isChosenByPreference(
  ...preferences: Array<(window: PeerState) => boolean>
): boolean {
  return chosenByPreference(knownWindows(), preferences);
}

function chosenByPreference(
  windows: PeerState[],
  preferences: Array<(window: PeerState) => boolean>,
): boolean {
  for (const qualifies of preferences) {
    const chosen = chooseWindow(windows, qualifiesOrJustDid(qualifies));
    if (chosen !== null) return chosen === windowId();
  }
  return false;
}

export function isChosenForInteraction(
  ...preferences: Array<(window: PeerState) => boolean>
): boolean {
  const windows = knownWindows();
  const shown = windows.filter((window) => window.nativeShown !== false);
  return chosenByPreference(shown.length ? shown : windows, preferences);
}

export function isChosenForThread(threadId: string): boolean {
  return isChosenForInteraction((window) => window.focusedThreadId === threadId, () => true);
}

export function whenWindowSeen(id: string, listener: () => void): () => void {
  if (peers.has(id)) {
    listener();
    return () => {};
  }
  const wrapped = (seen: string) => {
    if (seen !== id) return;
    seenListeners.delete(wrapped);
    listener();
  };
  seenListeners.add(wrapped);
  return () => seenListeners.delete(wrapped);
}

function post(type: 'hello' | 'state'): void {
  channel?.postMessage({
    type, state: selfState(), instance, bootedAt, alive: holdsLifeLock,
  } satisfies PeerMessage);
}

export function announceWindow(): void {
  const state = selfState();
  if (announced !== null && !sameView(announced, state)) {
    selfPrevious = announced;
    selfChangedAt = Date.now();
  }
  announced = state;
  for (const listener of selfListeners) listener();
  post('state');
}

function lifeLockName(of: string): string {
  return `lucidos-window-alive:${of}`;
}

function holdLifeLock(signal: AbortSignal): void {
  if (typeof navigator === 'undefined' || !navigator.locks) return;
  navigator.locks.request(lifeLockName(instance), { signal }, () => new Promise<void>((resolve) => {
    holdsLifeLock = true;
    post('state');
    signal.addEventListener('abort', () => resolve());
  })).catch(() => {  });
}

function watchPeer(id: string, peerInstance: string, signal: AbortSignal): void {
  if (!peerInstance || watchedInstances.has(peerInstance)) return;
  if (typeof navigator === 'undefined' || !navigator.locks) return;
  watchedInstances.add(peerInstance);
  navigator.locks.request(lifeLockName(peerInstance), { signal }, () => {
    watchedInstances.delete(peerInstance);
    if (peers.get(id)?.instance === peerInstance) peers.delete(id);
  }).catch(() => watchedInstances.delete(peerInstance));
}

function settleSharedId(otherInstance: string, otherBootedAt: number): void {
  const younger = bootedAt > otherBootedAt
    || (bootedAt === otherBootedAt && instance > otherInstance);
  if (younger && reassignWindowId()) {
    announced = null;
    post('hello');
  } else {
    post('state');
  }
}

function receive(message: PeerMessage): void {
  if (message.type === 'bye') {
    const entry = peers.get(message.id);
    if (entry && (!message.instance || entry.instance === message.instance)) peers.delete(message.id);
    return;
  }
  const { state } = message;
  const peerInstance = message.instance ?? '';
  if (state.id === windowId()) {
    if (peerInstance && peerInstance !== instance) settleSharedId(peerInstance, message.bootedAt ?? 0);
    return;
  }
  const known = peers.get(state.id);
  const changed = known !== undefined && known.instance === peerInstance && !sameView(known.state, state);
  peers.set(state.id, {
    state,
    instance: peerInstance,
    previous: changed ? known.state : known?.previous ?? null,
    changedAt: changed ? Date.now() : known?.changedAt ?? 0,
  });
  if (lifetime && message.alive) watchPeer(state.id, peerInstance, lifetime.signal);
  if (message.type === 'hello') announceWindow();
  for (const listener of [...seenListeners]) listener(state.id);
}

const ACTIVE_ANNOUNCE_INTERVAL_MS = 1_000;

function markActive(event: Event): void {
  const now = Date.now();
  const stale = now - activeAt >= ACTIVE_ANNOUNCE_INTERVAL_MS;
  activeAt = now;
  touchWindowSeen();
  if (stale || event.type === 'focus') announceWindow();
}

export function installWindowPeers(read: typeof readSelf): () => void {
  readSelf = read;
  nativeShown = undefined;
  activeAt = document.hasFocus() ? Date.now() : 0;
  if (typeof BroadcastChannel === 'undefined') return () => {};
  lifetime = new AbortController();
  holdLifeLock(lifetime.signal);
  channel = new BroadcastChannel(`lucidos-window-peers:${WORKSPACE_ID ?? ''}`);
  channel.onmessage = (event: MessageEvent<PeerMessage>) => receive(event.data);
  const sayBye = () => channel?.postMessage({ type: 'bye', id: windowId(), instance } satisfies PeerMessage);
  window.addEventListener('pointerdown', markActive, true);
  window.addEventListener('keydown', markActive, true);
  window.addEventListener('focus', markActive);
  window.addEventListener('pagehide', sayBye);
  announced = selfState();
  post('hello');
  let stopped = false;
  let stopShown = () => {};
  const shownLifetime = new AbortController();
  initializeNativeShown = () => {
    if (stopped) return Promise.reject(new Error('Native window visibility tracking stopped.'));
    if (nativeShownState.status === 'initializing') return nativeShownState.ready;
    if (nativeShownState.status !== 'failed') return Promise.resolve();
    const ready = startNativeWindowShownTracking((shown) => {
      if (stopped) return;
      nativeShown = shown;
      announceWindow();
    }, shownLifetime.signal).then((tracking) => {
      if (stopped) { tracking.stop(); return; }
      stopShown = tracking.stop;
      nativeShownState = { status: tracking.status };
      if (tracking.status === 'unsupported') {
        nativeShown = undefined;
        announceWindow();
      }
    }).catch((cause) => {
      const error = new Error(`Could not initialize native window visibility. Retry the interactive request. ${String(cause)}`);
      if (!stopped) nativeShownState = { status: 'failed', error };
      throw error;
    });
    nativeShownState = { status: 'initializing', ready };
    return ready;
  };
  nativeShownState = { status: 'failed', error: new Error('Native window visibility not initialized.') };
  void initializeNativeShown().catch(() => {});
  return () => {
    stopped = true;
    shownLifetime.abort();
    stopShown();
    sayBye();
    window.removeEventListener('pointerdown', markActive, true);
    window.removeEventListener('keydown', markActive, true);
    window.removeEventListener('focus', markActive);
    window.removeEventListener('pagehide', sayBye);
    channel?.close();
    channel = null;
    lifetime?.abort();
    lifetime = null;
    peers.clear();
    watchedInstances.clear();
    seenListeners.clear();
    selfListeners.clear();
    announced = null;
    selfPrevious = null;
    holdsLifeLock = false;
  };
}
