
export const PROVIDER_SCOPES: Record<string, string> = {
  google_drive: 'https://www.googleapis.com/auth/drive.file',
  dropbox: 'files.content.write files.content.read files.metadata.read account_info.read',
};

export const PROVIDER_ACCESS: Record<string, string> = {
  google_drive:
    'Lucidos asks for one Drive permission, drive.file. With it, Lucidos can see and change only the files it created. ' +
    'This permission does not let it read, list or delete anything else in your Drive. ' +
    'Backups are encrypted before upload, and the key is never uploaded. ' +
    'You can revoke access any time in your Google Account, under Third-party apps & services.',
  dropbox:
    'Lucidos asks for four Dropbox permissions: files.content.write, files.content.read, files.metadata.read and account_info.read. ' +
    'With them it can write, read, list and delete files, and see which account is connected. ' +
    'Your Dropbox app decides how far that reaches. ' +
    'With App folder access, it is only that app\'s own folder. With Full Dropbox access, it is your whole Dropbox. ' +
    'Backups use only the Lucidos Backups folder, and delete only older backups of this workspace past the Keep count. ' +
    'Backups are encrypted before upload, and the key is never uploaded. ' +
    'You can revoke access any time in your Dropbox settings, under Connected apps.',
};

export function oauthProviderFor(backupProviderId: string): string {
  return backupProviderId === 'google_drive' ? 'google' : backupProviderId;
}

export function backupAccessLine(providerName: string, missing: string[] | undefined): string {
  if (!missing?.length) return `${providerName} access not granted.`;
  const permissions = missing.length === 1 ? 'permission' : 'permissions';
  return `${providerName} is missing the ${missing.join(', ')} ${permissions}.`;
}

export function pickInitialProvider(
  configured: string | null | undefined,
  available: { id: string }[],
): string {
  if (configured && available.some((p) => p.id === configured)) return configured;
  return available[0]?.id ?? '';
}
