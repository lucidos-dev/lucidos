import { releaseCheck } from '../store/store';
import { withUpdateChannel } from './updateChannel';
import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { formatBuildId } from './buildId';

export function clientVersionLabel(): string {
  const packaged = typeof window !== 'undefined' ? window.__LUCIDOS_APP_VERSION__ : undefined;
  if (packaged) return withUpdateChannel(packaged, releaseCheck.value?.channel);
  return withUpdateChannel(formatBuildId(CLIENT_BUILD_ID), releaseCheck.value?.channel);
}
