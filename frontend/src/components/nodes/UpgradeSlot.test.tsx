import { describe, expect, it, vi, afterEach, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { useRef } from 'react';
import { UpgradeSlot } from './UpgradeSlot';

// jsdom has no layout: give elements their rectangle through data-rect="l,r,t,b".
const rectOf = (el: Element) => {
  // the slot span: it wraps the button, and sits where the icon naturally does
  const raw = el.firstElementChild?.getAttribute('data-testid') === 'b' ? '900,924,8,32' : (el as HTMLElement).dataset?.rect;
  const [left, right, top, bottom] = raw ? raw.split(',').map(Number) : [0, 0, 0, 0];
  return { left, right, top, bottom, width: right - left, height: bottom - top, x: left, y: top, toJSON: () => ({}) };
};

beforeEach(() => {
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    return rectOf(this) as DOMRect;
  });
});
afterEach(() => vi.restoreAllMocks());

// The slot (the span wrapping the button) is what is measured and shifted, so
// the slot is the element that carries the icon's rectangle.
function Harness({ cells }: { cells: string[] }) {
  const row = useRef<HTMLDivElement>(null);
  const tables = useRef<HTMLDivElement>(null);
  return (
    <>
      <div ref={row} data-rect="0,1000,0,40">
        <span data-upgrade-obstacle="" data-rect="10,200,4,36">name</span>
        <UpgradeSlot rowRef={row} tablesRef={tables}>
          <button data-testid='b'>x</button>
        </UpgradeSlot>
      </div>
      <div ref={tables}>
        {cells.map((rect, i) => (
          <div key={i} className="ant-table-container" data-rect="0,1000,90,500">
            <table><thead><tr><th className="rp-col-node-upgrade" data-rect={rect} /></tr></thead></table>
          </div>
        ))}
      </div>
    </>
  );
}

const slotOf = (c: HTMLElement) => c.querySelector('[data-testid="b"]')!.parentElement as HTMLElement;

describe('UpgradeSlot', () => {
  it('shifts its child over the 节点更新 header cell', () => {
    const { container } = render(<Harness cells={['400,468,100,130']} />);
    expect(slotOf(container).style.transform).toBe(`translateX(${434 - 912}px)`);
  });

  it('skips a collapsed (zero-width) group table and uses the next one', () => {
    const { container } = render(<Harness cells={['0,0,0,0', '300,368,100,130']} />);
    expect(slotOf(container).style.transform).toBe(`translateX(${334 - 912}px)`);
  });

  it('leaves the child in place when there is no table (phone)', () => {
    const { container } = render(<Harness cells={[]} />);
    expect(slotOf(container).style.transform).toBe('');
  });

  it('leaves the child in place when the column has scrolled out of the table viewport', () => {
    const { container } = render(<Harness cells={['1200,1268,100,130']} />);
    expect(slotOf(container).style.transform).toBe('');
  });
});
