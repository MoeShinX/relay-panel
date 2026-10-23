import { describe, expect, it } from 'vitest';
import { upgradeIconShift, ALIGN_GAP, type Box } from './upgradeAlign';

const box = (left: number, right: number, top = 0, bottom = 40): Box => ({ left, right, top, bottom });

// A 1000px wide header row; the icon (24px) naturally sits at 900..924, the
// column's header cell is 400..468 (centre 434), the table shows 0..1000.
const base = {
  button: box(900, 924, 8, 32),
  container: box(0, 1000),
  cell: box(400, 468, 100, 130),
  visible: box(0, 1000, 90, 500),
  obstacles: [] as Box[],
};

describe('upgradeIconShift', () => {
  it("moves the icon so its centre is the column cell's centre", () => {
    // icon centre 912 -> cell centre 434
    expect(upgradeIconShift(base)).toBe(434 - 912);
  });

  it('returns 0 when the icon is already there', () => {
    expect(upgradeIconShift({ ...base, button: box(422, 446, 8, 32) })).toBe(0);
  });

  it('stays put (null) when there is no cell: phone list, no table, nothing to align to', () => {
    expect(upgradeIconShift({ ...base, cell: null })).toBeNull();
  });

  it('stays put when the cell has no size (a collapsed group keeps its table at zero size)', () => {
    expect(upgradeIconShift({ ...base, cell: box(0, 0, 0, 0) })).toBeNull();
  });

  it('stays put when the column is scrolled out of view, on either side', () => {
    expect(upgradeIconShift({ ...base, visible: box(500, 1000) })).toBeNull();
    expect(upgradeIconShift({ ...base, visible: box(0, 380) })).toBeNull();
  });

  it('still aligns when the column is only partly inside the viewport but its centre is in', () => {
    expect(upgradeIconShift({ ...base, visible: box(420, 1000) })).toBe(434 - 912);
  });

  it('stays put when the shifted icon would leave the row', () => {
    expect(upgradeIconShift({ ...base, cell: box(-30, 20, 100, 130) })).toBeNull();
    expect(upgradeIconShift({ ...base, container: box(0, 430) })).toBeNull();
  });

  it('stays put when the shifted icon would land on text in the row', () => {
    // the icon would sit at 422..446; a tag ending just inside the gap
    expect(upgradeIconShift({ ...base, obstacles: [box(380, 422 - ALIGN_GAP + 1, 4, 36)] })).toBeNull();
    // the rate text at 410..600
    expect(upgradeIconShift({ ...base, obstacles: [box(410, 600, 4, 36)] })).toBeNull();
  });

  it('is not bothered by text that is clear of it, or on another line of a wrapped row', () => {
    expect(upgradeIconShift({ ...base, obstacles: [box(100, 300, 4, 36), box(600, 800, 4, 36)] })).toBe(434 - 912);
    expect(upgradeIconShift({ ...base, obstacles: [box(400, 500, 60, 90)] })).toBe(434 - 912);
  });
});
