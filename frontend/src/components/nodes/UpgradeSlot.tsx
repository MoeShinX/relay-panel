import { useCallback, useEffect, useLayoutEffect, useRef, type ReactNode, type RefObject } from 'react';
import { upgradeIconShift, UPGRADE_COL_CLASS, UPGRADE_OBSTACLE_ATTR, type Box } from './upgradeAlign';

const box = (el: Element): Box => {
  const r = el.getBoundingClientRect();
  return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
};

interface Props {
  /** The header row the icon sits in; the icon never leaves it. */
  rowRef: RefObject<HTMLElement | null>;
  /** The element holding the table(s) whose 节点更新 column to line up with. */
  tablesRef: RefObject<HTMLElement | null>;
  children: ReactNode;
}

/**
 * Keeps its child in the header row's normal flow, so the row lays out the same
 * whatever happens, and shifts it sideways (a CSS transform) so that its centre
 * sits over the centre of the first visible 节点更新 header cell below. Where
 * that is not possible (see upgradeIconShift) the shift is 0 and the child just
 * stays at its place in the row, so the button is always reachable.
 */
export function UpgradeSlot({ rowRef, tablesRef, children }: Props) {
  const slotRef = useRef<HTMLSpanElement>(null);
  // The shift currently applied, to read the un-shifted position back out of
  // getBoundingClientRect (which includes the transform).
  const appliedRef = useRef(0);

  const measure = useCallback(() => {
    const slot = slotRef.current;
    const row = rowRef.current;
    if (!slot || !row) return;
    const rect = box(slot);
    const natural: Box = {
      ...rect,
      left: rect.left - appliedRef.current,
      right: rect.right - appliedRef.current,
    };
    let cell: HTMLElement | null = null;
    const cells = Array.from(tablesRef.current?.querySelectorAll<HTMLElement>(`th.${UPGRADE_COL_CLASS}`) ?? []);
    for (const c of cells) {
      // A collapsed group keeps its table in the DOM at zero size: skip it.
      if (c.getBoundingClientRect().width > 0) { cell = c; break; }
    }
    const viewport = cell?.closest('.ant-table-container') ?? null;
    const obstacles = Array.from(row.querySelectorAll(`[${UPGRADE_OBSTACLE_ATTR}]`)).map(box);
    const shift = upgradeIconShift({
      button: natural,
      container: box(row),
      cell: cell ? box(cell) : null,
      visible: viewport ? box(viewport) : null,
      obstacles,
    }) ?? 0;
    if (shift !== appliedRef.current) {
      appliedRef.current = shift;
      slot.style.transform = shift ? `translateX(${shift}px)` : '';
    }
  }, [rowRef, tablesRef]);

  // Every render: the poll replaces rows, which can change column widths.
  useLayoutEffect(() => { measure(); });

  // Passive, not layout: a parent's ref is attached after this child's layout
  // effects run, so on the first mount the row is only reachable from here.
  useEffect(() => {
    measure();
    const row = rowRef.current;
    const tables = tablesRef.current;
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    if (row) ro?.observe(row);
    if (tables) ro?.observe(tables);
    window.addEventListener('resize', measure);
    // scroll does not bubble: the capture phase catches the table's own
    // horizontal scrolling from out here.
    tables?.addEventListener('scroll', measure, true);
    return () => {
      ro?.disconnect();
      window.removeEventListener('resize', measure);
      tables?.removeEventListener('scroll', measure, true);
    };
  }, [measure, rowRef, tablesRef]);

  return (
    <span ref={slotRef} style={{ display: 'inline-flex', alignItems: 'center' }}>
      {children}
    </span>
  );
}
