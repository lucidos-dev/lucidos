function hasConsentContext(el) {
    const textWords = ['cookie', 'informasjonskapsl', 'kakor'];
    const attributeWords = textWords.concat(['consent', 'samtykke', 'gdpr']);
    let node = el;
    for (let i = 0; i < 6 && node && node !== document.body; i++) {
        const t = (node.innerText || '').toLowerCase().substring(0, 500);
        const id = (node.id || '').toLowerCase();
        const cls = (node.getAttribute('class') || '').toLowerCase();
        if (textWords.some(w => t.includes(w))) return true;
        if (attributeWords.some(w => id.includes(w) || cls.includes(w))) return true;
        node = node.parentElement;
    }
    return false;
}
