import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { currentExternalLinkTarget } from './preferences';
import { mountedAppFrames } from '../../utils/appFrame';
import { isIOSPwa, isTauri } from '../../utils/platform';
import {
  EXTERNAL_LINKS_CHANNEL,
  type ExternalLinksPush,
  type FrameOpens,
} from '../../../../../packages/lucidos-sdk/src/externalLinks';

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

export function pushExternalLinksToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, EXTERNAL_LINKS_CHANNEL, answer());
}

export function installAppExternalLinksSync(): () => void {
  return effect(() => {
    const data = answer();
    for (const frame of mountedAppFrames()) {
      postToAppFrame(frame, EXTERNAL_LINKS_CHANNEL, data);
    }
  });
}
