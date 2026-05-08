import type { VNode } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import {
  activeExchanges,
  activeStreamingBuffer,
  threadsLoaded,
  artifacts,
  mobileView,
  focusedThreadId,
  threadMap,
} from '../../store/store';
import { scrolledUp, awayFromBottom, notAtTop, setActiveScrollElement, getActiveScrollElement, isElementVisible, makeScrollObservers } from './scrollState';
import { ChatExchange } from './ChatExchange';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { WelcomeMessage } from './WelcomeMessage';
import type { Exchange } from '../../store/thread-events';
import { exchangeStatus as getExchangeStatus, exchangeImageCount, exchangeResponseModel, exchangeReasoningEffort, unresumedAbortIndex } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';

/** Threads imageOffset + last model/effort across exchanges so each child sees its predecessors' state. */
export function renderExchanges(
  exchanges: Exchange[],
  threadId: string,
  streamingBuffer: string,
): VNode[] {
  // Compute once which abort exchange (if any) gets the Continue button —
  // the most recent ResponseAborted that has no later SessionRecovered.
  const unresumedIdx = unresumedAbortIndex(exchanges);
  const threadMeta = threadMap.value.get(threadId)?.meta;
  const threadIsCC = threadMeta?.channel === 'claude_code';
  const threadIdle = threadMeta?.status === 'idle';
  return exchanges.reduce<{ nodes: VNode[]; imgOffset: number; lastModel?: string; lastEffort?: string }>((acc, ex, i) => {
    const isLast = i === exchanges.length - 1;
    // Pass isLast=false for the prior exchange (it's at i-1, never the last
    // here): a mid-flight chat parent must read as 'streaming' so this
    // exchange's gate flips to 'queued', not slip through to 'done'.
    const priorActive = i > 0 && isStatusActive(getExchangeStatus(exchanges[i - 1], '', /* isLast */ false, /* hasPriorActive */ false, threadIsCC, threadIdle));
    acc.nodes.push(
      <ChatExchange
        key={'ex-' + ex.userSeq}
        exchange={ex}
        streamingBuffer={isLast ? streamingBuffer : ''}
        isLast={isLast}
        threadId={threadId}
        hasPriorActive={priorActive}
        imageOffset={acc.imgOffset}
        priorModel={acc.lastModel}
        priorEffort={acc.lastEffort}
        isUnresumedAbort={i === unresumedIdx}
      />
    );
    acc.imgOffset += exchangeImageCount(ex);
    acc.lastModel = exchangeResponseModel(ex) ?? acc.lastModel;
    acc.lastEffort = exchangeReasoningEffort(ex) ?? acc.lastEffort;
    return acc;
  }, { nodes: [], imgOffset: 0 }).nodes;
}

// --- Scroll anchoring for toggle changes (More/Less, Show/Hide Steps) ---
//
// When toggling global signals (stepsExpanded, detailsExpanded), ALL ChatExchange
// components re-render, changing content height. Without compensation, the scroll
// position stays at the same absolute offset but the viewport shows different
// content — the classic "scroll jump."
//
// iOS Safari PWA: WKWebView's compositor adjusts scroll position asynchronously
// when DOM nodes change, and `overflow-anchor` is unsupported (WebKit #171099).
// We freeze the container (`overflow:hidden`) during DOM changes so the browser
// can't touch scrollTop, then compensate and unfreeze.
//
// IMPORTANT: There are two .thread-content elements (desktop SplitLayout + mobile
// MobileSwipeContainer). We find the correct one via `anchor.closest()`, not by ID.

