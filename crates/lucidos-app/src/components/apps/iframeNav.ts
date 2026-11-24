import { APP_FRAME_ISOLATED } from './appFrameSandbox';
import { tellFrame } from '../../store/actions/app-frame-channel';


export function setAppFrameHash(iframe: HTMLIFrameElement, fragment: string): boolean {
  const win = iframe.contentWindow;
  if (!win) return false;
  if (APP_FRAME_ISOLATED) return tellFrame(iframe, 'hash', { fragment });
  const target = new URL(win.location.href);
  target.hash = fragment;
  if (target.href === win.location.href) return false;
  win.location.replace(target.href);
  return true;
}

export function splitFrameSrc(src: string): { doc: string; fragment: string } {
  const hash = src.indexOf('#');
  if (hash === -1) return { doc: src, fragment: '' };
  return { doc: src.slice(0, hash), fragment: src.slice(hash + 1) };
}
