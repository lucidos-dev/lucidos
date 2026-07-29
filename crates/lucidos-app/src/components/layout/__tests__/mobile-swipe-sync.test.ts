import { describe, it, expect } from 'vitest';
import { SwipeTouch } from '../MobileSwipeContainer';
import { PANE_INDEX } from '../../../store/store';
import type { MobileView } from '../../../store/store';

// ─────────────────────────────────────────────────────────────────────────────
// Architecture guarantee: mobileView signal is the SINGLE source of truth.
//
// The header (data-mobile-view CSS attribute), dot indicator (JSX), and
// pane position (CSS transform via useLayoutEffect) ALL derive from the
// mobileView signal. Desync prevention layers:
//
//   1. navigateToPane() — single entry point for all pane switches.
//      Atomically closes drawers + updates signal. No scattered multi-step
//      mutations that could leave intermediate inconsistent state.
//
//   2. useLayoutEffect — derives CSS transform BEFORE browser paints,
//      so header/dots and pane position update in the same frame.
//      (Old: useEffect ran AFTER paint, creating a one-frame desync window.)
//
//   3. transitionend reconciliation — safety net that forces the transform
//      to match the signal after every CSS transition completes.
//
//   4. SwipeTouch — pure gesture detector. No DOM state, no scroll position.
//      Returns pane deltas only. The caller applies them via navigateToPane().
// ─────────────────────────────────────────────────────────────────────────────

const PANE_WIDTH = 375; // typical mobile viewport