/** Keep `anchor` visually pinned while `fn` mutates the DOM. */
export function withScrollAnchor(anchor: Element | null | undefined, fn: () => void) {
  const container = anchor?.closest('.thread-content') as HTMLElement | null;
  if (!container || !anchor) { fn(); return; }

  // Blur focused element inside container — iOS auto-scrolls to keep focus visible.
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && container.contains(focused)) {
    focused.blur();
  }

  const el = anchor as HTMLElement;
  const offsetBefore = el.offsetTop;
  const scrollBefore = container.scrollTop;
  const overflowBefore = container.style.overflow;
  let restored = false;

  // Freeze: prevent browser from adjusting scroll during DOM changes.
  container.style.overflow = 'hidden';

  const restore = () => {
    if (restored) return;
    restored = true;
    observer.disconnect();

    const delta = el.offsetTop - offsetBefore;
    container.scrollTop = scrollBefore + delta;
    container.style.overflow = overflowBefore;

    // iOS may adjust after unfreeze — re-check in next frame.
    if (delta !== 0) {
      requestAnimationFrame(() => {
        if (!el.isConnected) return;
        const target = scrollBefore + (el.offsetTop - offsetBefore);
        if (Math.abs(container.scrollTop - target) > 1) {
          container.scrollTop = target;
        }
      });
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  // Synchronous check (if DOM changed synchronously)
  if (!restored && el.offsetTop !== offsetBefore) restore();

  // After Preact's Promise microtask render
  queueMicrotask(() => queueMicrotask(restore));

  // Final safety net — ensure overflow is always restored
  requestAnimationFrame(restore);
}

/** Auto-scroll to bottom when new exchanges arrive or streaming updates happen.
 *
 *  Listener setup tracks the actual DOM element via a ref, not just the `ready`
 *  boolean. If the element changes (e.g. component unmount/remount during SSE
 *  reconnection), listeners are detached from the old element and reattached to
 *  the new one on the next render. This prevents "dead listener" bugs where
 *  scroll events go to a detached DOM node. */
export function useAutoScroll(ref: preact.RefObject<HTMLDivElement>, deps: unknown[], ready: boolean) {
  const listenerRef = useRef<{ el: HTMLDivElement; cleanup: () => void } | null>(null);

  // Check on every render whether the target element has changed.
  // Runs after commit (ref.current is set), so we always see the current element.
  useEffect(() => {
    const el = ready ? ref.current : null;
    const prev = listenerRef.current;

    // Same element (or both null) — nothing to do
    if (el === (prev?.el ?? null)) return;

    // Cleanup old listeners
    prev?.cleanup();
    listenerRef.current = null;

    if (!el) return;

    const { onScroll, onResize } = makeScrollObservers(el);

    el.addEventListener('scroll', onScroll, { passive: true });
    const ro = new ResizeObserver(onResize);
    ro.observe(el);
    // The container is position:absolute inset:0 (chat.css), so its own box
    // never resizes when children grow — observing it alone misses in-thread
    // size changes (panel header expand/collapse). Observe each child too,
    // and re-observe on childList changes so new exchanges join in.
    function observeChildren() {
      for (const child of Array.from(el!.children)) {
        ro.observe(child);
      }
    }
    const mo = new MutationObserver(observeChildren);
    mo.observe(el, { childList: true });
    observeChildren();

    // Register as the active scroll target so scrollToBottom() targets
    // the correct element (not a hidden desktop duplicate on mobile).
    if (isElementVisible(el)) {
      setActiveScrollElement(el);
    }

    listenerRef.current = {
      el,
      cleanup: () => {
        el.removeEventListener('scroll', onScroll);
        ro.disconnect();
        mo.disconnect();
        // Only clear if we're still the active element (another instance
        // may have already registered itself during component transitions).
        if (getActiveScrollElement() === el) {
          setActiveScrollElement(null);
        }
      },
    };
  });

  // Cleanup on unmount
  useEffect(() => () => {
    listenerRef.current?.cleanup();
    listenerRef.current = null;
    notAtTop.value = false;
    awayFromBottom.value = false;
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el || scrolledUp.value) return;
    el.scrollTop = el.scrollHeight;
  }, deps);
}

export function CreateThreadView() {
  const exchanges = activeExchanges.value;
  const streamingBuffer = activeStreamingBuffer.value;
  const threadId = focusedThreadId.value || '';
  const loaded = threadsLoaded.value;
  const areaRef = useRef<HTMLDivElement>(null);
  const isUp = awayFromBottom.value;
  const isNotAtTop = notAtTop.value;

  const artLoadable = artifacts.value;
  const hasArtifacts = artLoadable.status === 'loaded' && artLoadable.data.length > 0;
  const isEmpty = exchanges.length === 0;
  // Show welcome when there are no exchanges and no loaded artifacts.
  // Don't gate on artifact loading status — that caused the scroll container
  // to be removed from the DOM while artifacts loaded, showing a blank screen.
  const showWelcome = isEmpty && !hasArtifacts;

  // hasContent: true exactly when the thread-content div will be in the DOM.
  // useAutoScroll depends on this — if we only pass `loaded`, the effect runs
  // when threadsLoaded becomes true but the div might not exist yet (no
  // exchanges, no welcome). Later when exchanges arrive, the effect never
  // re-runs → no scroll listener → button broken.
  const hasContent = loaded && (showWelcome || !isEmpty);

  // Auto-scroll to bottom when exchanges change or mobile view changes
  useAutoScroll(areaRef, [exchanges, streamingBuffer, mobileView.value], hasContent);

  return (
    <div class="thread-content-wrap">
      {hasContent && (
        <>
          <div class="thread-content visible" ref={areaRef}>

            {showWelcome ? (
              <WelcomeMessage />
            ) : (
              renderExchanges(exchanges, threadId, streamingBuffer)
            )}
          </div>
          {!isEmpty && (
            <ScrollControls
              showUp={isNotAtTop}
              showDown={isUp}
              onScrollUp={() => areaRef.current?.scrollTo({ top: 0, behavior: 'smooth' })}
              onScrollDown={() => {
                const el = areaRef.current;
                if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
              }}
            />
          )}
        </>
      )}
    </div>
  );
}

// Buttons stay mounted so the CSS `.visible` class can drive the fade transition.
export function ScrollControls({ showUp, showDown, onScrollUp, onScrollDown }: {
  showUp: boolean;
  showDown: boolean;
  onScrollUp: () => void;
  onScrollDown: () => void;
}) {
  return (
    <>
      <button class={`scroll-to-top${showUp ? ' visible' : ''}`} onClick={onScrollUp}>
        <ChevronUpIcon />
      </button>
      <button class={`scroll-to-bottom${showDown ? ' visible' : ''}`} onClick={onScrollDown}>
        <ChevronDownIcon />
      </button>
    </>
  );
}
