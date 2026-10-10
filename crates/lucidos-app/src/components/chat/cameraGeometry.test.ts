import { describe, it, expect } from 'vitest';
import { computeCaptureGeometry } from './cameraGeometry';

// See cameraGeometry.ts for why this rotation compensation is needed.

describe('computeCaptureGeometry', () => {
  it('portrait device + portrait frame: no rotation (iOS portrait, all platforms)', () => {
    const g = computeCaptureGeometry(1080, 1920, 0);
    expect(g.canvasWidth).toBe(1080);
    expect(g.canvasHeight).toBe(1920);
    expect(g.rotateRadians).toBe(0);
  });

  it('landscape device + portrait frame (iOS quirk): rotate 90° CW for angle 90', () => {
    // iOS Safari keeps the frame portrait even when the device is held landscape.
    const g = computeCaptureGeometry(1080, 1920, 90);
    expect(g.canvasWidth).toBe(1920);
    expect(g.canvasHeight).toBe(1080);
    expect(g.translateX).toBe(1920);
    expect(g.translateY).toBe(0);
    expect(g.rotateRadians).toBeCloseTo(Math.PI / 2);
  });

  it('landscape device + portrait frame (iOS quirk): rotate 90° CCW for angle 270', () => {
    const g = computeCaptureGeometry(1080, 1920, 270);
    expect(g.canvasWidth).toBe(1920);
    expect(g.canvasHeight).toBe(1080);
    expect(g.translateX).toBe(0);
    expect(g.translateY).toBe(1080);
    expect(g.rotateRadians).toBeCloseTo(-Math.PI / 2);
  });

  it('landscape device + landscape frame (Android Chrome): no rotation', () => {
    // Android Chrome rotates the frame to match device orientation, so applying
    // our rotation would over-rotate. Detected via aspect-ratio match.
    const g = computeCaptureGeometry(1920, 1080, 90);
    expect(g.canvasWidth).toBe(1920);
    expect(g.canvasHeight).toBe(1080);
    expect(g.rotateRadians).toBe(0);
  });

  it('upside-down portrait (angle 180): rotate 180°', () => {
    const g = computeCaptureGeometry(1080, 1920, 180);
    expect(g.canvasWidth).toBe(1080);
    expect(g.canvasHeight).toBe(1920);
    expect(g.translateX).toBe(1080);
    expect(g.translateY).toBe(1920);
    expect(g.rotateRadians).toBeCloseTo(Math.PI);
  });

  it('negative angle (-90 alias of 270) normalizes correctly', () => {
    const g = computeCaptureGeometry(1080, 1920, -90);
    expect(g.canvasWidth).toBe(1920);
    expect(g.canvasHeight).toBe(1080);
    expect(g.rotateRadians).toBeCloseTo(-Math.PI / 2);
  });

  it('unknown angle (e.g. 45) falls back to no rotation', () => {
    const g = computeCaptureGeometry(1080, 1920, 45);
    expect(g.canvasWidth).toBe(1080);
    expect(g.canvasHeight).toBe(1920);
    expect(g.rotateRadians).toBe(0);
  });
});
