import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // v1.2 (PR4): split the big vendor libs into their own chunks so the app +
  // per-page chunks stay small, and a React/antd upgrade only invalidates the
  // vendor chunk (not every page). Pages themselves are split by the per-route
  // React.lazy() in router.tsx.
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          // React core (react + react-dom + scheduler). The router goes here too
          // so the lazy wrappers resolve without an extra round-trip.
          'react-vendor': ['react', 'react-dom', 'react-router-dom'],
          // Ant Design is by far the largest dep; isolate it so it caches
          // independently and the per-page chunks don't each carry a copy.
          antd: ['antd'],
          // The icon set is large and tree-shaken, but still worth isolating.
          icons: ['@ant-design/icons'],
          // semver is used by the node-version compare util.
          semver: ['semver'],
        },
      },
    },
  },
  server: {
    // Proxy /api to the panel during development so the frontend can talk to
    // the Rust backend without CORS friction.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:18888',
        changeOrigin: true,
      },
    },
  },
});

