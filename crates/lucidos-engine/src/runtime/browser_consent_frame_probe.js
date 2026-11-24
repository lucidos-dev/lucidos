const acceptPhrases = [
    'godta alle', 'aksepter alle', 'tillat alle',
    'accept all', 'allow all', 'i agree',
    'acceptera alla', 'accepter alle',
    'alle akzeptieren', 'tout accepter',
    'aceptar todo', 'alles accepteren', 'accetta tutto'
];

const rejectPhrases = [
    'settings', 'preferences', 'manage', 'reject', 'decline',
    'learn more', 'read more', 'how we use', 'privacy policy',
    'innstillinger', 'avvis', 'les mer', 'velg'
];

const allElements = document.querySelectorAll('button, [role="button"], a, span, div, p');
let bestMatch = null;

for (const el of allElements) {
    const text = (el.innerText || el.textContent || '').toLowerCase().trim();

    if (!text || text.length > 50) continue;

    if (rejectPhrases.some(p => text.includes(p))) continue;

    const hasAccept = acceptPhrases.some(p => text.includes(p));
    if (!hasAccept) continue;

    const rect = el.getBoundingClientRect();

    if (rect.width < 20 || rect.height < 10) continue;
    if (rect.top < 0 || rect.top > 1400 || rect.left < 0) continue;

    if (!hasConsentContext(el)) continue;

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

const buttonCount = document.querySelectorAll('button').length;
return JSON.stringify({
    debug: true,
    buttonCount: buttonCount,
    url: location.origin + location.pathname
});
