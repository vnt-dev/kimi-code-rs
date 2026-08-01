export const MOBILE_LAYOUT_MAX_WIDTH = 760;
export const MOBILE_LAYOUT_QUERY = `(max-width: ${MOBILE_LAYOUT_MAX_WIDTH}px)`;

export function shouldUseWebMobileLayout(
  desktopRuntime: boolean,
  mobileQueryMatches: boolean,
): boolean {
  return !desktopRuntime && mobileQueryMatches;
}

export function resolveSidebarCollapsed(
  mobileLayout: boolean,
  desktopCollapsed: boolean,
  mobileSidebarOpen: boolean,
): boolean {
  return mobileLayout ? !mobileSidebarOpen : desktopCollapsed;
}
