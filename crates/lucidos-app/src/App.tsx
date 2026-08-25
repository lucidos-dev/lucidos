import { effect, signal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { SplitLayout } from './components/layout/SplitLayout';
import { ThreadPane } from './components/layout/ThreadPane';
import { ContentPane } from './components/layout/ContentPane';
import { AppHeader } from './components/layout/AppHeader';
import { BackupReminderBanner } from './components/layout/BackupReminderBanner';
import { ConnectionBanner } from './components/layout/ConnectionBanner';
import { Drawer } from './components/layout/Drawer';
import { MobileSwipeContainer } from './components/layout/MobileSwipeContainer';
import { OverlayLayer } from './components/layout/OverlayLayer';
import { ConfirmDialog } from './components/shared/ConfirmDialog';
import { ProgressDialog } from './components/shared/ProgressDialog';
import { PromptDialog } from './components/shared/PromptDialog';
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
import { useWindowDragRegion } from './hooks/useWindowDragRegion';
import { lazyComponent } from './utils/lazyComponent';
import {
  fileSearchOpen,
  popupImage,
  messageRoutePanel,
  stepDetailModal,
  checkpointDiffModal,
  contextViewer,
  eventConditionModal,
  searchEverywhereOpen,
  filePreviewModal,
} from './store/store';
import { scaleModalOpen } from './components/shared/scaleModalState';
import { releaseNoticeModalOpen } from './store/releaseNotices';
import { viewportIsMobile } from './utils/viewport';

const FileSearchModal = lazyComponent(() => import('./components/files/FileSearchModal').then(m => m.FileSearchModal));
const ImagePopup = lazyComponent(() => import('./components/shared/ImagePopup').then(m => m.ImagePopup));
const MessageRoutePanel = lazyComponent(() => import('./components/chat/MessageRoutePanel').then(m => m.MessageRoutePanel));
const StepDetailModal = lazyComponent(() => import('./components/chat/StepDetailModal').then(m => m.StepDetailModal));
const CheckpointDiffModal = lazyComponent(() => import('./components/chat/CheckpointDiffModal').then(m => m.CheckpointDiffModal));
const ContextViewerModal = lazyComponent(() => import('./components/chat/ContextViewerModal').then(m => m.ContextViewerModal));
const EventConditionModal = lazyComponent(() => import('./components/chat/EventConditionModal').then(m => m.EventConditionModal));
const ScaleModal = lazyComponent(() => import('./components/shared/ScaleModal').then(m => m.ScaleModal));
const SearchEverywhere = lazyComponent(() => import('./components/search/SearchEverywhere').then(m => m.SearchEverywhere));
const FilePreviewModal = lazyComponent(() => import('./components/files/FilePreviewModal').then(m => m.FilePreviewModal));
const ReleaseNoticeModal = lazyComponent(() => import('./components/shared/ReleaseNoticeModal').then(m => m.ReleaseNoticeModal));

// Each overlay sits in its own slot so the signal subscription is leaf-scoped:
// a flip re-renders the slot, not all of App and its descendants.
function ImagePopupSlot()        { return popupImage.value           ? <ImagePopup />        : null; }
function MessageRoutePanelSlot() { return messageRoutePanel.value    ? <MessageRoutePanel /> : null; }
function StepDetailModalSlot()   { return stepDetailModal.value      ? <StepDetailModal />   : null; }
function CheckpointDiffSlot()    { return checkpointDiffModal.value  ? <CheckpointDiffModal /> : null; }
function ContextViewerSlot()     { return contextViewer.value        ? <ContextViewerModal /> : null; }
function EventConditionSlot()    { return eventConditionModal.value  ? <EventConditionModal /> : null; }
function ScaleModalSlot()        { return scaleModalOpen.value       ? <ScaleModal />        : null; }
// Slot-gated for a second reason beyond the leaf-scoped subscription: it keeps
// the file preview renderers (which the content pane already lazy-loads) out of
// the initial bundle for every user who never opens an app that previews a file.
function FilePreviewModalSlot()  { return filePreviewModal.value     ? <FilePreviewModal />  : null; }
// Most workspaces owe no release notice, so the gate keeps the modal's chunk
// off every load that will never draw it.
function ReleaseNoticeSlot()     { return releaseNoticeModalOpen()   ? <ReleaseNoticeModal /> : null; }

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
  // still ran subscriptions, render, and FLIP layout reads on 100+ rows. Gating
  // on viewportIsMobile keeps one subtree alive. The signal self-corrects on
  // real viewport changes (rotation / wake / resize — see utils/viewport.ts), so
  // a wrong initial read on an iOS PWA cold launch or landscape start re-mounts
  // into the correct layout instead of stranding a portrait phone in the desktop
  // split. A genuine breakpoint crossing is rare, so the per-render cost is ~0.
  const mobile = viewportIsMobile.value;
  // Reclaimed macOS title-bar band: drags the window and double-click-zooms it.
  // Drag/zoom go through always-allowed app commands (useWindowDragRegion), since
  // data-tauri-drag-region's window-plugin IPC is denied by our capability ACL.
  const stripRef = useRef<HTMLDivElement>(null);
  useWindowDragRegion(stripRef, { maximize: true });
  return (
    <>
      <div class="app-shell">
        {/* Reclaimed macOS title-bar band + window drag region. Collapses to 0px
            (and is inert) off the macOS Tauri build — see .titlebar-strip CSS. */}
        <div ref={stripRef} class="titlebar-strip" />
        <AppHeader />
        {/* Desktop's app-shell banners: flow siblings that push the panes down.
            The mobile copies are mounted inside the fixed header instead (see
            AppHeader), and each renders only under its own viewport.
            The connection bar leads: it says the workspace is unreachable right
            now, where the reminder below it is about a risk that will still be
            there later. */}
        <ConnectionBanner layout="desktop" />
        <BackupReminderBanner layout="desktop" />
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

      {/* Overlays -- rendered outside the split layout.

          The layered group below moves as a unit into a natively fullscreen app
          panel, because a fullscreen element is painted alone and nothing
          outside its subtree can be seen at any z-index. Keeping them together
          is what preserves their z-order. The drop dispatcher (renders null) and
          the UI blocker (inerts its own siblings, so it must stay a child of the
          app root) are deliberately outside the layer, and OverlayLayer's doc
          comment says why. */}
      <OverlayLayer>
        <FileSearchModalSlot />
        <Toast />
        <ImagePopupSlot />
        <MessageRoutePanelSlot />
        <ConfirmDialog />
        <ProgressDialog />
        <PromptDialog />
        <StepDetailModalSlot />
        <CheckpointDiffSlot />
        <ContextViewerSlot />
        <EventConditionSlot />
        <FilePreviewModalSlot />
        <ScaleModalSlot />
        <ReleaseNoticeSlot />
        <SearchEverywhereSlot />
      </OverlayLayer>
      <DropZone />
      <UiBlockingOverlay />
    </>
  );
}
