import { useState } from 'preact/hooks';
import { currentCaptureContext, setCaptureContext } from '../../store/actions/preferences';
import { isPerfEnabled, setPerfEnabled } from '../../utils/perfQueue';
import { animationSpeed, enginePackaged, speedMultiplier } from '../../store/store';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import { openSettingsSubview } from '../../store/actions/menu';
import { restartControlHome } from './restartControl';
import { Explainer } from '../shared/Explainer';

/** Settings → System → Debugging: developer/diagnostic toggles, off by default.
 *
 *  - "Capture context per step" is a server preference (`capture_context`),
 *    moved here from the former Models → Debugging group so all diagnostics share
 *    one home. Calling `currentCaptureContext()` in render subscribes to
 *    preference updates (it reads `preferences.value`), same as LocaleSection.
 *  - "Perf instrumentation" toggles the per-device `lucidos:perf` localStorage
 *    flag that gates `recordPerfSample` (see utils/perfQueue.ts). It's NOT a
 *    server preference — device-local by design, read live, takes effect on the
 *    next sample with no reload. localStorage isn't reactive, so the checked
 *    state is held locally for immediate UI feedback; the gate reads the flag
 *    directly.
 *  - "Animation speed" scales every UI transition via `speedMultiplier` (a
 *    device-global slider persisted in localStorage, see store.ts/effects.ts).
 *    It lives here as a diagnostic — slowing animations down makes transition
 *    glitches inspectable; reading `animationSpeed.value` in render subscribes
 *    to the signal so the multiplier label updates live as you drag.
 *  - "Communication surfaces" opens the surface gallery, which renders every
 *    toast, banner and dialog against realistic content and performs no
 *    operation. It is its own subview rather than rows here, because it is a
 *    page of samples rather than a setting.
 *  - "Restart engine" is shown on a PACKAGED install only, and is the packaged
 *    counterpart of System > Overview's dev-only "Rebuild & Restart". Both sites
 *    branch on `restartControlHome`, so exactly one of the two ever renders. A
 *    packaged install ships its binary and has no source to rebuild, so the dev
 *    label was a lie there and the action is purely a recovery one: it belongs
 *    with the diagnostics, not on the page a user reads to see their version.
 *    Both routes go through `confirmAndRestartEngine`, the single
 *    confirm-then-restart entry point.
 *
 *    Its explainer deliberately does NOT name a mechanism, because the packaged
 *    restart has two shapes: the desktop app drives `restart_service`
 *    (`launchctl kickstart -k` on the LaunchAgent, taking the gateway and every
 *    workspace engine it spawned down with it), while a browser/PWA client POSTs
 *    /restart, which the engine forwards to the gateway control API to respawn
 *    just this workspace's stack. The blast radius is what the user needs, so
 *    that is what the explainer states.
 *
 *  Every row's prose is behind an *explainer* (`components/shared/Explainer.tsx`)
 *  rather than sitting under the row as a `.settings-row-note`: this page is
 *  four toggles and was three quarters grey paragraph. Nothing here is
 *  state-dependent or a next action, so all four moved. */
export function DebuggingSection() {
  // Lazy initializer: read localStorage once on mount, not on every render. The
  // panel remounts each time it's opened, so this reflects the current flag.
  const [perfOn, setPerfOn] = useState(() => isPerfEnabled());

  return (
    <div class="settings-section">
      <div class="settings-section-title">Debugging</div>
      <div class="settings-row" data-search-anchor="debugging:capture-context">
        <span class="settings-row-label">
          Capture context per step
          <Explainer title="Capture context per step">
            <p>
              Stores the text of each prompt section (system prompt, conversation,
              tools, memory, …) sent to the model on every step, so you can inspect what
              went into the context in a step's snapshot viewer.
            </p>
            <p>
              Off by default: it enlarges the event log. While off, the snapshot still
              shows each section's name and size, just not its contents.
            </p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={currentCaptureContext()}
            onChange={(e) => void setCaptureContext((e.currentTarget as HTMLInputElement).checked)}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      <div class="settings-row" data-search-anchor="debugging:perf">
        <span class="settings-row-label">
          Perf instrumentation
          <Explainer title="Perf instrumentation">
            <p>
              Logs thread-open / render / linkify timings to the engine log
              (<code>[Client/perf]</code> lines) for diagnosing lag.
            </p>
            <p>
              Per-device and off by default: turn it on only while measuring. Takes
              effect immediately, no reload.
            </p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={perfOn}
            onChange={(e) => {
              const on = (e.currentTarget as HTMLInputElement).checked;
              setPerfEnabled(on);
              setPerfOn(on);
            }}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      <div class="settings-row" data-search-anchor="debugging:animation-speed">
        <span class="settings-row-label">
          Animation speed
          <Explainer title="Animation speed">
            <p>
              Scales the duration of every UI transition (1.0x = normal). Slowing
              animations down makes transition glitches easier to inspect.
            </p>
            <p>Per-device, persisted locally, takes effect immediately.</p>
          </Explainer>
        </span>
        <div class="settings-row-options" style="gap: 0.5rem; align-items: center">
          <input
            type="range"
            min="-10"
            max="10"
            step="1"
            value={animationSpeed.value}
            onInput={(e) => {
              animationSpeed.value = parseInt((e.target as HTMLInputElement).value);
            }}
            style="width: 7rem"
          />
          <span class="settings-row-label" style="min-width: 2.5rem; text-align: right">{speedMultiplier.value.toFixed(1)}x</span>
        </div>
      </div>
      <div class="settings-row" data-search-anchor="debugging:communication-surfaces">
        <span class="settings-row-label">
          Communication surfaces
          <Explainer title="Communication surfaces">
            <p>
              Every way Lucidos speaks to you, on one page: toasts, banners and
              dialogs, each rendered against realistic content.
            </p>
            <p>
              Nothing on that page performs an operation. It exists so these surfaces
              can be compared and iterated on without provoking the real thing.
            </p>
          </Explainer>
        </span>
        <button class="action-btn" onClick={() => openSettingsSubview('communication-surfaces')}>
          Open
        </button>
      </div>
      {restartControlHome(enginePackaged.value) === 'debugging' && (
        <div class="settings-row" data-search-anchor="debugging:restart-engine">
          <span class="settings-row-label">
            Restart engine
            <Explainer title="Restart engine">
              <p>
                Stops this workspace's engine and starts it again. In the desktop app
                it restarts the whole background service, so every workspace it runs
                goes down and comes back with it.
              </p>
              <p>
                Nothing is rebuilt: this install ships its binary, so a restart is a
                recovery action for an unresponsive engine, not how you pick up a new
                version. Use Check for Updates under Overview for that.
              </p>
            </Explainer>
          </span>
          <button class="action-btn" onClick={() => { void confirmAndRestartEngine(); }}>
            Restart Engine
          </button>
        </div>
      )}
    </div>
  );
}
