import { useState } from 'preact/hooks';
import { currentAutocorrect, currentCaptureContext, setAutocorrect, setCaptureContext } from '../../store/actions/preferences';
import { clearPerfOptIn, isPerfEnabled, setPerfEnabled } from '../../utils/perfQueue';
import { isIOS } from '../../utils/platform';
import { animationSpeed, enginePackaged, preferences, speedMultiplier } from '../../store/store';
import { reducedMotion } from '../../utils/motion';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import { openSettingsSubview } from '../../store/actions/menu';
import { restartControlHome } from './restartControl';
import { Explainer } from '../shared/Explainer';
import { LoadableToggle } from '../shared/LoadableToggle';

export function DebuggingSection() {
  const [perfOn, setPerfOn] = useState(() => isPerfEnabled());
  const packaged = enginePackaged.value;

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
              From-source installs record automatically. Packaged installs start
              with recording off. The switch applies to this device immediately.
            </p>
            <p>
              A device that turned the switch on earlier keeps that stored choice
              when it opens a from-source workspace. Clear it here. Recording
              stays on from source, and the phone's viewport poll, which only
              the stored choice turns on, stops.
            </p>
          </Explainer>
        </span>
        {packaged === false && (
          <div class="settings-row-options" style="gap: 0.5rem; align-items: center">
            <span>Always on</span>
            {perfOn && (
              <button
                class="action-btn"
                aria-label="Clear stored perf opt-in"
                onClick={() => {
                  clearPerfOptIn();
                  setPerfOn(isPerfEnabled());
                }}
              >
                Clear stored opt-in
              </button>
            )}
          </div>
        )}
        {packaged === true && (
          <label class="toggle-switch">
            <input
              type="checkbox"
              aria-label="Perf instrumentation"
              checked={perfOn}
              onChange={(e) => {
                const on = (e.currentTarget as HTMLInputElement).checked;
                setPerfEnabled(on);
                setPerfOn(isPerfEnabled());
              }}
            />
            <span class="toggle-slider" />
          </label>
        )}
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
            <p>
              While motion is reduced, by Motion in Settings → Appearance or by
              this device's own setting, animations are off and this has no effect.
            </p>
          </Explainer>
        </span>
        <div class="settings-row-options" style="gap: 0.5rem; align-items: center">
          <input
            type="range"
            min="-10"
            max="10"
            step="1"
            disabled={reducedMotion.value}
            aria-label="Animation speed"
            value={animationSpeed.value}
            onInput={(e) => {
              animationSpeed.value = parseInt((e.target as HTMLInputElement).value);
            }}
            style="width: 7rem"
          />
          <span class="settings-row-label" style="min-width: 2.5rem; text-align: right">
            {reducedMotion.value ? 'Off' : `${speedMultiplier.value.toFixed(1)}x`}
          </span>
        </div>
      </div>
      {isIOS() && (
        <div class="settings-row" data-search-anchor="debugging:autocorrect">
          <span class="settings-row-label">
            Autocorrect
            <Explainer title="Autocorrect">
              <p>
                Whether iOS corrects words as you type in Lucidos and its apps, on this device.
              </p>
              <p>
                When autocorrect has just changed a word, iOS can keep the next
                tap for itself, so Send or Submit does nothing. Close the
                keyboard and tap again. If it happens often, turn autocorrect
                off here.
              </p>
              <p>Spell-check underlines and sentence capitals stay either way.</p>
            </Explainer>
          </span>
          <LoadableToggle
            loaded={preferences.value.status === 'loaded'}
            checked={currentAutocorrect()}
            onChange={(c) => void setAutocorrect(c)}
          />
        </div>
      )}
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
      {restartControlHome(packaged === true) === 'debugging' && (
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
