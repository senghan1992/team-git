import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";
import { gitBridgePlugin } from "./dev/git-bridge";

export default defineConfig({
  plugins: [tailwindcss(), gitBridgePlugin()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "esnext",
    outDir: "dist",
    emptyOutDir: true,
  },
});
