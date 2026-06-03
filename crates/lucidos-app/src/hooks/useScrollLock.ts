import { useEffect } from 'preact/hooks';
import { computed } from '@preact/signals';
import {
  confirmState,
  popupImage,
  searchEverywhereOpen,
} from '../store/store';
import { drawerOpen } from '../components/layout/Drawer';
import { isMobileOrTouch } from '../utils/viewport';

const anyOverlayOpen = computed(() =>
  confirmState.value.visible ||
  drawerOpen.value ||
  !!popupImage.value ||
  searchEverywhereOpen.value
);

let scrollY = 0;

export function useScrollLock(): void {
  useEffect(() => {
    return anyOverlayOpen.subscribe((locked) => {
      if (!isMobileOrTouch()) return;

      const app = document.querySelector<HTMLElement>('#app');
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
