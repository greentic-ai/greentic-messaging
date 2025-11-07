import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";

const sourceMapShim = fileURLToPath(new URL("./src/shims/source-map-js.ts", import.meta.url));

export default defineConfig({
  plugins: [react(), nodePolyfills()],
  envPrefix: ["VITE_", "WEBCHAT_"],
  resolve: {
    alias: {
      process: "process/browser",
      path: "path-browserify",
      url: "path-browserify",
      util: "util",
      stream: "stream-browserify",
      zlib: "browserify-zlib",
      "source-map-js": sourceMapShim,
      "source-map": sourceMapShim,
    },
  },
  define: {
    global: "globalThis",
    "process.env": {},
  },
  server: {
    port: 5174,
    proxy: {
      "/v3/directline": {
        target: "http://localhost:8090",
        changeOrigin: true,
        secure: false,
        ws: true,
      },
    },
  },
});
