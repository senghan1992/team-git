import "./styles/app.css";
import { createApp } from "./lib/app";

// Dev-only browser mock (no Tauri bridge); tree-shaken out of production builds.
if (import.meta.env.DEV) {
  await import("./_dev_shim");
}

const root = document.getElementById("app");
if (!root) throw new Error("missing #app");
createApp(root);