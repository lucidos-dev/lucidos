import { effect, signal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { SplitLayout } from './components/layout/SplitLayout';
import { ThreadPane } from './components/layout/ThreadPane';
import { ContentPane } from './components/layout/ContentPane';
import { AppHeader } from './components/layout/AppHeader';
import { BackupReminderBanner } from './components/layout/BackupReminderBanner';
import { ConnectionBanner } from './components/layout/ConnectionBanner';
import { IngressBanner } from './components/layout/IngressBanner';
import { WebhookRefusalBanner } from './components/layout/WebhookRefusalBanner';
import { AgentSandboxBanner } from './components/layout/AgentSandboxBanner';
import { PeerClientBanner } from './components/layout/PeerClientBanner';
import { SlownessBanner } from './components/layout/SlownessBanner';
import { MobileSwipeContainer } from './components/layout/MobileSwipeContainer';
import { OverlayLayer } from './components/layout/OverlayLayer';
import { ProgressDialog } from './components/shared/ProgressDialog';
import { ThreadDrawer } from './components/drawer/ThreadDrawer';
import { DrawerDivider } from './components/layout/DrawerDivider';
import { DropZone } from './components/files/DropZone';
import { UiBlockingOverlay } from './components/layout/UiBlockingOverlay';
import { Toast } from './components/shared/Toast';
import { useBootSplashReady } from './hooks/useBootSplashReady';
import { useTooltip } from './hooks/useTooltip';
import { useScrollLock } from './hooks/useScrollLock';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { useWindowDragRegion } from './hooks/useWindowDragRegion';
import { lazyComponent } from './utils/lazyComponent';
import { prefetchWhenIdle } from './utils/idlePrefetch';
import {
  fileSearchOpen,
  popupImage,
  messageRoutePanel,
  stepDetailModal,
  checkpointDiffModal,
  contextViewer,
  eventConditionPopover,
  searchEverywhereOpen,
  filePreviewModal,
  confirmState,
  promptState,
  registerAskDialogChunk,
} from './store/store';
import { scaleModalOpen } from './components/shared/scaleModalState';
import { DrawerChunk, drawerOpen } from './components/layout/drawerState';
import { releaseNoticeModalOpen } from './store/releaseNotices';
import { memoryChoiceOpen } from './store/memoryChoice';
import { engineBuildLogOpen } from './store/engineBuildLog';
import { viewportIsMobile } from './utils/viewport';
import { CameraCaptureSlot } from './components/chat/PromptInput';
import { DashboardRail } from './components/dashboard/DashboardRail';
import { widgetFitRequests } from './store/widgetFit';

export { startShell } from './shellStartup';

const FileSearchModal = lazyComponent(() => import('./components/files/FileSearchModal').then(m => m.FileSearchModal));
const ImagePopup = lazyComponent(() => import('./components/shared/ImagePopup').then(m => m.ImagePopup));
const MessageRoutePanel = lazyComponent(() => import('./components/chat/MessageRoutePanel').then(m => m.MessageRoutePanel));
const EngineBuildLogModal = lazyComponent(() => import('./components/layout/EngineBuildLogModal').then(m => m.EngineBuildLogModal));
const StepDetailModal = lazyComponent(() => import('./components/chat/StepDetailModal').then(m => m.StepDetailModal));
const CheckpointDiffModal = lazyComponent(() => import('./components/chat/CheckpointDiffModal').then(m => m.CheckpointDiffModal));
const ContextViewerModal = lazyComponent(() => import('./components/chat/ContextViewerModal').then(m => m.ContextViewerModal));
const EventConditionPopover = lazyComponent(() => import('./components/chat/EventConditionPopover').then(m => m.EventConditionPopover));
const ScaleModal = lazyComponent(() => import('./components/shared/ScaleModal').then(m => m.ScaleModal));
const SearchEverywhere = lazyComponent(() => import('./components/search/SearchEverywhere').then(m => m.SearchEverywhere));
const FilePreviewModal = lazyComponent(() => import('./components/files/FilePreviewModal').then(m => m.loadFilePreviewModal()));
const ReleaseNoticeModal = lazyComponent(() => import('./components/shared/ReleaseNoticeModal').then(m => m.ReleaseNoticeModal));
const MemoryChoice = lazyComponent(() => import('./components/settings/MemoryChoice').then(m => m.MemoryChoice));
const WidgetFitCheck = lazyComponent(() => import('./components/widgets/WidgetFitCheck').then(m => m.WidgetFitCheck));
const ConfirmAndPromptDialogs = lazyComponent(() => import('./components/shared/ConfirmAndPromptDialogs').then(m => m.ConfirmAndPromptDialogs));
prefetchWhenIdle(ConfirmAndPromptDialogs);
registerAskDialogChunk(ConfirmAndPromptDialogs);

function ImagePopupSlot()        { return popupImage.value           ? <ImagePopup />        : null; }
function MessageRoutePanelSlot() { return messageRoutePanel.value    ? <MessageRoutePanel /> : null; }
function EngineBuildLogSlot()    { return engineBuildLogOpen.value ? <EngineBuildLogModal /> : null; }
function StepDetailModalSlot()   { return stepDetailModal.value      ? <StepDetailModal />   : null; }
function CheckpointDiffSlot()    { return checkpointDiffModal.value  ? <CheckpointDiffModal /> : null; }
function ContextViewerSlot()     { return contextViewer.value        ? <ContextViewerModal /> : null; }
function EventConditionSlot()    { return eventConditionPopover.value ? <EventConditionPopover /> : null; }
function ScaleModalSlot()        { return scaleModalOpen.value       ? <ScaleModal />        : null; }
function DrawerSlot()            { return drawerOpen.value           ? <DrawerChunk />       : null; }
function FilePreviewModalSlot()  { return filePreviewModal.value     ? <FilePreviewModal />  : null; }
function ReleaseNoticeSlot()     { return releaseNoticeModalOpen()   ? <ReleaseNoticeModal /> : null; }
function MemoryChoiceSlot()      { return memoryChoiceOpen()         ? <MemoryChoice />      : null; }
function WidgetFitCheckSlot()    { return widgetFitRequests.value.length > 0 ? <WidgetFitCheck /> : null; }

const fileSearchEverOpen = signal(false);
const searchEverywhereEverOpen = signal(false);
effect(() => { if (fileSearchOpen.value) fileSearchEverOpen.value = true; });
effect(() => { if (searchEverywhereOpen.value) searchEverywhereEverOpen.value = true; });

function FileSearchModalSlot()   { return fileSearchEverOpen.value      ? <FileSearchModal />   : null; }
const askDialogEverOpen = signal(false);
effect(() => { if (confirmState.value.visible || promptState.value.visible) askDialogEverOpen.value = true; });
function ConfirmAndPromptSlot()  { return askDialogEverOpen.value       ? <ConfirmAndPromptDialogs /> : null; }
function SearchEverywhereSlot()  { return searchEverywhereEverOpen.value ? <SearchEverywhere /> : null; }

export function App() {
  useBootSplashReady();
  useTooltip();
  useScrollLock();
  useKeyboardShortcuts();

  const mobile = viewportIsMobile.value;
  const stripRef = useRef<HTMLDivElement>(null);
  useWindowDragRegion(stripRef, { maximize: true });
  return (
    <>
      <div class="app-shell">
        <div ref={stripRef} class="titlebar-strip" />
        <AppHeader />
        <ConnectionBanner layout="desktop" />
        <SlownessBanner layout="desktop" />
        <IngressBanner layout="desktop" />
        <WebhookRefusalBanner layout="desktop" />
        <AgentSandboxBanner layout="desktop" />
        <PeerClientBanner layout="desktop" />
        <BackupReminderBanner layout="desktop" />
        <DrawerSlot />
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
        <DashboardRail />
      </div>

      <OverlayLayer>
        <FileSearchModalSlot />
        <Toast />
        <ImagePopupSlot />
        <MessageRoutePanelSlot />
        <ConfirmAndPromptSlot />
        <ProgressDialog />
        <StepDetailModalSlot />
        <EngineBuildLogSlot />
        <CheckpointDiffSlot />
        <ContextViewerSlot />
        <EventConditionSlot />
        <FilePreviewModalSlot />
        <ScaleModalSlot />
        <ReleaseNoticeSlot />
        <MemoryChoiceSlot />
        <SearchEverywhereSlot />
        <CameraCaptureSlot />
      </OverlayLayer>
      <DropZone />
      <WidgetFitCheckSlot />
      <UiBlockingOverlay />
    </>
  );
}
