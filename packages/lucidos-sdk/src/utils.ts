const ATTR_ENTITIES: Record<string, string> = {
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
};

export const utils = {
  timeAgo(iso: string): string {
    const s = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
    if (s < 60) return 'just now';
    const m = Math.floor(s / 60); if (m < 60) return `${m}m ago`;
    const h = Math.floor(m / 60); if (h < 24) return `${h}h ago`;
    const d = Math.floor(h / 24); if (d < 30) return `${d}d ago`;
    return new Date(iso).toLocaleDateString();
  },

  escapeHtml(str: string): string {
    const d = document.createElement('div');
    d.textContent = str;
    return d.innerHTML;
  },

  escapeHtmlAttr(str: string): string {
    return String(str ?? '').replace(/[&<>"']/g, (c) => ATTR_ENTITIES[c]);
  },

  formatDate(iso: string): string {
    return new Date(iso).toLocaleString();
  },
};
