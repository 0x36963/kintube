import { reactRouter } from "@react-router/dev/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [tailwindcss(), reactRouter()],
  resolve: {
    tsconfigPaths: true,
  },
  server: {
    port: 5173,
    strictPort: true,
    // Same-origin API in dev, as in production (Cloudflare routes /api/* to the Rust origin).
    // The browser's Origin header passes through untouched, and the API checks it against APP_ORIGIN.
    proxy: {
      "/api": "http://127.0.0.1:8787",
    },
  },
});
