import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { applyColorScheme, loadColorScheme } from "./appearance";
import "./styles.css";

applyColorScheme(loadColorScheme());

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
