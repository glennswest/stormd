import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// The SPA is served by stormd at /ui/. Output filenames are fixed (no content
// hashes) so the embedded-asset handler and the git diff of web/dist stay
// stable across builds.
const target = process.env.STORMD_URL || 'http://localhost:9080'

export default defineConfig({
  base: '/ui/',
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: 'assets/app[extname]',
      },
    },
  },
  server: {
    proxy: {
      '/api': { target },
      '/ui/proxy': { target },
      '/ws': { target, ws: true },
    },
  },
})
