import { installScrollMemory } from './scroll';
import { installKeyboardForwarding } from './keyboardForward';
import { installScaleWheelForwarding } from './scaleWheelForward';
import { installAppPullToRefresh } from './pullToRefresh';
import { installAppPaneSwipe } from './paneSwipe';
import { installLinkPressGuard } from './linkPressGuard';
import { installAppLinkRouting } from './appLinks';
import { installExternalLinksListener, traceAppLink } from './externalLinks';
import { installFullScreenTrace } from './fullScreenTrace';
import { installAppScrollReport } from './appScroll';
import { installTooltips } from './tooltip';
import { primeDevicePreferences } from './ui';
import { installAutocorrectStamp } from './autocorrectStamp';
import { installNoKeyCodeText } from './noKeyCodeText';
import { isBridged } from './_bridge';
import { primeBridgedStorage } from './_storage';
import { installHostOps } from './hostOps';
import { installCapabilityRenewal, installDocumentRelativeUrls } from './frameCapability';
import { inSidewaysBox, installWidgetFrame } from './widgetFrame';
import { installOneSound } from './oneSound';

export * from './index';

if (typeof document !== 'undefined') {
  installAutocorrectStamp();
  installNoKeyCodeText();
  installHostOps();
  installCapabilityRenewal();
  installDocumentRelativeUrls();
  installWidgetFrame();
  installOneSound();
  if (isBridged()) {
    primeBridgedStorage()
      .catch((err) => {
        console.warn('[lucidos-sdk] could not read this app\'s stored values:', err);
      })
      .finally(() => { installScrollMemory(); });
  } else {
    installScrollMemory();
  }
  installTooltips({ longPressNeedsOptIn: false, hideAfterLongPressMs: 2000 });
  primeDevicePreferences().catch((err) => {
    console.warn('[lucidos-sdk] could not read this device\'s preferences:', err);
  });
  installKeyboardForwarding();
  installScaleWheelForwarding();
  installAppPullToRefresh();
  installAppPaneSwipe(inSidewaysBox);
  installAppScrollReport();

  installLinkPressGuard(window, {
    onLinkTouch: (info) => traceAppLink('touch', { ...info }),
    onSwallowed: (info) => {
      console.info('[lucidos-sdk] link-click-after-swipe', info);
      traceAppLink('swallowed', { ...info });
    },
  });

  installExternalLinksListener();
  installAppLinkRouting();

  installFullScreenTrace();
}
