export type ColorScheme = "light" | "dark";

const COLOR_SCHEME_STORAGE_KEY = "kimi-code.color-scheme";
const DEFAULT_COLOR_SCHEME: ColorScheme = "dark";

function isColorScheme(value: string | null): value is ColorScheme {
  return value === "light" || value === "dark";
}

export function loadColorScheme(): ColorScheme {
  if (typeof window === "undefined") return DEFAULT_COLOR_SCHEME;
  try {
    const stored = window.localStorage.getItem(COLOR_SCHEME_STORAGE_KEY);
    return isColorScheme(stored) ? stored : DEFAULT_COLOR_SCHEME;
  } catch {
    return DEFAULT_COLOR_SCHEME;
  }
}

export function applyColorScheme(colorScheme: ColorScheme): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.colorScheme = colorScheme;
  document.documentElement.style.colorScheme = colorScheme;

  const themeColor = colorScheme === "light" ? "#ffffff" : "#0d1117";
  document
    .querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')
    .forEach((meta) => meta.setAttribute("content", themeColor));
}

export function saveColorScheme(colorScheme: ColorScheme): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(COLOR_SCHEME_STORAGE_KEY, colorScheme);
  } catch {
    // A disabled or full storage area must not prevent theme switching.
  }
}