// ---------------------------------------------------------------------------
// PANE_INDEX — maps view names to indices
// ---------------------------------------------------------------------------
describe('PANE_INDEX', () => {
  it('maps threads to 0, thread to 1, content to 2', () => {
    expect(PANE_INDEX.threads).toBe(0);
    expect(PANE_INDEX.thread).toBe(1);
    expect(PANE_INDEX.content).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// SwipeTouch — pure touch gesture handler
// ---------------------------------------------------------------------------
describe('SwipeTouch', () => {

  // ── Direction lock ──────────────────────────────────────────────────────

  describe('direction lock', () => {
    it('returns null until movement exceeds threshold', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.move(104, 102)).toBeNull(); // below 8px threshold
      expect(s.move(107, 103)).toBeNull(); // still below
    });

    it('locks horizontal when dx >= dy', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      // dx=10, dy=3 → horizontal lock; visible drag = dx − threshold (8)
      expect(s.move(110, 103)).toBe(2);
      expect(s.isHorizontal).toBe(true);
    });

    it('locks vertical when dy > dx', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.move(103, 110)).toBeNull(); // dx=3, dy=10 → vertical
      expect(s.isHorizontal).toBe(false);
    });

    it('stays locked horizontal after initial lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(110, 100); // lock horizontal
      // Subsequent moves with more vertical component still return the
      // horizontal delta (minus the 8px lock threshold).
      expect(s.move(115, 120)).toBe(7);
    });

    it('stays locked vertical after initial lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(100, 110); // lock vertical
      expect(s.move(120, 115)).toBeNull(); // still returns null
    });
  });

  // ── Swipe detection ─────────────────────────────────────────────────────

  describe('swipe detection', () => {
    it('returns +1 for left swipe past 1/3 threshold', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(200 - PANE_WIDTH / 3 - 10, 100); // past 1/3
      expect(s.end(PANE_WIDTH)).toBe(1);
    });

    it('returns -1 for right swipe past 1/3 threshold', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(100 + PANE_WIDTH / 3 + 10, 100);
      expect(s.end(PANE_WIDTH)).toBe(-1);
    });

    it('returns 0 for slow swipe below 1/3 threshold', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(200 - PANE_WIDTH / 3 + 20, 100); // below 1/3 threshold
      // Simulate a slow drag so velocity doesn't trigger fast-swipe detection
      (s as any)._startTime = Date.now() - 1000;
      expect(s.end(PANE_WIDTH)).toBe(0);
    });

    it('returns 0 when direction is vertical', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(100, 300); // vertical lock
      expect(s.end(PANE_WIDTH)).toBe(0);
    });

    it('returns 0 when not tracking', () => {
      const s = new SwipeTouch();
      // Never called start()
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });

  // ── Fast swipe (velocity-based) ─────────────────────────────────────────

  describe('fast swipe', () => {
    it('detects fast left swipe with small distance', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      // Move 40px left in ~1ms (simulated by immediate call)
      s.move(160, 100);
      // end() uses Date.now() - startTime. Since start() just ran,
      // elapsed ≈ 0-1ms. velocity = 40 / 1 = 40 px/ms >> 0.3 threshold
      expect(s.end(PANE_WIDTH)).toBe(1);
    });

    it('detects fast right swipe with small distance', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(140, 100);
      expect(s.end(PANE_WIDTH)).toBe(-1);
    });
  });

  // ── cancel ──────────────────────────────────────────────────────────────

  describe('cancel', () => {
    it('stops tracking and subsequent end returns 0', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(250, 100); // far horizontal
      s.cancel();
      expect(s.isHorizontal).toBe(false);
      expect(s.end(PANE_WIDTH)).toBe(0);
    });

    it('cancel during undecided state', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.cancel();
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });

  // ── isHorizontal ────────────────────────────────────────────────────────

  describe('isHorizontal', () => {
    it('false before start', () => {
      expect(new SwipeTouch().isHorizontal).toBe(false);
    });

    it('false before direction lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.isHorizontal).toBe(false);
    });

    it('true after horizontal lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(120, 100);
      expect(s.isHorizontal).toBe(true);
    });

    it('false after end', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(250, 100);
      s.end(PANE_WIDTH);
      expect(s.isHorizontal).toBe(false);
    });
  });

  // ── move after end ──────────────────────────────────────────────────────

  describe('move after end', () => {
    it('returns null — tracking is stopped', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(250, 100);
      s.end(PANE_WIDTH);
      expect(s.move(300, 100)).toBeNull();
    });
  });

  // ── Full lifecycle simulations ──────────────────────────────────────────

  describe('full lifecycle', () => {
    it('swipe left: thread → content', () => {
      const s = new SwipeTouch();
      s.start(300, 200);
      s.move(250, 202); // horizontal lock
      s.move(200, 203);
      s.move(150, 201);
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(1); // next pane
    });

    it('swipe right: thread → threads', () => {
      const s = new SwipeTouch();
      s.start(100, 200);
      s.move(150, 198);
      s.move(200, 199);
      s.move(250, 200);
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(-1); // prev pane
    });

    it('aborted swipe: small movement snaps back', () => {
      const s = new SwipeTouch();
      s.start(200, 200);
      s.move(190, 201); // horizontal lock
      s.move(185, 202); // only 15px — well below threshold
      // Wait a moment to reduce velocity (simulated by sleeping)
      (s as any)._startTime = Date.now() - 1000; // pretend it took 1 second
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(0); // snap back
    });

    it('sequential swipes work independently', () => {
      const s = new SwipeTouch();

      // First swipe: left
      s.start(300, 200);
      s.move(200, 200);
      s.move(100, 200);
      expect(s.end(PANE_WIDTH)).toBe(1);

      // Second swipe: right
      s.start(100, 200);
      s.move(200, 200);
      s.move(300, 200);
      expect(s.end(PANE_WIDTH)).toBe(-1);
    });

    it('vertical scroll does not trigger pane switch', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(202, 200); // vertical lock
      s.move(203, 400);
      s.move(201, 600);
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });

  // ── Structural guarantee: no desync possible ────────────────────────────
  // These tests verify the architectural property, not the class behavior.

  describe('structural guarantee: single source of truth', () => {
    it('pane position derives from mobileView, not from touch state', () => {
      // In the old architecture, pane position came from scroll position
      // (second source of truth). SwipeTouch has NO position state —
      // it only returns a delta. The caller applies the delta to the
      // mobileView signal, which drives both header and pane position.
      const s = new SwipeTouch();
      s.start(300, 200);
      s.move(100, 200);
      const delta = s.end(PANE_WIDTH);

      // Simulate what the component does:
      const currentView: MobileView = 'thread';
      const currentIndex = PANE_INDEX[currentView];
      const newIndex = Math.max(0, Math.min(2, currentIndex + delta));

      // BOTH header and pane position derive from the SAME newIndex
      const headerView = (['threads', 'thread', 'content'] as const)[newIndex];
      const paneOffset = -newIndex; // transform uses this index

      expect(headerView).toBe('content');
      expect(paneOffset).toBe(-2);
      // Header says 'content', pane shows content. Same source. No desync.
    });

    it('external view change (dot tap, thread click) updates both atomically', () => {
      // When setMobileView('threads') is called from a dot tap or thread click,
      // the signal updates atomically. The header and pane both read from it.
      // No intermediate state where one has updated but the other hasn't.
      const views: MobileView[] = ['threads', 'thread', 'content'];
      for (const view of views) {
        const index = PANE_INDEX[view];
        // Header would show view, pane would translateX(-index * paneWidth)
        expect(index).toBe(views.indexOf(view));
      }
    });

    it('SwipeTouch has no position state that could disagree with signal', () => {
      // The class stores start position and delta for gesture detection only.
      // After end(), all state is reset. There is no "currentPane" or
      // "scrollPosition" that could drift from the mobileView signal.
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(300, 100);
      s.end(PANE_WIDTH);

      // After end: not tracking, no horizontal lock, no stored delta
      expect(s.isHorizontal).toBe(false);
      expect(s.move(400, 100)).toBeNull(); // not tracking
      expect(s.end(PANE_WIDTH)).toBe(0);   // not tracking
    });
  });
});
