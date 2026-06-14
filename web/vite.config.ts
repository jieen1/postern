/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath, URL } from 'node:url';

// SPA build + dev server. The browser cannot reach the UDS control.sock
// directly; in dev MSW intercepts /v1/* so the SPA runs with no daemon.
// When a real local bridge exists, set VITE_API_PROXY to its http origin
// and MSW can be disabled via VITE_ENABLE_MSW=false.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    port: 5173,
    // web-private form: when a local HTTP->UDS bridge is running, proxy /v1/* to
    // it (the browser can't speak UDS). Unset in mock (MSW intercepts) / tauri
    // (tauriTransport via invoke). Also the harness for headless live-backend e2e.
    proxy: process.env.VITE_API_PROXY
      ? { '/v1': { target: process.env.VITE_API_PROXY, changeOrigin: true } }
      : undefined,
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
});
