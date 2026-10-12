import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      // The program serves its API under `/api`, as the UI speaks to it.
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      }
    }
  }
})
