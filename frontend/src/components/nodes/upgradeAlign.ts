/**
 * Where the "upgrade all / upgrade group" icon goes so that it sits on the same
 * vertical line as the per-node upgrade icons, which live in the 节点更新 column
 * of the table below it.
 *
 * Pure geometry, kept apart from the DOM so it can be tested (jsdom has no
 * layout). All rects are in viewport coordinates, as getBoundingClientRect
 * returns them.
 */
export interface Box {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** Class on the 节点更新 column, which antd puts on its header and body cells. */
export const UPGRADE_COL_CLASS = 'rp-col-node-upgrade';
/** Marks the things in a header row that the icon must not be moved onto. */
export const UPGRADE_OBSTACLE_ATTR = 'data-upgrade-obstacle';

/** Clear space kept between the icon and any text beside it. */
export const ALIGN_GAP = 6;

export interface AlignInput {
  /** Where the icon sits in the header row with no shift applied. */
  button: Box;
  /** The row the icon lives in (a group header, or the page header). */
  container: Box;
  /** The 节点更新 header cell, or null when there is none to align to. */
  cell: Box | null;
  /** The part of that table that is actually on screen (its scroll viewport). */
  visible: Box | null;
  /** Things in the same row the icon must not land on: the name, tags, rate. */
  obstacles: Box[];
}

const overlaps = (a: Box, b: Box, gap: number) =>
  a.left < b.right + gap && a.right > b.left - gap && a.top < b.bottom && a.bottom > b.top;

/**
 * The horizontal shift (px, to the right when positive) that centres the icon
 * over the column, or null when it should stay where it naturally sits:
 *   - there is no usable cell (no table: phone list, collapsed group, no nodes),
 *   - the column is scrolled out of view (its centre is outside the viewport of
 *     the table),
 *   - or the shifted icon would leave the row or land on text, which is what
 *     happens on a narrow window.
 */
export function upgradeIconShift(input: AlignInput): number | null {
  const { button, container, cell, visible, obstacles } = input;
  if (!cell) return null;
  const cellWidth = cell.right - cell.left;
  if (cellWidth <= 0 || cell.bottom - cell.top <= 0) return null;
  const centre = (cell.left + cell.right) / 2;
  if (visible && (centre < visible.left || centre > visible.right)) return null;

  const width = button.right - button.left;
  const dx = centre - (button.left + width / 2);
  if (Math.abs(dx) < 0.5) return 0;
  const target: Box = { ...button, left: button.left + dx, right: button.right + dx };
  if (target.left < container.left || target.right > container.right) return null;
  if (obstacles.some((o) => overlaps(target, o, ALIGN_GAP))) return null;
  return dx;
}
