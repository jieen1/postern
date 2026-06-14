/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_ENABLE_MSW?: string;
  readonly VITE_API_PROXY?: string;
  readonly VITE_TARGET?: 'tauri' | 'web' | 'mock';
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
