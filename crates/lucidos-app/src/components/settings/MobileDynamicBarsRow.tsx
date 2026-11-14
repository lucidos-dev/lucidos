import { currentMobileDynamicBars, setMobileDynamicBars } from '../../store/actions/preferences';
import { isIOSPwa } from '../../utils/platform';
import { Explainer } from '../shared/Explainer';

/** The Dynamic bars switch. The installed iOS app always runs dynamic bars
 *  (ADR 0553), so there it shows on and disabled, with a note why. */
export function MobileDynamicBarsRow() {
  const alwaysOn = isIOSPwa();
  return (
    <>
      <div class="settings-row" data-search-anchor="appearance:mobile-dynamic-bars">
        <span class="settings-row-label">
          Dynamic bars
          <Explainer title="Dynamic bars">
            <p>
              The header, the thread title and the prompt glide away as
              you scroll down a thread, and come back as you scroll up.
            </p>
            <p>Off, they stay in place.</p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={currentMobileDynamicBars()}
            disabled={alwaysOn}
            onChange={(e) => void setMobileDynamicBars((e.currentTarget as HTMLInputElement).checked)}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      {alwaysOn && (
        <div class="settings-row-note" data-testid="dynamic-bars-always-on-note">
          Always on in the iOS app.
        </div>
      )}
    </>
  );
}
