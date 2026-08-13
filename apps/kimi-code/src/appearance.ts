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

export type FontSize = "12" | "13" | "14" | "15" | "16" | "18";

export const FONT_SIZE_OPTIONS: readonly { value: FontSize; label: string }[] =
  [
    { value: "12", label: "12px" },
    { value: "13", label: "13px" },
    { value: "14", label: "14px" },
    { value: "15", label: "15px" },
    { value: "16", label: "16px" },
    { value: "18", label: "18px" },
  ];

const FONT_SIZE_STORAGE_KEY = "kimi-code.font-size";
const DEFAULT_FONT_SIZE: FontSize = "14";

function isFontSize(value: string | null): value is FontSize {
  return FONT_SIZE_OPTIONS.some((option) => option.value === value);
}

export function loadFontSize(): FontSize {
  if (typeof window === "undefined") return DEFAULT_FONT_SIZE;
  try {
    const stored = window.localStorage.getItem(FONT_SIZE_STORAGE_KEY);
    return isFontSize(stored) ? stored : DEFAULT_FONT_SIZE;
  } catch {
    return DEFAULT_FONT_SIZE;
  }
}

export function applyFontSize(fontSize: FontSize): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.setProperty(
    "--font-scale",
    String(Number(fontSize) / Number(DEFAULT_FONT_SIZE)),
  );
}

export function saveFontSize(fontSize: FontSize): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(FONT_SIZE_STORAGE_KEY, fontSize);
  } catch {
    // A disabled or full storage area must not prevent font size switching.
  }
}

export type CustomColorKey = "accent";
export type CustomColors = Partial<Record<CustomColorKey, string>>;
export type CustomColorsByScheme = Record<ColorScheme, CustomColors>;

// Concrete fallbacks keep the native color picker in sync with the built-in
// palette. Surfaces and text deliberately stay theme-owned so changing an
// accent cannot accidentally reduce readability.
export const DEFAULT_SCHEME_COLORS: Record<ColorScheme, Record<CustomColorKey, string>> = {
  dark: { accent: "#58a6ff" },
  light: { accent: "#1783ff" },
};

export const ACCENT_COLOR_PRESETS = [
  { name: "violet", values: { dark: "#8b7cf6", light: "#6957df" } },
  { name: "blue", values: { dark: "#58a6ff", light: "#1783ff" } },
  { name: "teal", values: { dark: "#39b98a", light: "#087f5b" } },
  { name: "amber", values: { dark: "#e2a84f", light: "#b76500" } },
  { name: "rose", values: { dark: "#e27291", light: "#c63f63" } },
] as const;

const CUSTOM_COLORS_STORAGE_KEY = "kimi-code.custom-colors";
const HEX_COLOR_PATTERN = /^#[0-9a-fA-F]{6}$/;

function sanitizeCustomColors(value: unknown): CustomColors {
  if (typeof value !== "object" || value === null) return {};
  const colors: CustomColors = {};
  const color = (value as Record<string, unknown>).accent;
  if (typeof color === "string" && HEX_COLOR_PATTERN.test(color)) {
    colors.accent = color.toLowerCase();
  }
  return colors;
}

export function loadCustomColors(): CustomColorsByScheme {
  const fallback: CustomColorsByScheme = { light: {}, dark: {} };
  if (typeof window === "undefined") return fallback;
  try {
    const stored = window.localStorage.getItem(CUSTOM_COLORS_STORAGE_KEY);
    if (!stored) return fallback;
    const parsed: unknown = JSON.parse(stored);
    if (typeof parsed !== "object" || parsed === null) return fallback;
    const record = parsed as Record<string, unknown>;
    return {
      light: sanitizeCustomColors(record.light),
      dark: sanitizeCustomColors(record.dark),
    };
  } catch {
    return fallback;
  }
}

