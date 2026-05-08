import { useEffect } from 'preact/hooks';
import { computed } from '@preact/signals';
import {
  confirmState,
  popupImage,
  searchEverywhereOpen,
} from '../store/store';
import { drawerOpen } from '../components/layout/Drawer';

const anyOverlayOpen = computed(() =>
  confirmState.value.visible ||
  drawerOpen.value ||
  !!popupImage.value ||
  searchEverywhereOpen.value
);

let scrollY = 0;

/** iOS Safari ignores overflow:hidden on body — the position:fixed trick is needed there. */
function isMobileOrTouch(): boolean {
  return window.matchMedia('(max-width: 768px)').matches ||
    ('ontouchstart' in window && navigator.maxTouchPoints > 0);
}

export function useScrollLock(): void {
  useEffect(() => {
    return anyOverlayOpen.subscribe((locked) => {
      if (!isMobileOrTouch()) return;

      const app = document.getElementById('app');
      if (!app) return;

      if (locked) {
        scrollY = window.scrollY;
        app.style.position = 'fixed';
        app.style.top = `-${scrollY}px`;
        app.style.left = '0';
        app.style.right = '0';
        app.style.bottom = '0';
        app.style.overflow = 'hidden';
      } else {
        app.style.position = '';
        app.style.top = '';
        app.style.left = '';
        app.style.right = '';
        app.style.bottom = '';
        app.style.overflow = '';
        window.scrollTo(0, scrollY);
      }
    });
  }, []);
}
