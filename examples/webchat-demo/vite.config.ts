import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  envPrefix: ["VITE_", "WEBCHAT_"],
  server: {
    port: 5174,
    proxy: {
      "/v3/directline": {
        target: "http://localhost:8090",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
