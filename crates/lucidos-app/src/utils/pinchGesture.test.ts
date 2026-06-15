import { describe, it, expect } from 'vitest';
import {
  fingerDistance,
  fingerMidpoint,
  computePinchUpdate,
  clampPanTransform,
  computeZoomAt,
  naturalImageLayout,
  type PinchInitial,
} from './pinchGesture';

describe('fingerDistance', () => {
  it('returns Euclidean distance', () => {
    expect(fingerDistance(0, 0, 3, 4)).toBe(5);
    expect(fingerDistance(10, 10, 10, 10)).toBe(0);
  });
});

describe('fingerMidpoint', () => {
  it('returns the midpoint between two points', () => {
    expect(fingerMidpoint(0, 0, 100, 200)).toEqual({ x: 50, y: 100 });
    expect(fingerMidpoint(-50, 50, 50, -50)).toEqual({ x: 0, y: 0 });
  });
});

// Helper: simulate a complete pinch transform (compute + clamp) and assert
// the image-space point under the initial midpoint lands under the current
// midpoint. This is the "stick to fingers" invariant that defines a natural
// pinch.
function pointUnderMidpointAfter(
  initial: PinchInitial,
  curMidX: number,
  curMidY: number,
  curDist: number,
): { x: number; y: number } {
  const update = computePinchUpdate(initial, curMidX, curMidY, curDist, 0.001, 1000);
  // image-local point that started under the initial midpoint
  const localX = (initial.midX - initial.tx - initial.natCenterX) / initial.scale;
  const localY = (initial.midY - initial.ty - initial.natCenterY) / initial.scale;
  // where that point ends up after the new transform
  return {
    x: localX * update.scale + update.tx + initial.natCenterX,
    y: localY * update.scale + update.ty + initial.natCenterY,
  };
}

describe('computePinchUpdate', () => {
  const baseInitial: PinchInitial = {
    scale: 1,
    tx: 0,
    ty: 0,
    midX: 200,
    midY: 200,
    dist: 100,
    natCenterX: 200,
    natCenterY: 200,
  };

  it('scales by finger-distance ratio', () => {
    const u = computePinchUpdate(baseInitial, 200, 200, 200, 0.001, 1000);
    expect(u.scale).toBeCloseTo(2, 5);
  });

  it('clamps scale to maxScale', () => {
    const u = computePinchUpdate(baseInitial, 200, 200, 1000, 0.001, 5);
    expect(u.scale).toBe(5);
  });

  it('clamps scale to minScale', () => {
    const u = computePinchUpdate(baseInitial, 200, 200, 10, 1, 10);
    expect(u.scale).toBe(1);
  });

  it('returns initial when initial.dist is zero', () => {
    const init = { ...baseInitial, dist: 0, scale: 2, tx: 50, ty: 30 };
    const u = computePinchUpdate(init, 100, 100, 50, 0.001, 1000);
    expect(u).toEqual({ scale: 2, tx: 50, ty: 30 });
  });

  it('keeps the point under the initial midpoint glued to the current midpoint while zooming', () => {
    // Start: midpoint at (200,200) which is the natural center of the image.
    // Zoom 2x while moving midpoint to (260, 260). The pixel that was at
    // (200,200) before the gesture should now sit at (260,260).
    const result = pointUnderMidpointAfter(baseInitial, 260, 260, 200);
    expect(result.x).toBeCloseTo(260, 5);
    expect(result.y).toBeCloseTo(260, 5);
  });

  it('keeps the point under the initial midpoint glued during pure pan (constant distance)', () => {
    // Two fingers slide together, no scale change. Image must follow.
    const result = pointUnderMidpointAfter(baseInitial, 350, 280, 100);
    expect(result.x).toBeCloseTo(350, 5);
    expect(result.y).toBeCloseTo(280, 5);
  });

  it('handles off-center initial midpoint correctly', () => {
    const init: PinchInitial = {
      scale: 1.5,
      tx: 30,
      ty: -20,
      midX: 100,
      midY: 300,
      dist: 80,
      natCenterX: 200,
      natCenterY: 200,
    };
    const result = pointUnderMidpointAfter(init, 150, 280, 160);
    expect(result.x).toBeCloseTo(150, 5);
    expect(result.y).toBeCloseTo(280, 5);
  });

  it('produces no drift when called repeatedly with the same inputs', () => {
    // Recomputing from initial state (not previous frame) must be idempotent.
    const a = computePinchUpdate(baseInitial, 250, 240, 180, 1, 10);
    const b = computePinchUpdate(baseInitial, 250, 240, 180, 1, 10);
    expect(a).toEqual(b);
  });

  it('updates tx/ty for pure two-finger pan even when scale is clamped at min', () => {
    // Even when the scale ratio would be 0.5 but minScale is 1, the
    // translation still tracks the midpoint. Without this, a user trying to
    // shrink past 1x would see the image freeze instead of pan.
    // When clamped, the "stick to fingers" property only holds approximately
    // since the scale isn't honored, but tx/ty must still update so the user
    // sees motion.
    const init: PinchInitial = { ...baseInitial, scale: 1 };
    const u = computePinchUpdate(init, 300, 200, 50, 1, 10);
    expect(u.scale).toBe(1);
    // midpoint moved +100 in x; tx should reflect that
    expect(u.tx).toBeCloseTo(100, 5);
  });
});

