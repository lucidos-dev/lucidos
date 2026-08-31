import { PENDING_TITLE_PLACEHOLDER, type ThreadState } from '../store/thread-events';
import { getDraft } from '../store/composeDrafts';

const MAX_LEN = 40;

function previewText(text: string): string {
    const trimmed = text.trim();
    return trimmed.length > MAX_LEN ? trimmed.slice(0, MAX_LEN - 1) + '…' : trimmed;
}

/** Preview title for a composing/draft thread, derived from its compose text.
 *  Internal helper for `threadDisplayTitle`; kept exported for unit tests. */
export function composingTitle(composeText: string): string {
    return previewText(composeText) || 'Empty draft';
}

/** Single source of truth for the title the user sees, in both the drawer
 *  row and the chat header. The fallback chain prevents the header from
 *  flashing "Untitled Thread" between Send and `ThreadTitleGenerated`:
 *    1. composing → live composeText preview (drawer + header agree as the
 *       user types).
 *    2. real generated/renamed title → use it.
 *    3. no title yet → preview of the first user message (pending optimistic
 *       message before SSE; first `MessageReceived` event after SSE catches
 *       up). This is what the LLM is about to title anyway.
 *    4. nothing to derive from → "Untitled Thread" final fallback.
 *  Both surfaces MUST call this so they cannot drift. */
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
    // Map iteration is insertion-ordered; events are inserted by handleEvent
    // in seq order during DB load and SSE arrival, so the first MessageReceived
    // we hit is the one with the lowest seq. Out-of-order SSE on reconnect can
    // theoretically swap, but only AFTER a generated title arrived (which
    // short-circuits this path) — in practice we never run when that matters.
    //
    // `SpokenMessageReceived` counts too. It is the caller's own words on a
    // call the talker answered alone, and such a call writes no
    // `MessageReceived` at all. The engine's `format_display_title` takes the
    // same row from `first_message`, so the two surfaces agree.
    for (const ev of thread.events.values()) {
        if (ev?.type === 'MessageReceived' || ev?.type === 'SpokenMessageReceived') return ev.text;
    }
    if (thread.pendingUserMessages.length > 0) return thread.pendingUserMessages[0].text;
    return '';
}
