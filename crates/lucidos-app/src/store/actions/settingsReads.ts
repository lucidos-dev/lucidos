import { chatModels, credentials, oauthAccounts, repositories } from '../store';
import { devices, loadDevices } from './devices';
import { loadOAuthAccounts } from './oauth';
import { loadCredentials } from './credentials';
import { loadChatModels } from './models';
import { loadRepositories } from './chat';

export function warmSettingsReads(): void {
  if (devices.value.status === 'not-loaded') void loadDevices();
  if (oauthAccounts.value.status === 'not-loaded') void loadOAuthAccounts();
  if (credentials.value.status === 'not-loaded') void loadCredentials();
  if (chatModels.value.status === 'not-loaded') void loadChatModels();
  if (repositories.value.status === 'not-loaded') void loadRepositories();
}
