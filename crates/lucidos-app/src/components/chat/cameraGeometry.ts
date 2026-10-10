// iOS Safari `getUserMedia` delivers frames in the device's natural orientation
// (portrait on iPhone) regardless of how the device is held; Android Chrome
// rotates the frame to match device orientation. `canvas.drawImage(video)`
// captures the raw underlying frame, so on iOS a landscape-held phone produces
// a JPEG rotated 90° from what the user saw. We detect the mismatch by
// aspect ratio: if the frame's orientation doesn't match the device's, we
// rotate to compensate. When they already match (Android, or iOS in portrait),
// no rotation is applied.

export interface CaptureGeometry {
  canvasWidth: number;
  canvasHeight: number;
  translateX: number;
  translateY: number;
  rotateRadians: number;
}

const NO_ROTATION = (videoWidth: number, videoHeight: number): CaptureGeometry => ({
  canvasWidth: videoWidth,
  canvasHeight: videoHeight,
  translateX: 0,
  translateY: 0,
  rotateRadians: 0,
});

export function computeCaptureGeometry(
  videoWidth: number,
  videoHeight: number,
  deviceAngleDegrees: number,
): CaptureGeometry {
  const angle = ((deviceAngleDegrees % 360) + 360) % 360;
  const deviceLandscape = angle === 90 || angle === 270;
  const frameLandscape = videoWidth > videoHeight;
  // If frame orientation already matches device orientation, the platform
  // (or the user holding portrait) is doing the right thing — don't rotate.
  // Only the 180° case is ambiguous (same aspect ratio, but flipped); we still
  // apply a 180° rotation there so an upside-down hold produces a right-side-up
  // image.
  if (deviceLandscape === frameLandscape && angle !== 180) {
    return NO_ROTATION(videoWidth, videoHeight);
  }
  switch (angle) {
    case 90:
      return {
        canvasWidth: videoHeight,
        canvasHeight: videoWidth,
        translateX: videoHeight,
        translateY: 0,
        rotateRadians: Math.PI / 2,
      };
    case 270:
      return {
        canvasWidth: videoHeight,
        canvasHeight: videoWidth,
        translateX: 0,
        translateY: videoWidth,
        rotateRadians: -Math.PI / 2,
      };
    case 180:
      return {
        canvasWidth: videoWidth,
        canvasHeight: videoHeight,
        translateX: videoWidth,
        translateY: videoHeight,
        rotateRadians: Math.PI,
      };
    default:
      return NO_ROTATION(videoWidth, videoHeight);
  }
}

export function readDeviceAngle(): number {
  if (typeof window === 'undefined') return 0;
  const orientation = window.screen?.orientation;
  if (orientation && typeof orientation.angle === 'number') {
    return orientation.angle;
  }
  // Fallback for older Safari (deprecated but still present in iOS PWA).
  const legacy = (window as unknown as { orientation?: number }).orientation;
  return typeof legacy === 'number' ? legacy : 0;
}
