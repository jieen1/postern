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
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
});
