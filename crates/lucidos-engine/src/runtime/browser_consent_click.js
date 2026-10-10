// Only exact matches for accept-all type buttons
const exactAcceptTexts = [
    // English - specific "accept all" variants
    'accept all', 'accept all cookies', 'allow all', 'allow all cookies',
    'allow essential and optional cookies', 'i agree', 'agree to all',
    // Norwegian
    'godta alle', 'godta alle cookies', 'aksepter alle', 'tillat alle',
    'jeg godtar', 'jeg aksepterer', 'godkjenn alle', 'ja, jeg samtykker',
    // Swedish
    'acceptera alla', 'godkänn alla',
    // Danish
    'accepter alle', 'tillad alle',
    // German
    'alle akzeptieren', 'alle cookies akzeptieren',
    // French
    'tout accepter', 'accepter tout',
    // Spanish
    'aceptar todo', 'aceptar todas',
    // Dutch
    'alles accepteren',
    // Italian
    'accetta tutto', 'accetta tutti'
];

// Reject texts - never click these (be liberal)
const rejectTexts = [
    'choose', 'customize', 'settings', 'preferences', 'manage', 'options',
    'reject', 'decline', 'deny', 'refuse', 'only essential', 'only necessary',
    'learn more', 'more info', 'read more', 'how we use', 'about', 'details',
    'privacy policy', 'cookie policy', 'purpose', 'partner', 'vendor',
    'category', 'categories',
    'velg', 'innstillinger', 'avvis', 'kun nødvendige', 'les mer', // Norwegian
    'ablehnen', 'einstellungen', // German
    'refuser', 'paramètres' // French
];

// Find and click accept button
const buttons = document.querySelectorAll('button, [role="button"], a.button, input[type="button"], input[type="submit"]');
let fallbackButton = null;

for (const btn of buttons) {
    const text = (btn.innerText || btn.textContent || btn.value || '').toLowerCase().trim();
    const rect = btn.getBoundingClientRect();

    // Size checks
    if (rect.width < 50 || rect.height < 20) continue;

    // Must be reasonably visible
    if (rect.top < 0 || rect.top > 800) continue;

    // Consent buttons are SHORT - reject long text
    if (text.length > 40) continue;

    // Skip if contains reject text
    if (rejectTexts.some(t => text.includes(t))) continue;

    // Match accept phrases - exact match preferred
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

// Use fallback if no better match found
if (fallbackButton) {
    console.log('[Lucidos] Clicking fallback: ' + fallbackButton.text);
    fallbackButton.btn.click();
    return 'clicked fallback: ' + fallbackButton.text;
}

// The engine logs this result, so it names no page text: a logged-in
// page shows the account's email and name on its buttons.
return 'not found. Buttons: ' + buttons.length;
