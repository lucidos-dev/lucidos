import { isIOS } from './platform';
import { isNativeWindowActive } from './nativeWindow';

export function isPageActive(): boolean {
  if (typeof document === 'undefined') return false;
  if (!isNativeWindowActive()) return false;
  if (document.visibilityState !== 'visible') return false;
  if (isIOS()) return true;
  return document.hasFocus();
}
