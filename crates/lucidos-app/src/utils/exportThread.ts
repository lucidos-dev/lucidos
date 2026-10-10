import { fetchThreadEvents } from '../api/threads';
import { workspaceName, showToast } from '../store/store';
import { openLocalFile } from '../store/actions/artifacts';
import { errorDetail, isAbortError } from './errorDetail';
import { isIOS, isTauri } from './platform';
import { saveToDownloads } from './tauri';

// 60 chars keeps "thread-<8>-<slug>.json" comfortably under the 255-byte
// filesystem name limit on Windows / macOS / Linux.
function titleSlug(title: string): string {
    const slug = (title || 'untitled')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
        .slice(0, 60);
    return slug || 'untitled';
}

function shortId(threadId: string): string {
    return threadId.replace(/-/g, '').slice(0, 8);
}

function triggerDownload(blob: Blob, filename: string): void {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    // Defer revoke so Safari / iOS WebKit can start the download — they need
    // the object URL to remain live for at least one tick after click().
    setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** The folder's own name, for a toast that says where the file went. Both
 *  separators split, so a Windows path reads like a POSIX one. A path with no
 *  segment to take is returned whole. */
function folderLabel(dir: string): string {
    const segments = dir.split(/[/\\]/).filter(Boolean);
    return segments[segments.length - 1] ?? dir;
}

/** Can this browser hand `file` to the OS share sheet? Tested against the
 *  actual file rather than the API's presence: iOS answers no for a payload it
 *  will not take, and a `Share` button that cannot share is worse than none. */
function canShareFile(file: File): boolean {
    return typeof navigator.canShare === 'function' && navigator.canShare({ files: [file] });
}

/** Hand the export to the OS share sheet, which on iOS is the route to sending
 *  it and to Save to Files.
 *
 *  Called straight from the toast button's click, never after an await:
 *  `navigator.share` needs transient user activation, and awaiting first spends
 *  it. A dismissed sheet rejects with AbortError, which is the user's decision
 *  rather than a failure, so it says nothing. Every other rejection toasts. */
function shareFile(file: File): void {
    void navigator.share({ files: [file], title: file.name }).catch((err: unknown) => {
        if (isAbortError(err)) return;
        showToast(`Couldn't share ${file.name}: ${errorDetail(err)}`, 'error');
    });
}

/** Wraps the events with workspace + title + exported_at so a JSON file
 *  attached to a bug report carries enough context to identify the thread
 *  without the report author having to type it out.
 *
 *  Passes `includeContext: true` so the dump retains `ContextCaptured.sections`
 *  + `tools` — the snapshot endpoint strips these by default for the modal's
 *  lazy-fetch path, but an export that drops the prompt sections defeats the
 *  point of a bug-report attachment.
 *
 *  **The desktop client saves through Rust, not through the webview.** wry
 *  attaches a download delegate only when the app registers a download handler,
 *  and it registers none, so an `<a download>` click there is silently dropped.
 *  `saveToDownloads` both makes the file exist and reports the folder, which is
 *  what the toast opens. A browser keeps the blob download, and its toast can
 *  only NAME the folder: no web API opens one.
 *
 *  **iOS gets the share sheet INSTEAD of the download, not alongside it.**
 *  WebKit honours no `<a download>`, so the anchor opened the JSON in a viewer
 *  and took the PWA off the thread. A toast then named a downloads folder the
 *  file had never reached. The share sheet is the real route there, and it
 *  carries Save to Files. A sheet that refuses the file leaves the anchor as the
 *  only route left, so that case keeps it. */
export async function exportThread(threadId: string, title: string): Promise<void> {
    const filename = `thread-${shortId(threadId)}-${titleSlug(title)}.json`;
    try {
        const snapshot = await fetchThreadEvents(threadId, { includeContext: true });
        const envelope = {
            exported_at: new Date().toISOString(),
            workspace: workspaceName.value || 'unknown',
            thread_id: threadId,
            title: title || 'Untitled Thread',
            events: snapshot.events,
            current_aggregate: snapshot.currentAggregate,
        };
        const json = JSON.stringify(envelope, null, 2);
        if (isTauri()) {
            const saved = await saveToDownloads(filename, json);
            showToast(`Thread exported to ${folderLabel(saved.dir)}`, 'success', {
                action: { label: 'Open folder', onClick: () => openLocalFile(saved.dir) },
            });
            return;
        }
        const blob = new Blob([json], { type: 'application/json' });
        // Captured now, so the button's click can share without awaiting first.
        const file = new File([blob], filename, { type: blob.type });
        const shareable = canShareFile(file);
        const shareOnly = isIOS() && shareable;
        if (!shareOnly) triggerDownload(blob, filename);
        const message = shareOnly
            ? 'Thread export ready'
            : 'Thread exported to your downloads folder';
        showToast(message, 'success', {
            action: shareable ? { label: 'Share', onClick: () => shareFile(file) } : undefined,
        });
    } catch (err) {
        showToast(`Failed to export thread: ${errorDetail(err)}`, 'error');
    }
}
