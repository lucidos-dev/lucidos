export interface StripEnds {
  atStart: boolean;
  atEnd: boolean;
}

export function stripEnds(el: Pick<HTMLElement, 'scrollLeft' | 'clientWidth' | 'scrollWidth'>): StripEnds {
  return { atStart: el.scrollLeft <= 1, atEnd: el.scrollLeft + el.clientWidth >= el.scrollWidth - 1 };
}

export const sameStripEnds = (a: StripEnds, b: StripEnds) => a.atStart === b.atStart && a.atEnd === b.atEnd;