describe('clampPanTransform', () => {
  it('forces tx/ty to 0 when scale <= 1', () => {
    expect(clampPanTransform(1, 50, 30, 800, 600, 800, 600)).toEqual({ tx: 0, ty: 0 });
    expect(clampPanTransform(0.5, 50, 30, 800, 600, 800, 600)).toEqual({ tx: 0, ty: 0 });
  });

  it('allows pan within overflow band when zoomed', () => {
    // 2x zoom on 800x600 image in 800x600 container.
    // X overflow = (800*2 - 800)/2 = 400, Y overflow = (600*2 - 600)/2 = 300.
    const u = clampPanTransform(2, 100, 50, 800, 600, 800, 600);
    expect(u).toEqual({ tx: 100, ty: 50 });
  });

  it('clamps pan to overflow band edges', () => {
    // X overflow = 400, Y overflow = 300. Inputs (500, -500) clamp to (400, -300).
    const u = clampPanTransform(2, 500, -500, 800, 600, 800, 600);
    expect(u).toEqual({ tx: 400, ty: -300 });
  });

  it('handles different overflow per axis', () => {
    // 1.5x zoom, image 800x600, container 1000x600.
    // X: imgW * scale = 1200 → overflow (1200-1000)/2 = 100.
    // Y: imgH * scale = 900 → overflow (900-600)/2 = 150.
    const u = clampPanTransform(1.5, 200, 200, 1000, 600, 800, 600);
    expect(u).toEqual({ tx: 100, ty: 150 });
  });
});

