import assert from "node:assert/strict";
import test from "node:test";

import {
  applyCustomColors,
  applyCustomFonts,
  applyFontSize,
} from "../src/appearance.ts";

test("custom accents generate the complete coordinated color scale", () => {
  const values = new Map<string, string>();
  const previousDocument = globalThis.document;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      documentElement: {
        style: {
          removeProperty: (name: string) => values.delete(name),
          setProperty: (name: string, value: string) => values.set(name, value),
        },
      },
    },
  });

  try {
    applyCustomColors({ accent: "#e2a84f" }, "dark");
    assert.equal(values.get("--accent"), "#e2a84f");
    assert.equal(values.get("--accent-contrast"), "#111318");
    assert.match(values.get("--accent-2") ?? "", /white/);
    assert.match(values.get("--accent-soft") ?? "", /14%/);
    assert.match(values.get("--accent-border") ?? "", /32%/);

    applyCustomColors({}, "dark");
    assert.equal(values.size, 0);
  } finally {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: previousDocument,
    });
  }
});

test("dark custom accents receive a light foreground", () => {
  const values = new Map<string, string>();
  const previousDocument = globalThis.document;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      documentElement: {
        style: {
          removeProperty: (name: string) => values.delete(name),
          setProperty: (name: string, value: string) => values.set(name, value),
        },
      },
    },
  });

  try {
    applyCustomColors({ accent: "#43206b" }, "light");
    assert.equal(values.get("--accent-contrast"), "#ffffff");
    assert.match(values.get("--accent-2") ?? "", /black/);
  } finally {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: previousDocument,
    });
  }
});

test("font size scales typography without applying page zoom", () => {
  const values = new Map<string, string>();
  const previousDocument = globalThis.document;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      documentElement: {
        style: {
          setProperty: (name: string, value: string) => values.set(name, value),
        },
      },
    },
  });

  try {
    applyFontSize("18");
    assert.equal(values.get("--font-scale"), String(18 / 14));
  } finally {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: previousDocument,
    });
  }
});

test("font presets keep robust fallbacks and reset to the Kimi defaults", () => {
  const values = new Map<string, string>();
  const previousDocument = globalThis.document;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      documentElement: {
        style: {
          removeProperty: (name: string) => values.delete(name),
          setProperty: (name: string, value: string) => values.set(name, value),
        },
      },
    },
  });

  try {
    applyCustomFonts({ sans: "system", mono: "cascadia" });
    assert.match(values.get("--font-sans") ?? "", /Segoe UI/);
    assert.match(values.get("--font-sans") ?? "", /Noto Sans SC/);
    assert.match(values.get("--font-mono") ?? "", /Cascadia Code/);
    assert.match(values.get("--font-mono") ?? "", /JetBrains Mono/);

    applyCustomFonts({});
    assert.equal(values.size, 0);
  } finally {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: previousDocument,
    });
  }
});

test("custom local fonts are quoted and always retain safe fallbacks", () => {
  const values = new Map<string, string>();
  const previousDocument = globalThis.document;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      documentElement: {
        style: {
          removeProperty: (name: string) => values.delete(name),
          setProperty: (name: string, value: string) => values.set(name, value),
        },
      },
    },
  });

  try {
    applyCustomFonts({
      sans: "custom",
      sansCustom: "Microsoft YaHei",
      mono: "custom",
      monoCustom: "Maple Mono NF CN",
    });
    assert.match(
      values.get("--font-sans") ?? "",
      /^"Microsoft YaHei",.*Noto Sans SC.*sans-serif$/,
    );
    assert.match(
      values.get("--font-mono") ?? "",
      /^"Maple Mono NF CN",.*JetBrains Mono.*monospace$/,
    );

    applyCustomFonts({ sans: "custom", sansCustom: "invalid,font" });
    assert.equal(values.has("--font-sans"), false);
  } finally {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: previousDocument,
    });
  }
});
