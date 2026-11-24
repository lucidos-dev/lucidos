import { requestGenerationGuard } from './_core';
import { restoreAdmissionReady, waitForRestoreAdmission } from '../../utils/restoreAdmission';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { registrationToAwait } from '../../utils/deviceRegistration';

export const UPLOAD_STALL_MS = 20_000;
export const UPLOAD_RESPONSE_MS = 30_000;

export const UPLOAD_STALLED_MESSAGE = 'The upload stopped making progress';
export const UPLOAD_UNANSWERED_MESSAGE = 'Lucidos did not answer the upload in time';

export interface UploadProgress {
  sentBytes: number;
  totalBytes: number;
}

export interface UploadObserver {
  onProgress?: (progress: UploadProgress) => void;
  onBodySent?: () => void;
  signal?: AbortSignal;
}

export async function postWithUploadProgress(
  url: string,
  body: FormData,
  observer: UploadObserver = {},
): Promise<Response> {
  const checkGeneration = requestGenerationGuard(url);
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  const registration = registrationToAwait(url, 'POST');
  if (registration) await registration;
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  checkGeneration();
  observer.signal?.throwIfAborted();

  const response = await new Promise<Response>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    let deadline: ReturnType<typeof setTimeout> | undefined;
    let settled = false;

    const settle = (finish: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      observer.signal?.removeEventListener('abort', onCallerAbort);
      finish();
    };
    const arm = (ms: number, message: string) => {
      if (settled) return;
      clearTimeout(deadline);
      deadline = setTimeout(() => {
        settle(() => reject(new DOMException(message, 'TimeoutError')));
        xhr.abort();
      }, ms);
    };
    const armStall = () => arm(UPLOAD_STALL_MS, UPLOAD_STALLED_MESSAGE);
    function onCallerAbort() {
      settle(() => reject(new DOMException('Upload cancelled', 'AbortError')));
      xhr.abort();
    }

    xhr.open('POST', url);
    for (const [name, value] of Object.entries({ ...deviceIdHeader(), ...checkGeneration.headers })) xhr.setRequestHeader(name, value);
    xhr.upload.onprogress = (e) => {
      armStall();
      if (e.lengthComputable) observer.onProgress?.({ sentBytes: e.loaded, totalBytes: e.total });
    };
    xhr.upload.onload = () => {
      arm(UPLOAD_RESPONSE_MS, UPLOAD_UNANSWERED_MESSAGE);
      observer.onBodySent?.();
    };
    xhr.onload = () => settle(() => {
      if (xhr.status === 0) reject(new TypeError('Failed to fetch'));
      else resolve(responseFromXhr(xhr));
    });
    xhr.onerror = () => settle(() => reject(new TypeError('Failed to fetch')));
    xhr.onabort = () => settle(() => reject(new DOMException('Upload cancelled', 'AbortError')));
    observer.signal?.addEventListener('abort', onCallerAbort, { once: true });

    armStall();
    xhr.send(body);
  });
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  checkGeneration();
  return response;
}

const NULL_BODY_STATUSES = new Set([204, 205, 304]);

function responseFromXhr(xhr: XMLHttpRequest): Response {
  const headers = new Headers();
  for (const line of xhr.getAllResponseHeaders().split(/\r?\n/)) {
    const colon = line.indexOf(':');
    if (colon <= 0) continue;
    headers.append(line.slice(0, colon).trim(), line.slice(colon + 1).trim());
  }
  const text = NULL_BODY_STATUSES.has(xhr.status) ? null : xhr.responseText;
  return new Response(text, { status: xhr.status, statusText: xhr.statusText, headers });
}
