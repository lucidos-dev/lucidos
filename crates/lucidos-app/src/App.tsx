import { effect, signal } from '@preact/signals';
import { SplitLayout } from './components/layout/SplitLayout';
import { ThreadPane } from './components/layout/ThreadPane';
import { ContentPane } from './components/layout/ContentPane';
import { AppHeader } from './components/layout/AppHeader';
import { Drawer } from './components/layout/Drawer';
import { MobileSwipeContainer } from './components/layout/MobileSwipeContainer';
import { ConfirmDialog } from './components/shared/ConfirmDialog';
import { LandscapeLock } from './components/shared/LandscapeLock';
import { ThreadDrawer } from './components/drawer/ThreadDrawer';
import { DrawerDivider } from './components/layout/DrawerDivider';
import { DropZone } from './components/files/DropZone';
import { UiBlockingOverlay } from './components/layout/UiBlockingOverlay';
import { Toast } from './components/shared/Toast';
import { useStartup } from './hooks/useStartup';
import { useBootSplashReady } from './hooks/useBootSplashReady';
import { useTooltip } from './hooks/useTooltip';
import { useScrollLock } from './hooks/useScrollLock';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { lazyComponent } from './utils/lazyComponent';
import {
  fileSearchOpen,
  popupImage,
  messageRoutePanel,
  stepDetailModal,
  searchEverywhereOpen,
} from './store/store';
import { scaleModalOpen } from './components/shared/scaleModalState';
import { viewportIsMobile } from './utils/viewport';

const FileSearchModal = lazyComponent(() => import('./components/files/FileSearchModal').then(m => m.FileSearchModal));
const ImagePopup = lazyComponent(() => import('./components/shared/ImagePopup').then(m => m.ImagePopup));
const MessageRoutePanel = lazyComponent(() => import('./components/chat/MessageRoutePanel').then(m => m.MessageRoutePanel));
const StepDetailModal = lazyComponent(() => import('./components/chat/StepDetailModal').then(m => m.StepDetailModal));
const ScaleModal = lazyComponent(() => import('./components/shared/ScaleModal').then(m => m.ScaleModal));
const SearchEverywhere = lazyComponent(() => import('./components/search/SearchEverywhere').then(m => m.SearchEverywhere));

// Each overlay sits in its own slot so the signal subscription is leaf-scoped:
// a flip re-renders the slot, not all of App and its descendants.
function ImagePopupSlot()        { return popupImage.value           ? <ImagePopup />        : null; }
function MessageRoutePanelSlot() { return messageRoutePanel.value    ? <MessageRoutePanel /> : null; }
function StepDetailModalSlot()   { return stepDetailModal.value      ? <StepDetailModal />   : null; }
function ScaleModalSlot()        { return scaleModalOpen.value       ? <ScaleModal />        : null; }

// FileSearchModal and SearchEverywhere render a hidden shell instead of
// unmounting when closed (avoids will-change:transform ghost pixels on iOS
// Safari PWAs). Latch on first open and stay mounted thereafter.
const fileSearchEverOpen = signal(false);
const searchEverywhereEverOpen = signal(false);
effect(() => { if (fileSearchOpen.value) fileSearchEverOpen.value = true; });
effect(() => { if (searchEverywhereOpen.value) searchEverywhereEverOpen.value = true; });

function FileSearchModalSlot()   { return fileSearchEverOpen.value      ? <FileSearchModal />   : null; }
function SearchEverywhereSlot()  { return searchEverywhereEverOpen.value ? <SearchEverywhere /> : null; }

export function App() {
  useStartup();
  useBootSplashReady();
  useTooltip();
  useScrollLock();
  useKeyboardShortcuts();

  // Mount only the visible layout. Dual-mounting fanned every signal write out
  // to two ThreadDrawer + ThreadPane + ContentPane subtrees — the inactive one
  // still ran subscriptions, render, and FLIP layout reads on 100+ rows. Safe
  // to gate once: viewportIsMobile only flips on window resize, which never
  // fires in an iOS PWA.
  const mobile = viewportIsMobile.value;
  return (
    <>
      <div class="app-shell">
        {/* Reclaimed macOS title-bar band + window drag region. Collapses to 0px
            (and is inert) off the macOS Tauri build — see .titlebar-strip CSS. */}
        <div class="titlebar-strip" data-tauri-drag-region />
        <AppHeader />
        <Drawer />
        {mobile ? (
          <MobileSwipeContainer />
        ) : (
          <div class="content-row">
            <ThreadDrawer />
            <DrawerDivider />
            <SplitLayout
              threadPane={<ThreadPane />}
              contentPane={<ContentPane layout="desktop" />}
            />
          </div>
        )}
      </div>

      <LandscapeLock />

      {/* Overlays -- rendered outside the split layout */}
      <FileSearchModalSlot />
      <Toast />
      <DropZone />
      <ImagePopupSlot />
      <MessageRoutePanelSlot />
      <ConfirmDialog />
      <StepDetailModalSlot />
      <ScaleModalSlot />
      <SearchEverywhereSlot />
      <UiBlockingOverlay />
    </>
  );
}
