/** Mobile breakpoint in px — matches @media (max-width: 768px) in CSS. */
export const MOBILE_BREAKPOINT = 768;

/** True when the viewport is at or below the mobile breakpoint. */
export const isMobile = () => window.innerWidth <= MOBILE_BREAKPOINT;
