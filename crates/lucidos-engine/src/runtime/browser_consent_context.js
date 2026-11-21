// True when the element or a near ancestor reads as a cookie or consent
// dialog. Every consent click pass must check it. The browser runs under the
// user's logged-in profile, so any other "Accept" button is clicked as the user.
//
// The words are narrow on purpose. "privacy" sits in every terms dialog's
// text, and "tracking" in Tailwind's letter-spacing classes.
//
// The walk stops below <body>. The body holds the whole page, so a cookie
// word anywhere on it would pass every button.
function hasConsentContext(el) {
    const textWords = ['cookie', 'informasjonskapsl', 'kakor'];
    const attributeWords = textWords.concat(['consent', 'samtykke', 'gdpr']);
    let node = el;
    for (let i = 0; i < 6 && node && node !== document.body; i++) {
        const t = (node.innerText || '').toLowerCase().substring(0, 500);
        const id = (node.id || '').toLowerCase();
        // Not `className`: on an SVG element it is an object, not a string.
        const cls = (node.getAttribute('class') || '').toLowerCase();
        if (textWords.some(w => t.includes(w))) return true;
        if (attributeWords.some(w => id.includes(w) || cls.includes(w))) return true;
        node = node.parentElement;
    }
    return false;
}
