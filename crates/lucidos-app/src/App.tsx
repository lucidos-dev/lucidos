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
            contentPane={<ContentPane />}
          />
        </div>
        {/* Mobile: swipeable thread/content views */}
        <MobileSwipeContainer />
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
