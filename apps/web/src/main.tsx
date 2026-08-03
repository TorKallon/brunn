import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { StraylightApp } from "./App";
import { initializeAppearance } from "./lib/appearance";
import { createAppRouter } from "./router";
import "./styles.css";

initializeAppearance();

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

createRoot(root).render(
  <StrictMode>
    <StraylightApp router={createAppRouter()} />
  </StrictMode>,
);
