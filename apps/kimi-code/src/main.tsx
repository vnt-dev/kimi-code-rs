import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import {
  applyColorScheme,
  applyCustomColors,
  applyCustomFonts,
  applyFontSize,
  loadColorScheme,
  loadCustomColors,
  loadCustomFonts,
  loadFontSize,
} from "./appearance";
import "./styles.css";
import { isDesktop } from "./transport";

const initialColorScheme = loadColorScheme();
applyColorScheme(initialColorScheme);
applyFontSize(loadFontSize());
applyCustomColors(loadCustomColors()[initialColorScheme], initialColorScheme);
applyCustomFonts(loadCustomFonts());

if (isDesktop()) {
  document.addEventListener("contextmenu", (event) => event.preventDefault());
  document.addEventListener(
    "keydown",
    (event) => {
      const isReload =
        event.key === "F5" ||
        ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "r");
      if (isReload) event.preventDefault();
    },
    true,
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
