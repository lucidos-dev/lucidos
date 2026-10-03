import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { registrationToAwait } from '../../utils/deviceRegistration';

/** A body that makes no progress for this long has stalled. Progress resets
 *  it, so a large photo on a slow link is never cut off for being large. */
export const UPLOAD_STALL_MS = 20_000;
/** How long the engine has to answer once the whole body has left. */
export const UPLOAD_RESPONSE_MS = 30_000;

/** The `TimeoutError` messages for the two deadlines. Written for the user:
 *  the upload pipeline shows them as the failure reason. */
export const UPLOAD_STALLED_MESSAGE = 'The upload stopped making progress';
export const UPLOAD_UNANSWERED_MESSAGE = 'Lucidos did not answer the upload in time';

export interface UploadProgress {
  sentBytes: number;
  totalBytes: number;
}

export interface UploadObserver {
  onProgress?: (progress: UploadProgress) => void;
  /** The browser's network stack holds the whole body. WebKit says so before
   *  a small body has travelled, so this is not delivery. */
  onBodySent?: () => void;
  signal?: AbortSignal;
}

/** POST a form and report upload progress. XHR rather than `fetch`: it is the
 *  one API that reports upload progress on every browser Lucidos ships to,
 *  Safari included, and it can be aborted mid-body.
 *
 *  Resolves with a `Response`, so callers keep `throwIfNotOk` as the one place
 *  a body becomes a reason. Rejections match what `fetch` throws, so
 *  `isTransientFetchError` classifies them the same way: a dropped connection
 *  is a transport `TypeError`, a missed deadline a `TimeoutError`, a caller's
 *  cancel an `AbortError`. */
export async function postWithUploadProgress(
  url: string,
  body: FormData,
  observer: UploadObserver = {},
): Promise<Response> {
  const registration = registrationToAwait(url, 'POST');
  if (registration) await registration;
  observer.signal?.throwIfAborted();

  return new Promise<Response>((resolve, reject) => {
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
    // Settle first: `abort()` fires `onabort` synchronously, and the rejection
    // must name why we aborted rather than read as a plain cancel.
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
    for (const [name, value] of Object.entries(deviceIdHeader())) xhr.setRequestHeader(name, value);
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
}

/** Statuses the `Response` constructor refuses a body for. */
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
