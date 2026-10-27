import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    proxy: {
      "/api/cloud": "http://127.0.0.1:3200",
      "/api": "http://127.0.0.1:3100",
      "/auth": "http://127.0.0.1:3100",
      "/runtime.json": "http://127.0.0.1:3100",
    },
  },
});
