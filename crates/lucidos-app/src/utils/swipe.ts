// Direction lock threshold (px) — below this, direction is undecided
const LOCK_THRESHOLD = 8;
// Minimum distance for a fast swipe (px)
const MIN_SWIPE_DISTANCE = 30;
// Minimum velocity for a fast swipe (px/ms)
const FAST_SWIPE_VELOCITY = 0.3;

/** Pure touch gesture handler for horizontal pane swiping.
 *  No DOM dependencies — fully testable. */
export class SwipeTouch {
  private _startX = 0;
  private _startY = 0;
  private _startTime = 0;
  private _tracking = false;
  private _direction: 'horizontal' | 'vertical' | null = null;
  private _dx = 0;
  /** Which way the finger was going when the direction locked. Frozen there,
   *  because the threshold compensation below is one fixed offset for the whole
   *  gesture. Re-reading the sign per frame flips the offset the moment the
   *  finger drags back past its start, which reports +7 for a 1px move LEFT. */
  private _lockSign = 0;

  /** Record start of a new touch. */
  start(x: number, y: number): void {
    this._startX = x;
    this._startY = y;
    this._startTime = Date.now();
    this._tracking = true;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }

  /** Process a touch move. Returns the horizontal delta (px) if tracking
   *  horizontally, or null if vertical/undecided/not tracking. */
  move(x: number, y: number): number | null {
    if (!this._tracking) return null;

    const dx = x - this._startX;
    const dy = y - this._startY;

    if (this._direction === null) {
      if (Math.abs(dx) < LOCK_THRESHOLD && Math.abs(dy) < LOCK_THRESHOLD) return null;
      this._direction = Math.abs(dx) >= Math.abs(dy) ? 'horizontal' : 'vertical';
      // A horizontal lock needs `|dx| >= |dy|` and one of them past the
      // threshold, so `|dx| >= LOCK_THRESHOLD` here and the sign is decided.
      this._lockSign = Math.sign(dx);
    }

    if (this._direction === 'vertical') return null;

    // Subtract the lock threshold so visible drag starts from 0 instead of
    // jumping ±LOCK_THRESHOLD on the first frame after lock.
    const adjusted = dx - this._lockSign * LOCK_THRESHOLD;
    this._dx = adjusted;
    return adjusted;
  }

  /** End the touch. Returns the pane delta: -1 (prev), 0 (snap back), +1 (next).
   *  Only non-zero when the gesture was a confirmed horizontal swipe. */
  end(paneWidth: number): number {
    if (!this._tracking || this._direction !== 'horizontal') {
      this._tracking = false;
      return 0;
    }

    this._tracking = false;
    const elapsed = Math.max(1, Date.now() - this._startTime);
    const velocity = Math.abs(this._dx) / elapsed;

    const fastSwipe = velocity > FAST_SWIPE_VELOCITY && Math.abs(this._dx) > MIN_SWIPE_DISTANCE;
    const farDrag = Math.abs(this._dx) > paneWidth / 3;

    if (fastSwipe || farDrag) {
      return this._dx > 0 ? -1 : 1;
    }

    return 0;
  }

  /** Whether a horizontal swipe is currently being tracked. */
  get isHorizontal(): boolean {
    return this._tracking && this._direction === 'horizontal';
  }

  /** Cancel tracking without producing a result. */
  cancel(): void {
    this._tracking = false;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }
}
