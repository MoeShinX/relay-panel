import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';

// A test file's jsdom is torn down as soon as its last test finishes. React's
// scheduler can still have a task queued on Node's own timers at that moment
// (a state update from a mock that resolved late, an antd toast closing), and
// when it fires, react-dom touches `window`, which no longer exists. Vitest
// reports that as an unhandled error and fails the whole run although every
// test passed. setup.ts drains what it can after each test; this drops only
// that exact leftover: a "window/document is not defined" ReferenceError raised
// from react-dom or its scheduler. While a test runs, jsdom provides both
// globals, so this error cannot come from code under test and hides nothing.
// Every other unhandled error still fails the run.
function isReactAfterTeardown(error: { name?: string; message?: string; stack?: string }): boolean {
  return (
    error.name === 'ReferenceError' &&
    /^(window|document) is not defined$/.test(error.message ?? '') &&
    /node_modules[\\/](react-dom|scheduler)[\\/]/.test(error.stack ?? '')
  );
}

// v0.4.10: separate vitest config. vite.config.ts stays a pure vite config
// (so `vite build` and tsc see no unknown `test` field); this file adds the
// test settings on top. mergeConfig keeps the two in sync automatically.
export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      // Component tests need a DOM. Pure-logic tests (e.g. version.test) are
      // unaffected — jsdom provides a window they simply don't touch.
      environment: 'jsdom',
      setupFiles: ['./src/test/setup.ts'],
      exclude: ['node_modules', 'dist'],
      onUnhandledError(error) {
        if (isReactAfterTeardown(error)) {
          console.warn(`[vitest] ignored React work that ran after jsdom teardown: ${error.message}`);
          return false;
        }
      },
    },
  })
);
