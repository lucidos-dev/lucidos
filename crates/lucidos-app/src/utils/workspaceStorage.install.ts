import { peerClient } from '../../../../packages/lucidos-sdk/src/_peerScope';
import { WORKSPACE_ID } from './basePath';
import { installWorkspaceStorage } from './workspaceStorage';

if (typeof localStorage !== 'undefined') {
  installWorkspaceStorage(localStorage, WORKSPACE_ID, peerClient());
}
