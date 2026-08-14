import { useEffect } from 'preact/hooks';
import { toastPlacement, isToastPlacement } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { backupReminderBody } from '../layout/BackupReminderBanner';
import { connectionBannerBody } from '../layout/ConnectionBanner';
import {
  TOAST_PLACEMENT_OPTIONS,
  sampleShortToast,
  sampleLongToast,
  sampleErrorToast,
  sampleActionToast,
  sampleProgressToast,
  sampleToastBurst,
  sampleConfirmDanger,
  sampleConfirmDefault,
  samplePrompt,
  sampleAcknowledgeWedged,
  sampleAcknowledgeDeferred,
  sampleAcknowledgeStranded,
  sampleConsentPrompt,
  playSampleProgressDialog,
  showSampleProgressPhase,
  stopSampleProgressDialog,
  sampleRestartDialog,
} from './communicationSamples';

/** Settings → System → Communication Surfaces: every way Lucidos speaks to the
 *  user, on one page, rendered against realistic content.
 *
 *  Three surfaces, split by the WEIGHT of the message rather than its origin.
 *  A toast is transient and ignorable. A banner is a condition that stays true
 *  until it ends. A dialog is irreversible or needs an answer now. The page
 *  exists because that split is only judgeable side by side.
 *
 *  Permanent, not scaffolding. These surfaces get iterated on, and this is
 *  where that happens. The one temporary thing on it is the toast-placement
 *  picker, which goes when a shape is chosen (docs/temporary-measures.md).
 *
 *  NOTHING HERE PERFORMS AN OPERATION. Every button renders a surface and
 *  stops. See `communicationSamples.ts` and its inertness guard. */
export function CommunicationSurfacesPage() {
  // A fake progress run would otherwise outlive the page. It is an interval
  // writing a store signal, and navigating away would leave a modal over the
  // app with nothing left to stop it.
  useEffect(() => stopSampleProgressDialog, []);

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="surfaces:toasts">Toasts</div>
        <p class="settings-row-note">
          Transient status and deferrable offers. Ignorable by design: a toast that
          must be answered belongs in a dialog.
        </p>
        <div class="settings-row" data-search-anchor="surfaces:toast-placement">
          <span class="settings-row-label">
            Placement
            <Explainer title="Toast placement">
              <p>
                Where the stack sits and how wide a toast is drawn. A full-width bar
                covers the pane divider; a narrower card sits on it, with the seam
                showing above and below.
              </p>
              <p>
                Per-device and temporary: this picker exists to choose a shape, and
                goes once one is chosen. Takes effect immediately, no reload.
              </p>
            </Explainer>
          </span>
          <div class="settings-row-options">
            <Dropdown
              options={TOAST_PLACEMENT_OPTIONS}
              value={toastPlacement.value}
              onChange={(v) => { if (isToastPlacement(v)) toastPlacement.value = v; }}
            />
          </div>
        </div>
        <div class="settings-row">
          <span class="settings-row-label">Samples</span>
          <div class="settings-row-options surfaces-sample-buttons">
            <button class="action-btn" onClick={sampleShortToast}>Short</button>
            <button class="action-btn" onClick={sampleLongToast}>Long</button>
            <button class="action-btn" onClick={sampleErrorToast}>Error</button>
            <button class="action-btn" onClick={sampleActionToast}>With actions</button>
            <button class="action-btn" onClick={sampleProgressToast}>Progress</button>
            <button class="action-btn" onClick={sampleToastBurst}>Burst of four</button>
          </div>
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="surfaces:banners">Banners</div>
        <p class="settings-row-note">
          A condition that stays true until something changes. Rendered inline here
          rather than fired, so neither one has to be provoked into existing.
        </p>
        {/* The real bodies, not copies. Both are already hook-free pure
            functions for exactly this reason, so the gallery cannot drift from
            what the app shows. */}
        <div class="surfaces-banner-preview">
          {backupReminderBody({ layout: 'desktop', onSetUp: () => {}, onDismiss: () => {} })}
        </div>
        <div class="surfaces-banner-preview">
          {connectionBannerBody({ layout: 'desktop', status: 'disconnected', workspace: 'dev' })}
        </div>
        <div class="surfaces-banner-preview">
          {connectionBannerBody({ layout: 'desktop', status: 'connecting', workspace: 'dev' })}
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="surfaces:dialogs">Dialogs</div>
        <p class="settings-row-note">
          Irreversible, or needing an answer now. A dialog blocks, so it has to earn
          it: everything that can be deferred is a toast instead.
        </p>
        <div class="settings-row">
          <span class="settings-row-label">Shipping today</span>
          <div class="settings-row-options surfaces-sample-buttons">
            <button class="action-btn" onClick={sampleConfirmDanger}>Confirm, danger</button>
            <button class="action-btn" onClick={sampleConfirmDefault}>Confirm, default</button>
            <button class="action-btn" onClick={samplePrompt}>Prompt</button>
          </div>
        </div>
        <div class="settings-row">
          <span class="settings-row-label">
            Proposed
            <Explainer title="Proposed dialogs">
              <p>
                Four messages that are toasts today but behave like dialogs: each sets
                itself non-dismissable and offers a single button, which is an
                acknowledgement box with no way out.
              </p>
              <p>
                Nothing is wired to these yet. They render here so the shape can be
                judged before any real flow is pointed at them.
              </p>
            </Explainer>
          </span>
          <div class="settings-row-options surfaces-sample-buttons">
            <button class="action-btn" onClick={sampleAcknowledgeWedged}>Cannot deliver</button>
            <button class="action-btn" onClick={sampleAcknowledgeDeferred}>Applied, deferred</button>
            <button class="action-btn" onClick={sampleAcknowledgeStranded}>Not served yet</button>
            <button class="action-btn" onClick={sampleConsentPrompt}>Thread wants to open</button>
          </div>
        </div>
        <div class="settings-row">
          <span class="settings-row-label">
            Progress dialog
            <Explainer title="Progress dialog">
              <p>
                For the two flows that take the workspace away and bring it back: the
                packaged update install, and an engine restart or version switch.
              </p>
              <p>
                Play walks the real install phases on a slow ticker, because several of
                them last under a second in reality and cannot otherwise be looked at.
              </p>
            </Explainer>
          </span>
          <div class="settings-row-options surfaces-sample-buttons">
            <button class="action-btn" onClick={playSampleProgressDialog}>Play install</button>
            <button class="action-btn" onClick={() => showSampleProgressPhase(1)}>Downloading</button>
            <button class="action-btn" onClick={() => showSampleProgressPhase(4)}>Committed</button>
            <button class="action-btn" onClick={sampleRestartDialog}>Engine restart</button>
          </div>
        </div>
      </div>
    </>
  );
}
