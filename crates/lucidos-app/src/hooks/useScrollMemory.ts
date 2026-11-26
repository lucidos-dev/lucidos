import { ANCHOR_PREFIX, ROW_PREFIX, LINE_PREFIX, LIVE_EDGE_VALUE, readSavedScroll, type SavedScroll } from '../store/savedScroll';
export { LIVE_EDGE_VALUE, parseSavedScroll, readSavedScroll, type SavedScroll } from '../store/savedScroll';
import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

import {
  deepLinkHasResolved,
  EVENT_RESOLVE_DEADLINE_MS,
  classifyScroll,
  hasPendingEventScroll,
  recordReadingLine,
  markNavigationScroll,
  onDeepLinkClaimed,
  onDeepLinkResolved,
  onFollowRideStarted,
  resumeFollowingBottom,
  applyFollowSeed,
  type FollowResumeFrom,
  type ScrollClass,
} from '../components/chat/scrollState';
import { scrollPosition } from '../components/chat/scrollOwner';
import { onClassifiedScroll } from '../components/chat/classifiedScroll';
import { ANCHOR_ATTR, anchorRow, anchorTargetTop, anchorTurn, anchorTurnIsClamped, lineBlockOf, lineTargetTop, readLineAnchor, readRowAnchor, readScrollAnchor, rowTargetTop, type LineAnchor, type RowAnchor, type ScrollAnchor } from '../components/chat/scrollAnchor';
import { onPageHide, onPageWake } from '../utils/pageVisit';
import { postClientLog } from '../utils/clientLog';
import { watchUserAction } from '../utils/userAction';

export function isFullyRestorable(saved: number, scrollHeight: number, clientHeight: number): boolean {
  if (saved < 0) return false;
  if (saved === 0) return true;
  const max = Math.max(0, scrollHeight - clientHeight);
  return max >= saved;
}

type ContentPlace = Extract<SavedScroll, { kind: 'anchor' | 'row' | 'line' }>;

function namesContent(record: SavedScroll | null): record is ContentPlace {
  return record?.kind === 'anchor' || record?.kind === 'row' || record?.kind === 'line';
}

function turnOfLine(record: LineAnchor): SavedScroll {
  return { kind: 'anchor', eventId: record.eventId, relTop: record.turnRelTop };
}

function formatLineAnchor(anchor: LineAnchor): string {
  return `${LINE_PREFIX}${anchor.relTop}:${anchor.turnRelTop}:${anchor.eventId}:${anchor.path.join('.')}~${anchor.kinds}`;
}

function formatScrollAnchor(anchor: ScrollAnchor): string {
  return `${ANCHOR_PREFIX}${Math.round(anchor.relTop)}:${anchor.eventId}`;
}

function formatRowAnchor(anchor: RowAnchor): string {
  return `${ROW_PREFIX}${Math.round(anchor.relTop)}:${anchor.rowEventId}`;
}

const SAVE_DEBOUNCE_MS = 150;
const CONTENT_READ_THROTTLE_MS = 150;
const RESTORE_DEADLINE_MS = 3000;
const ANCHOR_RESTORE_CEILING_MS = 20_000;
const DEAD_DEEP_LINK_SLACK_MS = 500;

export interface ScrollMemoryOptions {
  paused?: boolean;
  shouldRestore?: () => boolean;
  resetOnEmpty?: boolean;
  followsLiveEdge?: boolean;
  anchorsToContent?: boolean;
  onRestoreSettled?: () => void;
  restoreIsBusy?: () => boolean;
}

export type ScrollMemoryLive = Pick<ScrollMemoryOptions, 'shouldRestore' | 'onRestoreSettled' | 'restoreIsBusy'>;

