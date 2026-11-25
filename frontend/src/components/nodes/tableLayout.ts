// v1.3.3: column widths for the desktop node table.
//
// The order is now: status / node / live rate / connections / CPU / memory /
// network / version / upgrade / disk / cumulative traffic / uptime / detail.
// The rate moved up beside the identity block because it was the column you
// had to scroll for, and the two right-fixed columns (detail + remove) became
// one, so "系统运行时长" is no longer covered by them at 1440px.
//
// Transfer figures stay on one line: the table scrolls horizontally rather
// than splitting a rate such as "994.56 KB/s" across two lines.
export const nodeDesktopColumnWidths = {
  status: 78,
  node: 168,
  rate: 132,
  connections: 70,
  cpu: 80,
  mem: 96,
  network: 260,
  nodeVersion: 90,
  nodeUpgrade: 68,
  disk: 96,
  traffic: 172,
  uptime: 88,
  /** v1.3.3: one right-fixed column carrying the detail link and, on an offline
   *  row, the delete action. */
  detail: 108,
} as const;
