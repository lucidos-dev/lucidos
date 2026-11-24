
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
  const legacy = (window as unknown as { orientation?: number }).orientation;
  return typeof legacy === 'number' ? legacy : 0;
}
