/**
 * Window peers (`docs/glossary.md` § window peers).
 *
 * Every window of a workspace on this device shares one device id, so a
 * device-scoped engine event reaches all of them. The windows tell each other
 * what they show over a `BroadcastChannel`, and each one computes the same
 * winner for an event. So exactly one window acts.
 *
 * A window announces itself on boot and on every change, answers a newcomer's
 * hello, and says goodbye on `pagehide`. Each page load is an INSTANCE, which
 * holds a Web Lock for its life. A peer waits on that lock, so a window that
 * crashed without a goodbye still drops out.
 *
 * Two tabs can share a window id, since a duplicated tab copies
 * `sessionStorage`. The younger instance then takes a new id.
 */

import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { reassignWindowId, touchWindowSeen, windowId } from '../utils/windowStorage';

/** What a window shows, as its peers see it. */
export interface PeerState {
  id: string;
  focusedThreadId: string | null;
  appId: string | null;
  /** The OAuth authorization this window opened and is still waiting on, with
   *  its provider when known. `null` when none. */
  authorizing: { provider: string | null } | null;
  /** When the user last touched this window, in epoch ms. 0 when never. */
  activeAt: number;
}

type PeerMessage =
  | {
    type: 'hello' | 'state';
    state: PeerState;
    instance?: string;
    bootedAt?: number;
    /** The sender holds its life lock, so a peer may wait on it. */
    alive?: boolean;
  }
  | { type: 'bye'; id: string; instance?: string };

/** The window that should act: among the windows `qualifies` accepts, the one
 *  the user touched last. Ties break by id, so every window picks the same.
 *  `null` when none qualifies. Pure, for tests. */
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

/** How long a window still counts for what it showed before its last change.
 *  The window that acts on an event often changes what it shows at once, and
 *  a slower peer must still see the state that won. */
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
let channel: BroadcastChannel | null = null;
let lifetime: AbortController | null = null;
let announced: PeerState | null = null;
let selfPrevious: PeerState | null = null;
let selfChangedAt = 0;
let holdsLifeLock = false;

function selfState(): PeerState {
  return { id: windowId(), activeAt, ...readSelf() };
}

/** Whether two states show the same thing. `activeAt` is not what a window
 *  shows, so a touch is not a change. */
function sameView(a: PeerState, b: PeerState): boolean {
  return a.focusedThreadId === b.focusedThreadId && a.appId === b.appId
    && (a.authorizing === null) === (b.authorizing === null)
    && a.authorizing?.provider === b.authorizing?.provider;
}

/** Every window this one knows, itself included. */
export function knownWindows(): PeerState[] {
  return [selfState(), ...[...peers.values()].map((entry) => entry.state)];
}

/** What `id` showed before its last change, while that is still recent. */
function recentPrevious(id: string, now: number): PeerState | null {
  const [previous, changedAt] = id === windowId()
    ? [selfPrevious, selfChangedAt]
    : [peers.get(id)?.previous ?? null, peers.get(id)?.changedAt ?? 0];
  return previous !== null && now - changedAt < PEER_CHANGE_GRACE_MS ? previous : null;
}

/** `qualifies`, widened to what each window showed just before its last change. */
function qualifiesOrJustDid(qualifies: (window: PeerState) => boolean) {
  const now = Date.now();
  return (window: PeerState) => {
    if (qualifies(window)) return true;
    const previous = recentPrevious(window.id, now);
    return previous !== null && qualifies(previous);
  };
}

/** Does THIS window win among the windows `qualifies` accepts? */
export function isChosenWindow(qualifies: (window: PeerState) => boolean): boolean {
  return chooseWindow(knownWindows(), qualifiesOrJustDid(qualifies)) === windowId();
}

/** The first choice that any window qualifies for decides; this window acts
 *  only if it wins that choice. For "the window showing X, else the window
 *  showing any Y, else the last-touched window". */
export function isChosenByPreference(
  ...preferences: Array<(window: PeerState) => boolean>
): boolean {
  const windows = knownWindows();
  for (const qualifies of preferences) {
    const chosen = chooseWindow(windows, qualifiesOrJustDid(qualifies));
    if (chosen !== null) return chosen === windowId();
  }
  return false;
}

/** Does THIS window act on what `threadId` asked for? The window showing that
 *  thread wins, else the window the user touched last. */
export function isChosenForThread(threadId: string): boolean {
  return isChosenByPreference((window) => window.focusedThreadId === threadId, () => true);
}

/** Call `listener` once a window with `id` is heard from, now or later.
 *  Returns the unsubscribe. */
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

/** Announce what this window shows now. Call whenever it changes. */
export function announceWindow(): void {
  const state = selfState();
  if (announced !== null && !sameView(announced, state)) {
    selfPrevious = announced;
    selfChangedAt = Date.now();
  }
  announced = state;
  post('state');
}

function lifeLockName(of: string): string {
  return `lucidos-window-alive:${of}`;
}

/** Hold this instance's life lock until teardown or the page dies, and say so
 *  once it is held. A peer that waited on a lock not yet taken would get it
 *  at once and read a live window as gone. */
function holdLifeLock(signal: AbortSignal): void {
  if (typeof navigator === 'undefined' || !navigator.locks) return;
  navigator.locks.request(lifeLockName(instance), { signal }, () => new Promise<void>((resolve) => {
    holdsLifeLock = true;
    post('state');
    signal.addEventListener('abort', () => resolve());
  })).catch(() => { /* no lock: a crashed window lingers until it reloads */ });
}

/** Forget `id` once `peerInstance` is gone, goodbye or not. */
function watchPeer(id: string, peerInstance: string, signal: AbortSignal): void {
  if (!peerInstance || watchedInstances.has(peerInstance)) return;
  if (typeof navigator === 'undefined' || !navigator.locks) return;
  watchedInstances.add(peerInstance);
  navigator.locks.request(lifeLockName(peerInstance), { signal }, () => {
    watchedInstances.delete(peerInstance);
    if (peers.get(id)?.instance === peerInstance) peers.delete(id);
  }).catch(() => watchedInstances.delete(peerInstance));
}

/** Another instance claims this window's id. The younger one takes a new id,
 *  and the elder answers so the younger learns of the clash. */
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

/** How often typing and clicking re-announce a window that stays in use. A
 *  switch between windows always announces, through `focus`. */
const ACTIVE_ANNOUNCE_INTERVAL_MS = 1_000;

function markActive(event: Event): void {
  const now = Date.now();
  const stale = now - activeAt >= ACTIVE_ANNOUNCE_INTERVAL_MS;
  activeAt = now;
  touchWindowSeen();
  if (stale || event.type === 'focus') announceWindow();
}

/** Join this workspace's window peers. `read` reports what this window shows.
 *  Returns the teardown. */
export function installWindowPeers(read: typeof readSelf): () => void {
  readSelf = read;
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
  return () => {
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
    announced = null;
    selfPrevious = null;
    holdsLifeLock = false;
  };
}
