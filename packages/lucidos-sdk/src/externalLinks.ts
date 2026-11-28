
import { onHostPush } from './_bridge';
import { traceToHost } from './hostTrace';
import { inDesktopApp, inIOSStandalone } from './platform';

export const EXTERNAL_LINKS_CHANNEL = 'external-links';

export type FrameOpens = 'tab' | 'safari' | 'host';

const FRAME_OPENS: readonly FrameOpens[] = ['tab', 'safari', 'host'];

export interface ExternalLinksPush {
  frameOpens: FrameOpens;
}

let frameOpens: FrameOpens = 'host';

export function adoptHostExternalLinks(data: unknown): void {
  const given = (data as Partial<ExternalLinksPush> | null)?.frameOpens;
  frameOpens = FRAME_OPENS.find((mode) => mode === given) ?? 'host';
}

export function currentFrameOpens(): FrameOpens {
  return frameOpens;
}

export function pageOpensTabs(): boolean {
  if (typeof window === 'undefined') return false;
  if (window.parent !== window) return frameOpens === 'tab';
  return !inDesktopApp() && !inIOSStandalone();
}

export function pageHandsOffToSafari(): boolean {
  return typeof window !== 'undefined' && window.parent !== window && frameOpens === 'safari';
}

/** iOS's escape hatch from a standalone PWA to the Safari app: a URL with this
 *  prefix goes to the system. The host's `utils/openExternalUrl.ts` imports it.
 *
 *  Temporary measure (undocumented Apple scheme): `docs/temporary-measures.md`
 *  § "`x-safari-` scheme prefix to escape an iOS standalone PWA". */
export const SAFARI_SCHEME_PREFIX = 'x-safari-';

export function openTabNow(url: string): boolean {
  const opened = window.open(url, '_blank');
  if (!opened || opened.closed) return false;
  try {
    opened.opener = null;
  } catch {
  }
  return true;
}

export function openSafariNow(url: string): boolean {
  return openTabNow(SAFARI_SCHEME_PREFIX + url);
}

export function traceAppLink(step: string, detail: Record<string, unknown> = {}): void {
  traceToHost('log.app-link', step, detail);
}

export function installExternalLinksListener(): () => void {
  return onHostPush(EXTERNAL_LINKS_CHANNEL, adoptHostExternalLinks);
}
