// v0.4.10: global test setup. Adds jest-dom matchers (toBeInTheDocument etc.)
// to Vitest's expect so component assertions read naturally. Loaded once per
// test run via vitest config setupFiles.
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

// jsdom doesn't implement ResizeObserver or matchMedia, but antd components
// (Table, Collapse, Progress) reference them on mount. Provide minimal stubs so
// component-rendering tests don't crash. Harmless for non-antd tests.
if (!globalThis.ResizeObserver) {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }) as unknown as MediaQueryList;
}

// jsdom throws "Not implemented: window.getComputedStyle(elt, pseudoElt)" when
// antd reads a pseudo-element style (e.g. ::before for Wave / ripple effects).
// jsdom DOES implement the single-arg form, so wrap it to drop the pseudoElt
// arg. This silences a known-environment noise that floods the test output and
// obscures REAL warnings (e.g. antd deprecations, React 19 issues) — it does
// NOT suppress any console.warn/error, only this one jsdom "Error: Not
// implemented" path. Real getComputedStyle values are unaffected for tests
// that pass no pseudo-element.
const _origGetComputedStyle = window.getComputedStyle.bind(window);
// jsdom throws on the 2-arg (pseudoElt) form; drop any extra args so it uses
// the implemented single-arg form. Declared as a rest arg so we don't name an
// unused parameter.
window.getComputedStyle = ((elt: Element) =>
  _origGetComputedStyle(elt)) as typeof window.getComputedStyle;

// v1.2.0: @ant-design/charts renders on <canvas>, which jsdom does not
// implement — mounting any page that embeds a chart (Dashboard, Account) would
// crash with "Not implemented: HTMLCanvasElement.prototype.getContext". A
// chart is visual output jsdom could never assert on anyway, so stub the
// library; the page tests keep testing the page around it.
vi.mock('@ant-design/charts', () => ({
  Column: () => null,
}));

// Unmount rendered components between tests so each test starts from a clean
// DOM (otherwise multiple StateProbe instances accumulate and getByTestId finds
// duplicates). @testing-library/react auto-cleans when globals are enabled; we
// do it explicitly since vitest runs without globals here.
afterEach(() => {
  cleanup();
});
