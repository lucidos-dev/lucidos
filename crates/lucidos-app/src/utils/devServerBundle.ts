
export function isDevServerBundle(): boolean {
  return import.meta.env.DEV === true;
}

export const DEV_SERVER_SW_REASON =
  'This page is the frontend preview (a Vite dev server), which registers no service worker, so push cannot be enabled here. Enable it in the real app instead.';
