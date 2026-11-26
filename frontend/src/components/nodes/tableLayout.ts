// Keep high-variance transfer figures readable before spending horizontal room
// on the short status columns. The table already scrolls horizontally, so a
// little extra width is preferable to splitting a rate such as "994.56 KB/s"
// across two lines.
export const nodeDesktopColumnWidths = {
  status: 78,
  nodeVersion: 90,
  nodeUpgrade: 68,
  cpu: 80,
  mem: 96,
  disk: 96,
  rate: 192,
  traffic: 172,
} as const;
