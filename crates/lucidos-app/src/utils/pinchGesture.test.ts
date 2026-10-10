import { describe, it, expect } from 'vitest';
import {
  fingerDistance,
  fingerMidpoint,
  computePinchUpdate,
  clampPanTransform,
  computeZoomAt,
  fitToWindowScale,
  fullSizeScale,
  naturalImageLayout,
  sameScale,
  steppedScale,
  zoomCeiling,
  zoomFloor,
  zoomPercent,
  ZOOM_STEP,
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

describe('steppedScale', () => {
  it('multiplies and divides by one step', () => {
    expect(steppedScale(1, 1, 1, 10)).toBeCloseTo(ZOOM_STEP, 5);
    expect(steppedScale(ZOOM_STEP, -1, 1, 10)).toBeCloseTo(1, 5);
  });

  it('stops at the range ends', () => {
    expect(steppedScale(1, -1, 1, 10)).toBe(1);
    expect(steppedScale(9, 1, 1, 10)).toBe(10);
  });
});

describe('fullSizeScale', () => {
  it('is the ratio of natural width to the width in screen pixels', () => {
    expect(fullSizeScale(300, 1200, 1)).toBe(4);
    expect(fullSizeScale(1200, 1200, 1)).toBe(1);
  });

  // The bug this answers: the popup measured CSS pixels. A phone screenshot
  // filling the phone it came from read 33%, while every one of its pixels sat
  // on a screen pixel. "Actual size" from there was a threefold blow-up.
  it('calls a screenshot of this screen, filling this screen, full size', () => {
    // 1179 device pixels wide, laid out across the phone's 393 CSS pixels.
    expect(fullSizeScale(393, 1179, 3)).toBe(1);
  });

  it('halves the target on a screen drawing two pixels per CSS pixel', () => {
    expect(fullSizeScale(300, 1200, 2)).toBe(2);
  });

  it('answers 0 when there is nothing to measure', () => {
    expect(fullSizeScale(0, 1200, 1)).toBe(0);
    expect(fullSizeScale(300, 0, 1)).toBe(0);
    expect(fullSizeScale(300, 1200, 0)).toBe(0);
  });
});

describe('zoomCeiling', () => {
  it('raises the cap so full size stays reachable', () => {
    expect(zoomCeiling(10, 22)).toBe(22);
  });

  it('keeps the cap when full size is already under it', () => {
    expect(zoomCeiling(10, 4)).toBe(10);
    // An image with no intrinsic width reports 0; the plain cap stands.
    expect(zoomCeiling(10, 0)).toBe(10);
  });
});

describe('zoomFloor', () => {
  it('rests on the fitted view for an image the window has to shrink', () => {
    // Full size is above the fit, so the fit is as far out as zooming goes.
    expect(zoomFloor(1, 3.02)).toBe(1);
  });

  it('drops below the fit so a blown-up image can reach its own pixels', () => {
    // 100x80 fitted 9.5x into a desktop window, at 1:1 on a 2x screen.
    expect(zoomFloor(9.5, 0.5)).toBe(0.5);
  });

  it('rests on the fitted view when there is no full size to aim at', () => {
    // An SVG that declares no intrinsic width reports 0.
    expect(zoomFloor(2.5, 0)).toBe(2.5);
  });

  it('rests on the fitted view when the two coincide', () => {
    expect(zoomFloor(1, 1)).toBe(1);
  });
});

describe('fitToWindowScale', () => {
  // The bug this answers: a small image sat at its own size in a big window,
  // and the control called that a fit.
  it('grows a small image out to the nearer window edge', () => {
    // 300x200 in an 1800x900 window: width would allow 6x, height only 4.5x.
    expect(fitToWindowScale(1800, 900, 300, 200)).toBeCloseTo(4.5, 5);
  });

  it('leaves an image CSS already contained at 1', () => {
    // An oversized screenshot lays out at the container width, so it fits now.
    expect(fitToWindowScale(1800, 900, 1800, 600)).toBe(1);
    expect(fitToWindowScale(1800, 900, 1200, 900)).toBe(1);
  });

  it('does not chase a sub-pixel gap left by the measured box', () => {
    // A contained image measures a hair short of its box. Asking for 1.0001x
    // resamples the whole layer to close a gap nobody can see.
    expect(fitToWindowScale(1800, 760, 1200, 759.9)).toBe(1);
  });

  it('answers 1 when there is nothing to measure', () => {
    expect(fitToWindowScale(0, 900, 300, 200)).toBe(1);
    expect(fitToWindowScale(1800, 900, 0, 0)).toBe(1);
  });
});

describe('zoomPercent', () => {
  it('reads 100% at one image pixel per screen pixel', () => {
    // A screenshot laid out at 40% of its own width: full size is scale 2.5.
    expect(zoomPercent(2.5, 2.5, 1)).toBe(100);
  });

  // The reported bug, end to end through the two functions that produce it: a
  // 1179-wide screenshot fitted across the 393 CSS pixels of the phone it was
  // taken on. It read 33%. Nothing about the image had to change.
  it('reads a phone screenshot fitted to its own phone as 100%', () => {
    const full = fullSizeScale(393, 1179, 3);
    expect(zoomPercent(1, full, 1)).toBe(100);
  });

  it('reads the fitted view of an oversized image well under 100%', () => {
    expect(zoomPercent(1, 2.5, 1)).toBe(40);
  });

  it('reads the fitted view of a small image well over 100%', () => {
    expect(zoomPercent(4.5, 1, 4.5)).toBe(450);
  });

  it('falls back to the fitted view for an image with no intrinsic size', () => {
    expect(zoomPercent(2, 0, 2)).toBe(100);
    expect(zoomPercent(3, 0, 2)).toBe(150);
  });

  it('rounds to whole percent, so the readout never jitters on a decimal', () => {
    expect(zoomPercent(1.234, 1, 1)).toBe(123);
  });
});

describe('sameScale', () => {
  it('accepts a fit recomputed from measured pixels', () => {
    expect(sameScale(4.5, 4.500_001)).toBe(true);
  });

  it('holds its tolerance relative, at 1x and at 45x alike', () => {
    expect(sameScale(1, 1.05)).toBe(false);
    expect(sameScale(45, 45.01)).toBe(true);
    expect(sameScale(45, 47)).toBe(false);
  });
});
