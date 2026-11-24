import { formatBytes } from '../utils/formatBytes';
import type { AppUpdateRunning } from '../utils/tauri';
import type { ProgressDialogState } from './types';


export interface AppUpdateNarration {
  message: string;
  progress: number | null;
  cancellable: boolean;
}

function updateLabel(version: string | null): string {
  return version ? `Lucidos ${version}` : 'the update';
}

export function appUpdateNarration(frame: AppUpdateRunning): AppUpdateNarration {
  const name = updateLabel(frame.version);
  switch (frame.phase) {
    case 'checking':
      return { message: 'Checking for updates…', progress: null, cancellable: true };
    case 'downloading': {
      const { downloaded, total } = frame;
      const sized = total !== null && total > 0;
      return {
        message: `Downloading ${name}: ${formatBytes(downloaded)}${sized ? ` of ${formatBytes(total)}` : ''}`,
        progress: sized ? Math.min(1, downloaded / total) : null,
        cancellable: true,
      };
    }
    case 'verifying':
      return { message: `Verifying ${name}…`, progress: 1, cancellable: false };
    case 'installing':
      return { message: `Installing ${name}…`, progress: null, cancellable: false };
    case 'restarting-services':
      return { message: 'Restarting background services…', progress: null, cancellable: false };
    case 'relaunching':
      return { message: 'Relaunching Lucidos…', progress: null, cancellable: false };
  }
}

export function restartTakesPageDown(opts: { servedByGateway: boolean; restartsGateway: boolean }): boolean {
  return !opts.servedByGateway || opts.restartsGateway;
}

const RESTART_DIALOG_MESSAGE ='The workspace is unavailable until the engine comes back.';

export function restartDialogState(newVersion: boolean): ProgressDialogState {
  return {
    visible: true,
    title: newVersion ? 'Starting new version' : 'Restarting engine',
    message: RESTART_DIALOG_MESSAGE,
    progress: null,
  };
}

export function appUpdateDialogState(
  frame: AppUpdateRunning,
  onCancel: (() => void) | null,
): ProgressDialogState {
  const narration = appUpdateNarration(frame);
  return {
    visible: true,
    title: 'Updating Lucidos',
    message: narration.message,
    progress: narration.progress,
    cancel: narration.cancellable && onCancel ? { label: 'Cancel', onClick: onCancel } : undefined,
  };
}
