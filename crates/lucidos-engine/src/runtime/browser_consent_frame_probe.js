// Key phrases to look for (must contain one of these)
const acceptPhrases = [
    'godta alle', 'aksepter alle', 'tillat alle',
    'accept all', 'allow all', 'i agree',
    'acceptera alla', 'accepter alle',
    'alle akzeptieren', 'tout accepter',
    'aceptar todo', 'alles accepteren', 'accetta tutto'
];

// Reject if contains these
const rejectPhrases = [
    'settings', 'preferences', 'manage', 'reject', 'decline',
    'learn more', 'read more', 'how we use', 'privacy policy',
    'innstillinger', 'avvis', 'les mer', 'velg'
];

// Search all elements for accept text
const allElements = document.querySelectorAll('button, [role="button"], a, span, div, p');
let bestMatch = null;

for (const el of allElements) {
    const text = (el.innerText || el.textContent || '').toLowerCase().trim();

    // Skip if empty or too long
    if (!text || text.length > 50) continue;

    // Skip reject patterns
    if (rejectPhrases.some(p => text.includes(p))) continue;

    // Check for accept phrase
    const hasAccept = acceptPhrases.some(p => text.includes(p));
    if (!hasAccept) continue;

    const rect = el.getBoundingClientRect();

    // Must be visible
    if (rect.width < 20 || rect.height < 10) continue;
    if (rect.top < 0 || rect.top > 1400 || rect.left < 0) continue;

    // In every frame, a CMP's own included. A frame's URL proves nothing.
    if (!hasConsentContext(el)) continue;

    // Prefer shorter text (more specific match)
    if (!bestMatch || text.length < bestMatch.textLen) {
        bestMatch = {
            x: rect.left + rect.width / 2,
            y: rect.top + rect.height / 2,
            text: text.substring(0, 50),
            textLen: text.length
        };
    }
}

if (bestMatch) {
    return JSON.stringify({
        x: bestMatch.x + (window.screenX || 0),
        y: bestMatch.y + (window.screenY || 0),
        clientX: bestMatch.x,
        clientY: bestMatch.y,
        text: bestMatch.text
    });
}

// Debug: what is in this frame. Both fields below go straight into
// an engine log line, which is persisted.
//
// origin + pathname, never location.href. A frame's query string
// and fragment can carry an OAuth `code`, `access_token` or
// `id_token`.
//
// The body TEXT is not reported, for the same reason. A consent,
// callback or 2FA page shows a one-time passcode or the account
// email in its visible body. The button count is the diagnostic.
const buttonCount = document.querySelectorAll('button').length;
return JSON.stringify({
    debug: true,
    buttonCount: buttonCount,
    url: location.origin + location.pathname
});