function hexToRgb(hex: string): [number, number, number] {
  return [
    Number.parseInt(hex.slice(1, 3), 16),
    Number.parseInt(hex.slice(3, 5), 16),
    Number.parseInt(hex.slice(5, 7), 16),
  ];
}

function relativeLuminance(hex: string): number {
  const channels = hexToRgb(hex).map((channel) => {
    const value = channel / 255;
    return value <= 0.04045
      ? value / 12.92
      : ((value + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

export function applyCustomColors(
  colors: CustomColors,
  colorScheme: ColorScheme,
): void {
  if (typeof document === "undefined") return;
  const style = document.documentElement.style;
  const derivedVariables = [
    "--accent",
    "--accent-2",
    "--accent-soft",
    "--accent-soft-subtle",
    "--accent-border",
    "--accent-contrast",
    "--focus-outline",
    "--landing-grid-line",
  ];

  if (!colors.accent) {
    for (const variable of derivedVariables) style.removeProperty(variable);
    return;
  }

  const accent = colors.accent;
  const hoverTarget = colorScheme === "dark" ? "white" : "black";
  const contrast = relativeLuminance(accent) > 0.179 ? "#111318" : "#ffffff";
  style.setProperty("--accent", accent);
  style.setProperty(
    "--accent-2",
    `color-mix(in srgb, ${accent} 84%, ${hoverTarget})`,
  );
  style.setProperty("--accent-soft", `color-mix(in srgb, ${accent} 14%, transparent)`);
  style.setProperty(
    "--accent-soft-subtle",
    `color-mix(in srgb, ${accent} 5%, transparent)`,
  );
  style.setProperty("--accent-border", `color-mix(in srgb, ${accent} 32%, transparent)`);
  style.setProperty("--accent-contrast", contrast);
  style.setProperty("--focus-outline", `color-mix(in srgb, ${accent} 72%, transparent)`);
  style.setProperty("--landing-grid-line", `color-mix(in srgb, ${accent} 12%, transparent)`);
}

export function saveCustomColors(colors: CustomColorsByScheme): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(CUSTOM_COLORS_STORAGE_KEY, JSON.stringify(colors));
  } catch {
    // A disabled or full storage area must not prevent color switching.
  }
}

export type InterfaceFontPreset =
  | "kimi"
  | "system"
  | "noto-sans"
  | "microsoft-yahei"
  | "pingfang"
  | "serif"
  | "custom";
export type CodeFontPreset =
  | "kimi"
  | "system"
  | "cascadia"
  | "consolas"
  | "sf-mono"
  | "menlo"
  | "custom";
export type FontFamilyPreset = InterfaceFontPreset | CodeFontPreset;
export type FontRole = "sans" | "mono";

export interface CustomFonts {
  sans?: InterfaceFontPreset;
  mono?: CodeFontPreset;
  sansCustom?: string;
  monoCustom?: string;
}

const INTERFACE_FALLBACK =
  '"Schibsted Grotesk Variable", "Schibsted Grotesk", "Helvetica Neue", Arial, "Noto Sans SC", "PingFang SC", "Microsoft YaHei", sans-serif';
const CODE_FALLBACK =
  '"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace';

const INTERFACE_FONT_STACKS: Record<InterfaceFontPreset, string | undefined> = {
  kimi: undefined,
  system:
    '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Noto Sans SC", "PingFang SC", "Microsoft YaHei", sans-serif',
  "noto-sans": `"Noto Sans SC", "Source Han Sans SC", ${INTERFACE_FALLBACK}`,
  "microsoft-yahei": `"Microsoft YaHei", ${INTERFACE_FALLBACK}`,
  pingfang: `"PingFang SC", ${INTERFACE_FALLBACK}`,
  serif:
    'Charter, Georgia, "Noto Serif SC", "Source Han Serif SC", "Songti SC", SimSun, serif',
  custom: undefined,
};

const CODE_FONT_STACKS: Record<CodeFontPreset, string | undefined> = {
  kimi: undefined,
  system:
    'ui-monospace, "SF Mono", Menlo, Monaco, Consolas, "Liberation Mono", monospace',
  cascadia:
    `"Cascadia Code", "Cascadia Mono", ${CODE_FALLBACK}`,
  consolas: `Consolas, ${CODE_FALLBACK}`,
  "sf-mono": `"SF Mono", ${CODE_FALLBACK}`,
  menlo: `Menlo, ${CODE_FALLBACK}`,
  custom: undefined,
};

const CUSTOM_FONT_VARS: Record<FontRole, string> = {
  sans: "--font-sans",
  mono: "--font-mono",
};

const CUSTOM_FONTS_STORAGE_KEY = "kimi-code.custom-fonts";
export const CUSTOM_FONT_NAME_MAX_LENGTH = 80;

function sanitizeCustomFontName(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (
    !trimmed ||
    trimmed.length > CUSTOM_FONT_NAME_MAX_LENGTH ||
    /[\u0000-\u001f,]/.test(trimmed)
  ) {
    return undefined;
  }
  return trimmed;
}

function isPreset<T extends string>(
  value: unknown,
  stacks: Record<T, string | undefined>,
): value is T {
  return typeof value === "string" && value in stacks;
}

export function loadCustomFonts(): CustomFonts {
  if (typeof window === "undefined") return {};
  try {
    const stored = window.localStorage.getItem(CUSTOM_FONTS_STORAGE_KEY);
    if (!stored) return {};
    const parsed: unknown = JSON.parse(stored);
    if (typeof parsed !== "object" || parsed === null) return {};
    const record = parsed as Record<string, unknown>;
    return {
      sans: isPreset(record.sans, INTERFACE_FONT_STACKS)
        ? record.sans
        : undefined,
      mono: isPreset(record.mono, CODE_FONT_STACKS) ? record.mono : undefined,
      sansCustom: sanitizeCustomFontName(record.sansCustom),
      monoCustom: sanitizeCustomFontName(record.monoCustom),
    };
  } catch {
    return {};
  }
}

export function applyCustomFonts(fonts: CustomFonts): void {
  if (typeof document === "undefined") return;
  const style = document.documentElement.style;
  const sansPreset = fonts.sans ?? "kimi";
  const monoPreset = fonts.mono ?? "kimi";
  const customSans = sanitizeCustomFontName(fonts.sansCustom);
  const customMono = sanitizeCustomFontName(fonts.monoCustom);
  const sans =
    sansPreset === "custom" && customSans
      ? `${JSON.stringify(customSans)}, ${INTERFACE_FALLBACK}`
      : INTERFACE_FONT_STACKS[sansPreset];
  const mono =
    monoPreset === "custom" && customMono
      ? `${JSON.stringify(customMono)}, ${CODE_FALLBACK}`
      : CODE_FONT_STACKS[monoPreset];
  if (sans) style.setProperty(CUSTOM_FONT_VARS.sans, sans);
  else style.removeProperty(CUSTOM_FONT_VARS.sans);
  if (mono) style.setProperty(CUSTOM_FONT_VARS.mono, mono);
  else style.removeProperty(CUSTOM_FONT_VARS.mono);
}

export function saveCustomFonts(fonts: CustomFonts): void {
  if (typeof window === "undefined") return;
  try {
    const stored: CustomFonts = {
      sans: isPreset(fonts.sans, INTERFACE_FONT_STACKS)
        ? fonts.sans
        : undefined,
      mono: isPreset(fonts.mono, CODE_FONT_STACKS) ? fonts.mono : undefined,
      sansCustom: sanitizeCustomFontName(fonts.sansCustom),
      monoCustom: sanitizeCustomFontName(fonts.monoCustom),
    };
    window.localStorage.setItem(CUSTOM_FONTS_STORAGE_KEY, JSON.stringify(stored));
  } catch {
    // A disabled or full storage area must not prevent font switching.
  }
}
