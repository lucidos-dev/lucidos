import { PENDING_TITLE_PLACEHOLDER, type ThreadState } from '../store/thread-events';
import { getDraft } from '../store/composeDrafts';

const MAX_LEN = 40;

export const UNTITLED_THREAD = 'Untitled thread';

export function storedThreadTitle(title: string | null | undefined): string {
    return title?.trim() || UNTITLED_THREAD;
}

function previewText(text: string): string {
    const trimmed = text.trim();
    return trimmed.length > MAX_LEN ? trimmed.slice(0, MAX_LEN - 1) + '…' : trimmed;
}

export function composingTitle(composeText: string): string {
    return previewText(composeText) || 'Empty draft';
}

export function threadDisplayTitle(thread: ThreadState): string {
    if (thread.meta.state === 'composing') {
        return composingTitle(getDraft(thread.meta.id).text);
    }
    if (thread.meta.title && thread.meta.title !== PENDING_TITLE_PLACEHOLDER) {
        const trimmed = thread.meta.title.trim();
        if (trimmed) return trimmed;
    }
    const messageText = firstUserMessageText(thread);
    if (messageText) {
        const preview = previewText(messageText);
        if (preview) return preview;
    }
    return 'Untitled Thread';
}

function firstUserMessageText(thread: ThreadState): string {
    for (const ev of thread.events.values()) {
        if (ev?.type === 'MessageReceived' || ev?.type === 'SpokenMessageReceived') return ev.text;
    }
    if (thread.pendingUserMessages.length > 0) return thread.pendingUserMessages[0].text;
    return '';
}