describe('naturalImageLayout', () => {
  it('returns the rect geometry unchanged when the image is not transformed', () => {
    const layout = naturalImageLayout(
      { width: 800, height: 600 },
      { left: 100, top: 50, width: 400, height: 300 },
      1, 0, 0,
    );
    expect(layout).toEqual({
      containerW: 800,
      containerH: 600,
      imgW: 400,
      imgH: 300,
      natCenterX: 300, // 100 + 400/2
      natCenterY: 200, // 50 + 300/2
    });
  });

  it('divides out an active scale so imgW/imgH are the natural (scale-1) size', () => {
    // getBoundingClientRect reports the *scaled* rect. A natural 400x300 image
    // rendered at scale 2 measures 800x600 — naturalImageLayout must recover 400x300.
    const layout = naturalImageLayout(
      { width: 800, height: 600 },
      { left: 0, top: 0, width: 800, height: 600 },
      2, 0, 0,
    );
    expect(layout.imgW).toBe(400);
    expect(layout.imgH).toBe(300);
  });

  it('subtracts the active translation so natCenter is the untransformed center', () => {
    // The natural center must be independent of the current pan. The rect
    // center is shifted by (tx, ty) = (120, -40); dividing it back out recovers
    // the real anchor point the pinch math expects.
    const layout = naturalImageLayout(
      { width: 800, height: 600 },
      { left: 220, top: 110, width: 400, height: 300 }, // center (420, 260)
      1, 120, -40,
    );
    expect(layout.natCenterX).toBe(300); // 420 - 120
    expect(layout.natCenterY).toBe(300); // 260 - (-40)
  });

  it('guards against a zero scale (treats it as 1)', () => {
    const layout = naturalImageLayout(
      { width: 800, height: 600 },
      { left: 0, top: 0, width: 400, height: 300 },
      0, 0, 0,
    );
    expect(layout.imgW).toBe(400);
    expect(layout.imgH).toBe(300);
  });

  // Regression: a gesture (second pinch, or one-finger pan) that starts while
  // the image is already zoomed must still clamp pan to the *real* image
  // bounds. Previously captureLayout read the live (scaled) rect, inflating
  // the clamp band so the image could be dragged outside its own edges.
  it('keeps pan bounded when a gesture starts on an already-zoomed image', () => {
    // Image fills an 800x600 slide; a prior gesture left it at scale 2, panned
    // tx=300. getBoundingClientRect therefore reports the scaled 1600x1200 rect:
    // center (400,300) -> +scale (unchanged) -> +translate (700,300);
    // left = 700 - 1600/2 = -100, top = 300 - 1200/2 = -300.
    const liveRect = { left: -100, top: -300, width: 1600, height: 1200 };
    const layout = naturalImageLayout({ width: 800, height: 600 }, liveRect, 2, 300, 0);
    expect(layout.imgW).toBe(800);
    expect(layout.imgH).toBe(600);
    expect(layout.natCenterX).toBe(400);
    expect(layout.natCenterY).toBe(300);

    // A far-out pan now clamps to the real overflow ((800*2-800)/2 = 400), not
    // the inflated bound a naive scaled capture (imgW=1600 -> overflow 1200)
    // would have allowed.
    const clamped = clampPanTransform(
      2, 1000, 0,
      layout.containerW, layout.containerH, layout.imgW, layout.imgH,
    );
    expect(clamped.tx).toBe(400);
  });

  // Regression: rotating the device while zoomed must re-clamp the existing
  // pan to the new viewport. The image still carries its old transform when the
  // resize fires, so naturalImageLayout has to recover the *new* natural bounds
  // (from the new slide size) before clamping pulls an out-of-bounds pan back in.
  it('re-clamps an out-of-bounds pan after the viewport changes (rotation)', () => {
    // Before: scale 2, tx=400 (the max for an 800-wide slide). Device rotates to
    // a 400-wide / 600-tall slide; the image (landscape, object-fit contain)
    // now renders 400x300 at scale 1. With the old transform still applied its
    // live rect is 800x600 centered at (200+400, 300) -> left 200, top 0.
    const liveRect = { left: 200, top: 0, width: 800, height: 600 };
    const layout = naturalImageLayout({ width: 400, height: 600 }, liveRect, 2, 400, 0);
    expect(layout.imgW).toBe(400);
    expect(layout.imgH).toBe(300);
    expect(layout.natCenterX).toBe(200); // new slide centre
    const clamped = clampPanTransform(
      2, 400, 0,
      layout.containerW, layout.containerH, layout.imgW, layout.imgH,
    );
    // New overflow is (400*2-400)/2 = 200, so tx=400 is pulled in to 200.
    expect(clamped).toEqual({ tx: 200, ty: 0 });
  });

  // Regression: unzooming a panned image with the fingers held over its centre
  // recenters cleanly instead of drifting sideways. The drift came from a
  // natCenter polluted by the prior pan (off by tx).
  it('recenters without drift when unzooming a panned image in place', () => {
    const liveRect = { left: -100, top: -300, width: 1600, height: 1200 };
    const layout = naturalImageLayout({ width: 800, height: 600 }, liveRect, 2, 300, 0);
    const initial: PinchInitial = {
      scale: 2, tx: 300, ty: 0,
      midX: layout.natCenterX, midY: layout.natCenterY, // fingers on the real centre
      dist: 200,
      natCenterX: layout.natCenterX, natCenterY: layout.natCenterY,
    };
    // Pinch the fingers to half distance (scale -> 1), midpoint unchanged.
    const u = computePinchUpdate(initial, layout.natCenterX, layout.natCenterY, 100, 1, 10);
    const c = clampPanTransform(
      u.scale, u.tx, u.ty,
      layout.containerW, layout.containerH, layout.imgW, layout.imgH,
    );
    expect(u.scale).toBeCloseTo(1, 5);
    expect(c).toEqual({ tx: 0, ty: 0 });
  });
});

describe('computeZoomAt', () => {
  it('keeps the point under the anchor fixed after a scale change', () => {
    // Zoom from 1x to 2x with anchor at (300, 300), nat center at (200, 200).
    const u = computeZoomAt(1, 0, 0, 300, 300, 2, 200, 200, 0.1, 10);
    // The point at (300,300) in screen coords corresponds to image-local
    // (100,100). After 2x zoom that point should still be at (300,300).
    const localX = (300 - 0 - 200) / 1; // 100
    const localY = (300 - 0 - 200) / 1; // 100
    const newScreenX = localX * u.scale + u.tx + 200;
    const newScreenY = localY * u.scale + u.ty + 200;
    expect(newScreenX).toBeCloseTo(300, 5);
    expect(newScreenY).toBeCloseTo(300, 5);
  });

  it('clamps scale to bounds', () => {
    const a = computeZoomAt(1, 0, 0, 0, 0, 100, 0, 0, 0.1, 10);
    expect(a.scale).toBe(10);
    const b = computeZoomAt(1, 0, 0, 0, 0, 0.01, 0, 0, 0.1, 10);
    expect(b.scale).toBe(0.1);
  });
});
