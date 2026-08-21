import { resolve } from 'node:path'
import { defineConfig } from 'electron-vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  main: {
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/main/index.ts') } } },
  },
  preload: {
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/preload/index.ts') } } },
  },
  renderer: {
    root: resolve(__dirname, 'src/renderer'),
    plugins: [react()],
    build: {
      // Without this the renderer bundle escapes to the repo root, because outDir
      // resolves relative to `root`.
      outDir: resolve(__dirname, 'out/renderer'),
      rollupOptions: { input: { index: resolve(__dirname, 'src/renderer/index.html') } },
    },
  },
})
