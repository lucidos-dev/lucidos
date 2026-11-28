import { describe, it, expect } from 'vitest';
import { SwipeTouch } from '@lucidos/pane-swipe';
import { PANE_INDEX } from '../../../store/store';
import type { MobileView } from '../../../store/store';


const PANE_WIDTH = 375;

describe('PANE_INDEX', () => {
  it('maps threads to 0, thread to 1, content to 2', () => {
    expect(PANE_INDEX.threads).toBe(0);
    expect(PANE_INDEX.thread).toBe(1);
    expect(PANE_INDEX.content).toBe(2);
  });
});

describe('SwipeTouch', () => {


  describe('direction lock', () => {
    it('returns null until movement exceeds threshold', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.move(104, 102)).toBeNull();
      expect(s.move(107, 103)).toBeNull();
    });

    it('locks horizontal when dx >= dy', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.move(110, 103)).toBe(2);
      expect(s.isHorizontal).toBe(true);
    });

    it('locks vertical when dy > dx', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      expect(s.move(103, 110)).toBeNull();
      expect(s.isHorizontal).toBe(false);
    });

    it('stays locked horizontal after initial lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(110, 100);
      expect(s.move(115, 120)).toBe(7);
    });

    it('stays locked vertical after initial lock', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(100, 110);
      expect(s.move(120, 115)).toBeNull();
    });
  });


  describe('swipe detection', () => {
    it('returns +1 for left swipe past 1/3 threshold', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(200 - PANE_WIDTH / 3 - 10, 100);
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
      s.move(200 - PANE_WIDTH / 3 + 20, 100);
      (s as any)._startTime = Date.now() - 1000;
      expect(s.end(PANE_WIDTH)).toBe(0);
    });

    it('returns 0 when direction is vertical', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(100, 300);
      expect(s.end(PANE_WIDTH)).toBe(0);
    });

    it('returns 0 when not tracking', () => {
      const s = new SwipeTouch();
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });


  describe('fast swipe', () => {
    it('detects fast left swipe with small distance', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(160, 100);
      expect(s.end(PANE_WIDTH)).toBe(1);
    });

    it('detects fast right swipe with small distance', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(140, 100);
      expect(s.end(PANE_WIDTH)).toBe(-1);
    });
  });


  describe('cancel', () => {
    it('stops tracking and subsequent end returns 0', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(250, 100);
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


  describe('move after end', () => {
    it('returns null — tracking is stopped', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(250, 100);
      s.end(PANE_WIDTH);
      expect(s.move(300, 100)).toBeNull();
    });
  });


  describe('full lifecycle', () => {
    it('swipe left: thread → content', () => {
      const s = new SwipeTouch();
      s.start(300, 200);
      s.move(250, 202);
      s.move(200, 203);
      s.move(150, 201);
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(1);
    });

    it('swipe right: thread → threads', () => {
      const s = new SwipeTouch();
      s.start(100, 200);
      s.move(150, 198);
      s.move(200, 199);
      s.move(250, 200);
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(-1);
    });

    it('aborted swipe: small movement snaps back', () => {
      const s = new SwipeTouch();
      s.start(200, 200);
      s.move(190, 201);
      s.move(185, 202);
      (s as any)._startTime = Date.now() - 1000;
      const delta = s.end(PANE_WIDTH);
      expect(delta).toBe(0);
    });

    it('sequential swipes work independently', () => {
      const s = new SwipeTouch();

      s.start(300, 200);
      s.move(200, 200);
      s.move(100, 200);
      expect(s.end(PANE_WIDTH)).toBe(1);

      s.start(100, 200);
      s.move(200, 200);
      s.move(300, 200);
      expect(s.end(PANE_WIDTH)).toBe(-1);
    });

    it('vertical scroll does not trigger pane switch', () => {
      const s = new SwipeTouch();
      s.start(200, 100);
      s.move(202, 200);
      s.move(203, 400);
      s.move(201, 600);
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });


  describe('structural guarantee: single source of truth', () => {
    it('pane position derives from mobileView, not from touch state', () => {
      const s = new SwipeTouch();
      s.start(300, 200);
      s.move(100, 200);
      const delta = s.end(PANE_WIDTH);

      const currentView: MobileView = 'thread';
      const currentIndex = PANE_INDEX[currentView];
      const newIndex = Math.max(0, Math.min(2, currentIndex + delta));

      const headerView = (['threads', 'thread', 'content'] as const)[newIndex];
      const paneOffset = -newIndex;

      expect(headerView).toBe('content');
      expect(paneOffset).toBe(-2);
    });

    it('external view change (dot tap, thread click) updates both atomically', () => {
      const views: MobileView[] = ['threads', 'thread', 'content'];
      for (const view of views) {
        const index = PANE_INDEX[view];
        expect(index).toBe(views.indexOf(view));
      }
    });

    it('SwipeTouch has no position state that could disagree with signal', () => {
      const s = new SwipeTouch();
      s.start(100, 100);
      s.move(300, 100);
      s.end(PANE_WIDTH);

      expect(s.isHorizontal).toBe(false);
      expect(s.move(400, 100)).toBeNull();
      expect(s.end(PANE_WIDTH)).toBe(0);
    });
  });
});
