import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { currentExternalLinkTarget } from './preferences';
import { mountedAppFrames } from '../../utils/appFrame';
import { isIOSPwa, isTauri } from '../../utils/platform';
// The SDK module that listens on the channel. Relative, because the barrel
// does not export it; it has no import-time side effects.
import {
  EXTERNAL_LINKS_CHANNEL,
  type ExternalLinksPush,
  type FrameOpens,
} from '../../../../../packages/lucidos-sdk/src/externalLinks';

/** Where an app frame opens an external web page from. The frame opens a
 *  `tab` itself wherever `openExternalUrl` would only open a plain browser tab.
 *  It hands off to `safari` itself in an installed iOS PWA set to Safari, which
 *  only the tap can do. The desktop app's web view drops `window.open`, and the
 *  share sheet needs a host, so both stay on the `host` route. */
export function frameOpensExternalLinks(): FrameOpens {
  if (isTauri()) return 'host';
  if (!isIOSPwa()) return 'tab';
  switch (currentExternalLinkTarget()) {
    case 'in-app': return 'tab';
    case 'safari': return 'safari';
    default: return 'host';
  }
}

function answer(): ExternalLinksPush {
  return { frameOpens: frameOpensExternalLinks() };
}

/** Hand one app frame the current answer. Called as the frame loads, since a
 *  push to a document that has not run the SDK yet reaches nobody. */
export function pushExternalLinksToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, EXTERNAL_LINKS_CHANNEL, answer());
}

/** Keep every open app frame's answer current. The link target reads the
 *  preferences signal, so a change in Settings re-runs this. Returns the
 *  teardown. */
export function installAppExternalLinksSync(): () => void {
  return effect(() => {
    const data = answer();
    for (const frame of mountedAppFrames()) {
      postToAppFrame(frame, EXTERNAL_LINKS_CHANNEL, data);
    }
  });
}
