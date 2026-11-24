const exactAcceptTexts = [
    'accept all', 'accept all cookies', 'allow all', 'allow all cookies',
    'allow essential and optional cookies', 'i agree', 'agree to all',
    'godta alle', 'godta alle cookies', 'aksepter alle', 'tillat alle',
    'jeg godtar', 'jeg aksepterer', 'godkjenn alle', 'ja, jeg samtykker',
    'acceptera alla', 'godkänn alla',
    'accepter alle', 'tillad alle',
    'alle akzeptieren', 'alle cookies akzeptieren',
    'tout accepter', 'accepter tout',
    'aceptar todo', 'aceptar todas',
    'alles accepteren',
    'accetta tutto', 'accetta tutti'
];

const rejectTexts = [
    'choose', 'customize', 'settings', 'preferences', 'manage', 'options',
    'reject', 'decline', 'deny', 'refuse', 'only essential', 'only necessary',
    'learn more', 'more info', 'read more', 'how we use', 'about', 'details',
    'privacy policy', 'cookie policy', 'purpose', 'partner', 'vendor',
    'category', 'categories',
    'velg', 'innstillinger', 'avvis', 'kun nødvendige', 'les mer',
    'ablehnen', 'einstellungen',
    'refuser', 'paramètres'
];

const buttons = document.querySelectorAll('button, [role="button"], a.button, input[type="button"], input[type="submit"]');
let fallbackButton = null;

for (const btn of buttons) {
    const text = (btn.innerText || btn.textContent || btn.value || '').toLowerCase().trim();
    const rect = btn.getBoundingClientRect();

    if (rect.width < 50 || rect.height < 20) continue;

    if (rect.top < 0 || rect.top > 800) continue;

    if (text.length > 40) continue;

    if (rejectTexts.some(t => text.includes(t))) continue;

    const normalizedText = text.replace(/[^a-z0-9æøåäöü\s]/g, '').trim();
    const isExact = exactAcceptTexts.some(t => text === t || normalizedText === t);
    const keyPhrases = ['godta alle', 'accept all', 'allow all', 'aksepter alle', 'tillat alle'];
    const isContains = text.length < 30 && keyPhrases.some(p => text.includes(p) || normalizedText.includes(p));
    const isFallback = !fallbackButton && ['accept', 'godta', 'accepter', 'aksepter'].includes(text);
    if (!(isExact || isContains || isFallback) || !hasConsentContext(btn)) continue;

    if (isExact || isContains) {
        console.log('[Lucidos] Clicking ' + (isExact ? 'exact' : 'contains') + ' match: ' + text);
        btn.click();
        return 'clicked: ' + text;
    }

    fallbackButton = { btn, text };
}

if (fallbackButton) {
    console.log('[Lucidos] Clicking fallback: ' + fallbackButton.text);
    fallbackButton.btn.click();
    return 'clicked fallback: ' + fallbackButton.text;
}

return 'not found. Buttons: ' + buttons.length;
