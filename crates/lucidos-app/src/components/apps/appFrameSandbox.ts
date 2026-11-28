
export const APP_FRAME_SANDBOX =
  'allow-scripts allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox allow-downloads';

export const APP_FRAME_ISOLATED = !APP_FRAME_SANDBOX.includes('allow-same-origin');

export const APP_FRAME_ALLOW = 'autoplay; fullscreen *; encrypted-media; clipboard-write';