export function attachScrollMemory(
  el: HTMLElement,
  key: string,
  opts: {
    live: () => ScrollMemoryLive;
    resetOnEmpty?: boolean;
    followsLiveEdge?: boolean;
    anchorsToContent?: boolean;
    isCurrent?: () => boolean;
  },
): () => void {
  const {
    live,
    resetOnEmpty = false,
    followsLiveEdge = false,
    anchorsToContent = false,
    isCurrent,
  } = opts;

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let lastSaved: string | null = null;
  let hasWritten = false;
  const restore = scrollPosition.beginRestore();
  const writeRestore = (top: number) => restore.write(() => markNavigationScroll(el, top));
  let hasSettled = false;
  let observed: string | undefined;
  let contentObserver: MutationObserver | null = null;
  let contentTimer: ReturnType<typeof setTimeout> | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let mutationObserver: MutationObserver | null = null;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  let stopUserWatch: (() => void) | null = null;
  let inheritedBeforeDeepLink: number | null = null;
  const restoreStartedAt = Date.now();
  let lastHeight = el.scrollHeight;

  const readSaved = (): SavedScroll | null => {
    const value = readSavedScroll(key);
    if (value?.kind === 'live-edge' && !followsLiveEdge) return null;
    if (namesContent(value) && !anchorsToContent) return null;
    return value;
  };

  const saved: SavedScroll | null = readSaved();

  let reported = false;
  const reportRestore = (outcome: string) => {
    if (reported || !anchorsToContent) return;
    reported = true;
    postClientLog('scroll', 'restore', { outcome, form: saved?.kind ?? 'none' });
  };

  const settleRestore = () => {
    if (hasSettled) return;
    hasSettled = true;
    if (!isCurrent || isCurrent()) live().onRestoreSettled?.();
  };

  const stopRestore = (settled = true) => {
    restore.finish();
    resizeObserver?.disconnect();
    resizeObserver = null;
    mutationObserver?.disconnect();
    mutationObserver = null;
    if (deadlineTimer !== null) {
      clearTimeout(deadlineTimer);
      deadlineTimer = null;
    }
    stopUserWatch?.();
    stopUserWatch = null;
    if (settled) settleRestore();
  };

  const openIsOurs = () => live().shouldRestore?.() ?? true;

  const replayRecordedFollow = (record: SavedScroll | null, from: FollowResumeFrom) => {
    if (record?.kind === 'live-edge') resumeFollowingBottom(el, from);
  };

  const offsetFor = (record: SavedScroll): number | null => {
    const top = record.kind === 'anchor'
      ? (anchorTurnIsClamped(el, record) ? null : anchorTargetTop(el, record))
      : record.kind === 'row' ? rowTargetTop(el, record)
      : record.kind === 'line' ? lineTargetTop(el, record)
      : record.kind === 'offset' ? record.top
      : null;
    if (top === null) return null;
    if (isFullyRestorable(top, el.scrollHeight, el.clientHeight)) return top;
    if (record.kind === 'offset') return null;
    return Math.max(0, el.scrollHeight - el.clientHeight);
  };

  const placementFor = (record: SavedScroll): { record: SavedScroll; top: number } | null => {
    const top = offsetFor(record);
    if (top !== null) return { record, top };
    return record.kind === 'line' ? placementFor(turnOfLine(record)) : null;
  };

  const placeFromRecord = (record: SavedScroll, top: number) => {
    writeRestore(top);
    const target = record.kind === 'anchor' ? anchorTurn(el, record)
      : record.kind === 'row' ? anchorRow(el, record)
      : record.kind === 'line' ? lineBlockOf(el, record)
      : null;
    if (!target) return;
    recordReadingLine(el, target);
  };

  const positionFromRecord = () => {
    if (saved === null || saved.kind === 'live-edge') return;
    if (saved.kind === 'offset' && saved.top === 0) {
      writeRestore(0);
      restore.finish();
      reportRestore('landed');
      return;
    }
    tryRestore();
  };

  const tryRestore = (final = false) => {
    if (!restore.waiting() || saved === null || saved.kind === 'live-edge') return;
    const placement = placementFor(saved);
    if (placement === null) {
      if (!final) keepWaitingForAnchor();
      return;
    }
    placeFromRecord(placement.record, placement.top);
    reportRestore(el.scrollTop < el.scrollHeight - el.clientHeight - 1 ? 'landed' : 'landed-at-bottom');
    stopRestore();
  };

  const ceilingReached = () => Date.now() - restoreStartedAt >= ANCHOR_RESTORE_CEILING_MS;

  const keepWaitingForAnchor = () => {
    if (!namesContent(saved) || deadlineTimer === null) return;
    if (el.scrollHeight === lastHeight) return;
    lastHeight = el.scrollHeight;
    if (ceilingReached()) return;
    clearTimeout(deadlineTimer);
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
  };

  const onDeadline = () => {
    if (anchorsToContent && el.clientHeight <= 0 && !ceilingReached()) {
      deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
      return;
    }
    if (live().restoreIsBusy?.() && !ceilingReached()) {
      deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
      return;
    }
    tryRestore(true);
    if (restore.waiting()) reportRestore('gave-up');
    stopRestore();
  };

  const standDownForDeepLink = () => {
    reportRestore('deep-link');
    stopRestore(false);
    if (followsLiveEdge) {
      const recorded = readSaved();
      if (recorded === null) applyFollowSeed(el, 'in-place');
      else replayRecordedFollow(recorded, 'in-place');
    }
    if (deepLinkHasResolved()) {
      recordDeepLinkLanding();
      settleRestore();
      return;
    }
    if (inheritedBeforeDeepLink === null) inheritedBeforeDeepLink = el.scrollTop;
    const inherited = inheritedBeforeDeepLink;
    const armDeadline = () => {
      deadlineTimer = setTimeout(() => {
        deadlineTimer = null;
        if (hasPendingEventScroll()) {
          armDeadline();
          return;
        }
        settleRestore();
        if (isCurrent && !isCurrent()) return;
        if (el.scrollTop !== inherited) return;
        if (!openIsOurs()) return;
        const recorded = readSaved();
        const rescue = recorded === null ? null : placementFor(recorded);
        if (recorded?.kind === 'live-edge') {
          resumeFollowingBottom(el);
        } else if (rescue !== null) {
          placeFromRecord(rescue.record, rescue.top);
        } else if (resetOnEmpty) {
          writeRestore(0);
        }
      }, EVENT_RESOLVE_DEADLINE_MS + DEAD_DEEP_LINK_SLACK_MS);
    };
    armDeadline();
  };

  const writeNow = () => {
    const next = observed;
    if (next === undefined) return;
    if (hasWritten && next === lastSaved) return;
    lastSaved = next;
    hasWritten = true;
    try {
      localStorage.setItem(key, next);
    } catch {  }
  };

  const scheduleSave = () => {
    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(writeNow, SAVE_DEBOUNCE_MS);
  };

  const currentPlace = (): string | null => {
    if (anchorsToContent) {
      const line = readLineAnchor(el);
      if (line) return formatLineAnchor(line);
      const row = readRowAnchor(el);
      if (row) return formatRowAnchor(row);
      const anchor = readScrollAnchor(el);
      if (anchor) return formatScrollAnchor(anchor);
      if (el.children?.length) return null;
    }
    return String(Math.floor(el.scrollTop));
  };

  const currentPosition = (scroll: ScrollClass = classifyScroll(el)): string | null =>
    followsLiveEdge && scroll.followSurvives ? LIVE_EDGE_VALUE : currentPlace();

  const recordDeepLinkLanding = () => {
    if (isCurrent && !isCurrent()) return;
    const next = currentPosition();
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  const recordContentPlace = () => {
    if (observed === undefined) return;
    if (restore.waiting()) return;
    if (isCurrent && !isCurrent()) return;
    const next = currentPosition();
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  const flush = () => {
    if (contentTimer !== null) {
      clearTimeout(contentTimer);
      contentTimer = null;
      recordContentPlace();
    }
    if (saveTimer !== null) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    writeNow();
  };

  const onScroll = (scroll: ScrollClass) => {
    if (restore.waiting()) return;
    if (isCurrent && !isCurrent()) return;
    const next = currentPosition(scroll);
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  function subscribeToArm() {
    if (!followsLiveEdge) return null;
    return onFollowRideStarted(() => {
      if (isCurrent && !isCurrent()) return;
      stopRestore();
      observed = LIVE_EDGE_VALUE;
      scheduleSave();
    });
  }

  const unsubscribeArm = subscribeToArm();

  const allowRestore = openIsOurs();

  if (!allowRestore) {
    standDownForDeepLink();
  } else if (saved === null) {
    const seeded = followsLiveEdge && applyFollowSeed(el);
    reportRestore(seeded ? 'follow-seed' : 'top');
    if (!seeded && resetOnEmpty) writeRestore(0);
    restore.finish();
  } else if (saved.kind === 'live-edge') {
    resumeFollowingBottom(el);
    restore.finish();
    reportRestore('live-edge');
  } else {
    positionFromRecord();
  }

  if (restore.waiting()) {
    if (resetOnEmpty) writeRestore(0);
    resizeObserver = new ResizeObserver(() => tryRestore());
    resizeObserver.observe(el);
    mutationObserver = new MutationObserver(() => tryRestore());
    mutationObserver.observe(el, { childList: true, subtree: true });
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
    stopUserWatch = watchUserAction(() => {
      reportRestore('reader-took-over');
      stopRestore();
    });
  }

  const watchContent = () => {
    if (!anchorsToContent || typeof MutationObserver !== 'function') return;
    contentObserver = new MutationObserver(() => {
      if (contentTimer !== null) return;
      contentTimer = setTimeout(() => {
        contentTimer = null;
        recordContentPlace();
      }, CONTENT_READ_THROTTLE_MS);
    });
    contentObserver.observe(el, { childList: true, subtree: true, attributeFilter: [ANCHOR_ATTR] });
  };
  watchContent();

  const stopScroll = onClassifiedScroll(el, onScroll);


  const unsubscribeDeepLink = onDeepLinkClaimed(() => {
    if (!restore.waiting() && deadlineTimer === null) return;
    if (openIsOurs()) return;
    standDownForDeepLink();
  });

  const unsubscribeDeepLinkResolved = onDeepLinkResolved(() => {
    if (openIsOurs()) return;
    if (!restore.waiting() && deadlineTimer !== null) stopRestore();
    recordDeepLinkLanding();
    settleRestore();
  });

  const unsubscribeHide = onPageHide(flush);

  const unsubscribeWake = followsLiveEdge
    ? onPageWake(() => {
        if (isCurrent && !isCurrent()) return;
        replayRecordedFollow(readSaved(), openIsOurs() ? 'live-edge' : 'in-place');
      })
    : null;

  return () => {
    stopRestore(false);
    contentObserver?.disconnect();
    contentObserver = null;
    stopScroll();
    unsubscribeArm?.();
    unsubscribeDeepLink();
    unsubscribeDeepLinkResolved();
    unsubscribeHide();
    unsubscribeWake?.();
    flush();
  };
}

export function useScrollMemory(
  ref: RefObject<HTMLElement>,
  key: string | null,
  options: ScrollMemoryOptions = {},
) {
  const {
    paused = false,
    resetOnEmpty = false,
    followsLiveEdge = false,
    anchorsToContent = false,
  } = options;
  const liveRef = useRef<ScrollMemoryLive>(options);
  liveRef.current = options;
  const keyRef = useRef(key);
  keyRef.current = key;
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  useEffect(() => {
    if (!key || paused) return;
    const el = ref.current;
    if (!el) return;
    return attachScrollMemory(el, key, {
      live: () => liveRef.current,
      resetOnEmpty,
      followsLiveEdge,
      anchorsToContent,
      isCurrent: () => keyRef.current === key && !pausedRef.current,
    });
  }, [ref, key, paused, resetOnEmpty, followsLiveEdge, anchorsToContent]);
}
