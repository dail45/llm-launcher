import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// In dev, proxy API calls to a running launcher (launcherd or the desktop app).
export default defineConfig({
  plugins: [svelte()],
  base: './',
  server: {
    port: 5173,
    proxy: { '/api': 'http://127.0.0.1:8090' },
  },
  build: { outDir: 'dist', emptyOutDir: true, chunkSizeWarningLimit: 1500 },
})
