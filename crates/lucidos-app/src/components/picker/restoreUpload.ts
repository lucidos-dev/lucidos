import { discardRestoreUpload, remoteControlBase, restoreBackup, type RestoreRequest, type RestoreStarted } from '../../api/client/control';
import { RETRY_DELAYS_MS } from '../../api/client/resumableUpload';
import { uploadErrorMessage } from '../../api/client/uploadProgress';
import { hopRefusalReason } from './remoteControl';
import { IDLE_RESTORE_UPLOAD, type RestoreUpload } from './workspaceForms';

export interface RestoreTarget {
  controlBase?: string;
  describe: (err: unknown) => string;
}

const LOCAL_RESTORE_TARGET: RestoreTarget = { describe: uploadErrorMessage };

export function restoreTargetFor(gateway: string | null): RestoreTarget {
  if (gateway === null) return LOCAL_RESTORE_TARGET;
  return {
    controlBase: remoteControlBase(gateway),
    describe: (err) => hopRefusalReason(err, gateway) ?? uploadErrorMessage(err),
  };
}

export async function uploadRestore(
  request: RestoreRequest,
  signal: AbortSignal,
  show: (upload: RestoreUpload) => void,
  target: RestoreTarget = LOCAL_RESTORE_TARGET,
): Promise<RestoreStarted | null> {
  let sent = { sentBytes: 0, totalBytes: request.file.size };
  show({ kind: 'uploading', ...sent });
  try {
    return await restoreBackup(request, {
      signal,
      onProgress: (progress) => {
        sent = progress;
        show({ kind: 'uploading', ...progress });
      },
      onRetry: (attempt, reason) => show({
        kind: 'retrying',
        attempt,
        maxAttempts: RETRY_DELAYS_MS.length,
        reason: uploadErrorMessage(reason),
        ...sent,
      }),
      onUploaded: () => show({ kind: 'starting' }),
    }, target.controlBase);
  } catch (err) {
    if (!signal.aborted) throw new Error(target.describe(err));
  } finally {
    show(IDLE_RESTORE_UPLOAD);
  }
  try {
    await discardRestoreUpload(request.file, target.controlBase);
  } catch (err) {
    throw new Error(`The cancelled upload stays on the gateway for up to a day: ${target.describe(err)}`);
  }
  return null;
}
