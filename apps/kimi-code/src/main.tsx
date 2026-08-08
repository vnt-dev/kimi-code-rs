import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { applyColorScheme, loadColorScheme } from "./appearance";
import "./styles.css";
import { isDesktop } from "./transport";

applyColorScheme(loadColorScheme());

if (isDesktop()) {
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
