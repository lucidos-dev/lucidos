import { updatePanelUrl } from '../../store/actions/artifacts';
import { useEffect, useRef, useCallback } from 'preact/hooks';
import { isTauri } from '../../utils/platform';
import { invoke, listen } from '../../utils/tauri';
import { panelUrl, panelTitle, webviewInitialUrl, showToast } from '../../store/store';
import { isMainFrameUrl } from '../../utils/urlFilter';
import { errorDetail } from '../../utils/errorDetail';
import { viewportIsMobile } from '../../utils/viewport';

interface Props {
  url: string;
  /** Skip mounting in the inactive dual-rendered layout — otherwise both
   *  SplitLayout and MobileSwipeContainer copies create a webview / iframe. */
  layout: 'desktop' | 'mobile';
}

export function UrlPreviewInline({ url, layout }: Props) {
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const containerRef = useRef<HTMLDivElement>(null);
  const webviewCreated = useRef(false);
  // Track the URL the native webview is currently showing (may differ from prop
  // when user navigates within the webview). This prevents recreating the webview
  // when the panelUrl signal is updated from an in-webview navigation event.
  const currentWebviewUrl = useRef(url);
  // Track whether we've captured the actual loaded URL from the first page load.
  // Redirects can change the URL, making webviewInitialUrl (set from the requested
  // URL) differ from the actual loaded URL. Capturing the first panel-url-changed
  // event fixes the mismatch so webviewHasHistory works correctly.
  const initialUrlCaptured = useRef(false);

  const createWebview = useCallback(async (targetUrl: string) => {
    const el = containerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    try {
      await invoke('create_panel_webview', {
        url: targetUrl,
        x: rect.left,
        y: rect.top,
        width: rect.width,
        height: rect.height,
        viewportHeight: window.innerHeight,
      });
      webviewCreated.current = true;
      currentWebviewUrl.current = targetUrl;
    } catch (e) {
      showToast(`Failed to open preview: ${errorDetail(e)}`, 'error');
    }
  }, []);

  const updateBounds = useCallback(async () => {
    const el = containerRef.current;
    if (!el || !webviewCreated.current) return;
    const rect = el.getBoundingClientRect();
    try {
      await invoke('set_panel_webview_bounds', {
        x: rect.left,
        y: rect.top,
        width: rect.width,
        height: rect.height,
        viewportHeight: window.innerHeight,
      });
    } catch { /* webview may have been closed */ }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    if (!isActiveLayout) return;

    // Reset so the first panel-url-changed for the new URL captures redirects.
    initialUrlCaptured.current = false;

    // Only create a new webview if the URL changed from an external action
    // (e.g., clicking a link in chat), not from in-webview navigation.
    if (url !== currentWebviewUrl.current) {
      if (webviewCreated.current) {
        invoke('navigate_panel_webview', { url }).then(() => {
          currentWebviewUrl.current = url;
        }).catch(() => {
          // Webview may have been closed — recreate
          void createWebview(url);
        });
      } else {
        void createWebview(url);
      }
    } else if (!webviewCreated.current) {
      void createWebview(url);
    }
  }, [url, createWebview, isActiveLayout]);

  useEffect(() => {
    if (!isTauri()) return;
    if (!isActiveLayout) return;

    // Listen for URL changes from on_page_load (main-frame navigations only).
    // isMainFrameUrl filters as a safety net in case non-page URLs slip through.
    // `cleaned` guards the race where the effect's cleanup runs before `listen()`
    // resolves — without it the late `.then((fn) => { unlistenUrl = fn })` would
    // assign an unlisten that nothing ever calls, leaking a global-signal-writing
    // listener after the component is gone.
    let cleaned = false;
    let unlistenUrl: (() => void) | null = null;
    listen<string>('panel-url-changed', (e) => {
      const newUrl = e.payload;
      if (!newUrl || !isMainFrameUrl(newUrl)) return;

      if (!initialUrlCaptured.current) {
        initialUrlCaptured.current = true;
        webviewInitialUrl.value = newUrl;
      }

      if (newUrl !== panelUrl.value) {
        currentWebviewUrl.current = newUrl;
        updatePanelUrl(newUrl);
      }
    }).then((fn) => {
      if (cleaned) { fn(); return; }
      unlistenUrl = fn;
    }).catch((err) => {
      showToast(`Failed to listen for URL changes: ${errorDetail(err)}`, 'error');
    });

    // Listen for title changes from the native webview
    let unlistenTitle: (() => void) | null = null;
    listen<string>('panel-title-changed', (e) => {
      const title = e.payload;
      if (title !== panelTitle.value) {
        panelTitle.value = title || null;
      }
    }).then((fn) => {
      if (cleaned) { fn(); return; }
      unlistenTitle = fn;
    }).catch((err) => {
      showToast(`Failed to listen for title changes: ${errorDetail(err)}`, 'error');
    });

    // Track container and window resizes to reposition the native webview (debounced)
    const el = containerRef.current;
    let observer: ResizeObserver | null = null;
    let resizeTimer: ReturnType<typeof setTimeout> | null = null;
    const onResize = () => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(updateBounds, 16);
    };
    if (el) {
      observer = new ResizeObserver(onResize);
      observer.observe(el);
    }
    window.addEventListener('resize', onResize);

    return () => {
      cleaned = true;
      if (unlistenUrl) unlistenUrl();
      if (unlistenTitle) unlistenTitle();
      if (observer) observer.disconnect();
      window.removeEventListener('resize', onResize);
      if (resizeTimer) clearTimeout(resizeTimer);
      // Best-effort cleanup during unmount: the webview may already be closed
      // (race with another tab/layout flip) or the Tauri command may fail
      // because the host window is tearing down. No user-facing recovery path
      // exists — the next `createWebview` call recreates it on demand.
      invoke('close_panel_webview').catch(() => {});
      webviewCreated.current = false;
      currentWebviewUrl.current = '';
      panelTitle.value = null;
    };
  }, [isActiveLayout, updateBounds]);

  // Skip rendering in the inactive dual-rendered layout — otherwise both
  // SplitLayout and MobileSwipeContainer copies create a webview/iframe.
  if (!isActiveLayout) return null;

  // Browser mode: render an iframe (many sites block framing, but it's the best
  // we can do without a native webview). Tauri mode uses an overlay native webview.
  if (!isTauri()) {
    return (
      <div class="url-preview-inline">
        <iframe
          src={url}
          class="url-preview-frame"
          // No allow-downloads: this frame renders an arbitrary EXTERNAL page,
          // never our own app content, so it must not be able to start one.
          sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
          referrerpolicy="no-referrer"
        />
      </div>
    );
  }

  return (
    <div class="url-preview-inline">
      <div ref={containerRef} class="url-preview-frame" />
    </div>
  );
}
