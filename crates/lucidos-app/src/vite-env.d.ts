/// <reference types="vite/client" />

declare module 'virtual:engine-version' {
  /** Engine VERSION baked into the client bundle at Vite load time (or '0.0.0-dev'). */
  export const ENGINE_VERSION: string;
}

declare module 'virtual:build-id' {
  /** Per-build id stamped into the client bundle at build time — the SAME id the
   *  `lucidos-sw-stamp` plugin writes into sw.js. Identifies the build that
   *  produced the running code, so the client can detect when a newer build is
   *  being served. The literal `__LUCIDOS_BUILD_ID__` placeholder in the live
   *  dev server (where the stamp plugin is inert). */
  export const CLIENT_BUILD_ID: string;
}
