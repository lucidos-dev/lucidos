export function scopeRemoteContent(node: Element, basePath: string, origin: string): void {
  for (const name of ['style', 'filter', 'fill', 'stroke', 'mask', 'clip-path', 'cursor', 'marker-start', 'marker-mid', 'marker-end']) {
    const value = node.getAttribute(name);
    if (value && /[\\@]|\/\*|url\s*\(|image-set\s*\(|expression\s*\(/i.test(value)) {
      node.removeAttribute(name);
    }
  }
  node.removeAttribute('srcset');
  node.removeAttribute('ping');
  for (const attribute of ['src', 'href', 'xlink:href', 'poster', 'background', 'data']) {
    if (attribute === 'href' && node.localName === 'a') continue;
    const value = node.getAttribute(attribute);
    if (value === null || value.startsWith('#')) continue;
    try {
      const url = new URL(value, `${origin}${basePath}/`);
      if (url.origin !== origin && (url.protocol === 'https:' || url.protocol === 'http:')) continue;
      if (url.origin === origin && ['api/v1/', 'data/', '~cap/', 'app/'].some((path) => url.pathname.startsWith(`${basePath}/${path}`))) continue;
    } catch {  }
    node.removeAttribute(attribute);
  }
}
