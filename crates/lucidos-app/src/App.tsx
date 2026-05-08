import { SplitLayout } from './components/layout/SplitLayout';
import { ThreadPane } from './components/layout/ThreadPane';
import { ContentPane } from './components/layout/ContentPane';
import { AppHeader } from './components/layout/AppHeader';
import { Drawer } from './components/layout/Drawer';
import { MobileSwipeContainer } from './components/layout/MobileSwipeContainer';
import { ConfirmDialog } from './components/shared/ConfirmDialog';
import { ImagePopup } from './components/shared/ImagePopup';
import { MessageRoutePanel } from './components/chat/MessageRoutePanel';
import { ScaleModal } from './components/shared/ScaleModal';
import { ThreadDrawer } from './components/drawer/ThreadDrawer';
import { DrawerDivider } from './components/layout/DrawerDivider';
import { DropZone } from './components/files/DropZone';
import { FileSearchModal } from './components/files/FileSearchModal';
import { RestartOverlay } from './components/layout/RestartOverlay';
import { SearchEverywhere } from './components/search/SearchEverywhere';
import { NotificationsModal } from './components/notifications/NotificationsModal';
import { Toast } from './components/shared/Toast';
import { useStartup } from './hooks/useStartup';
import { useTooltip } from './hooks/useTooltip';
import { useScrollLock } from './hooks/useScrollLock';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';

export function App() {
  useStartup();
  useTooltip();
  useScrollLock();
  useKeyboardShortcuts();

  return (
    <>
      <div class="app-shell">
        <AppHeader />
        <Drawer />
        <div class="content-row">
          <ThreadDrawer />
          <DrawerDivider />
          <SplitLayout
            threadPane={<ThreadPane />}
            contentPane={<ContentPane layout="desktop" />}
          />
        </div>
        {/* Mobile: swipeable thread/content views */}
        <MobileSwipeContainer />
      </div>

      {/* Landscape lock — CSS-only; visible only on phones in landscape */}
      <div class="landscape-lock" role="alert">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="6" y="2" width="12" height="20" rx="2" />
          <path d="M3 14a9 9 0 0 1 9-9" />
          <polyline points="3 9 3 14 8 14" />
        </svg>
        <p>Please rotate your device to portrait</p>
      </div>

      {/* Overlays -- rendered outside the split layout */}
      <FileSearchModal />
      <Toast />
      <DropZone />
      <ImagePopup />
      <MessageRoutePanel />
      <ConfirmDialog />
      <ScaleModal />
      <NotificationsModal />
      <SearchEverywhere />
      <RestartOverlay />
    </>
  );
}
