/// <reference types="vite/client" />

declare module 'virtual:engine-version' {
  /** Engine VERSION baked into the client bundle at Vite load time (or '0.0.0-dev'). */
  export const ENGINE_VERSION: string;
}
