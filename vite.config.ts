import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri 开发环境固定端口，避免与后端冲突
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
