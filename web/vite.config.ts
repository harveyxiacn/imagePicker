import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, 'src') } },
  define: { __MOCK__: JSON.stringify(mode === 'mock' || process.env.VITE_MOCK === '1') },
  server: {
    port: 5173,
    proxy:
      mode === 'mock'
        ? undefined
        : { '/api': { target: 'http://127.0.0.1:7878', changeOrigin: true, ws: true } },
  },
  // Largest chunk after the route split is the main one (~694 kB); keep the warning just above it so growth shows up.
  build: { chunkSizeWarningLimit: 720 },
  test: { environment: 'node', include: ['src/**/*.test.ts'], globals: false },
}))
