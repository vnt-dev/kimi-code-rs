import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  MOBILE_LAYOUT_MAX_WIDTH,
  MOBILE_LAYOUT_QUERY,
  resolveSidebarCollapsed,
  shouldUseWebMobileLayout,
} from "../responsive";
import { isDesktop } from "../transport";

export function useResponsiveSidebar(
  setProfileOpen: Dispatch<SetStateAction<boolean>>,
) {
  const desktopRuntime = useMemo(isDesktop, []);
  const [mobileQueryMatches, setMobileQueryMatches] = useState(() =>
    typeof window.matchMedia === "function"
      ? window.matchMedia(MOBILE_LAYOUT_QUERY).matches
      : window.innerWidth <= MOBILE_LAYOUT_MAX_WIDTH,
  );
  const mobileLayout = shouldUseWebMobileLayout(
    desktopRuntime,
    mobileQueryMatches,
  );
  const [desktopSidebarCollapsed, setDesktopSidebarCollapsed] = useState(false);
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [mobileViewportHeight, setMobileViewportHeight] = useState<number>();
  const mobileMenuButtonRef = useRef<HTMLButtonElement>(null);

  const sidebarCollapsed = resolveSidebarCollapsed(
    mobileLayout,
    desktopSidebarCollapsed,
    mobileSidebarOpen,
  );

  const closeMobileNavigation = useCallback((): void => {
    if (!mobileLayout) return;
    setMobileSidebarOpen(false);
    setProfileOpen(false);
    window.requestAnimationFrame(() => mobileMenuButtonRef.current?.focus());
  }, [mobileLayout, setProfileOpen]);

  const openSidebar = useCallback((): void => {
    setProfileOpen(false);
    if (mobileLayout) setMobileSidebarOpen(true);
    else setDesktopSidebarCollapsed(false);
  }, [mobileLayout, setProfileOpen]);

  const toggleSidebar = useCallback((): void => {
    setProfileOpen(false);
    if (mobileLayout) {
      if (mobileSidebarOpen) closeMobileNavigation();
      else setMobileSidebarOpen(true);
    } else {
      setDesktopSidebarCollapsed((collapsed) => !collapsed);
    }
  }, [closeMobileNavigation, mobileLayout, mobileSidebarOpen, setProfileOpen]);

  const expandDesktopSidebar = useCallback((): void => {
    setDesktopSidebarCollapsed(false);
  }, []);

  useEffect(() => {
    if (desktopRuntime || typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(MOBILE_LAYOUT_QUERY);
    const sync = (): void => setMobileQueryMatches(query.matches);
    sync();
    query.addEventListener("change", sync);
    return () => query.removeEventListener("change", sync);
  }, [desktopRuntime]);

  useEffect(() => {
    setMobileSidebarOpen(false);
    setProfileOpen(false);
  }, [mobileLayout, setProfileOpen]);

  useEffect(() => {
    if (!mobileLayout) {
      setMobileViewportHeight(undefined);
      return;
    }
    const viewport = window.visualViewport;
    const sync = (): void => {
      setMobileViewportHeight(
        Math.round(viewport?.height ?? window.innerHeight),
      );
    };
    sync();
    window.addEventListener("resize", sync);
    viewport?.addEventListener("resize", sync);
    return () => {
      window.removeEventListener("resize", sync);
      viewport?.removeEventListener("resize", sync);
    };
  }, [mobileLayout]);

  useEffect(() => {
    if (!mobileLayout || !mobileSidebarOpen) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      closeMobileNavigation();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [closeMobileNavigation, mobileLayout, mobileSidebarOpen]);

  return {
    desktopRuntime,
    mobileLayout,
    mobileSidebarOpen,
    mobileViewportHeight,
    sidebarCollapsed,
    mobileMenuButtonRef,
    closeMobileNavigation,
    openSidebar,
    toggleSidebar,
    expandDesktopSidebar,
  };
}
