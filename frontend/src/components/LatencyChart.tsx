import { useMemo } from 'react';
import { Line } from '@ant-design/charts';
import { useI18n } from '../i18n/context';
import { SERIES_COLORS_LIGHT } from '../utils/trafficBuckets';
import {
  SYMBOL_MAX_POINTS,
  formatChartTime,
  lossMarkers,
  maxPointsPerKey,
  smoothPoints,
  tooltipText,
  type ChartPoint,
} from '../utils/latencyChart';

/** The subset of the chart instance the latency pages use (see @ant-design/plots). */
export interface LatencyChartHandle {
  downloadImage?: (name?: string, type?: string) => string;
}

/** Height of the chart box on both latency pages. The loading, empty and error
 *  states fill the same box, so the page does not jump while data comes and goes. */
export const CHART_HEIGHT = 320;

/** Axis and grid ink. Neutral and translucent, so it reads on any surface. */
const INK = 'rgba(100, 108, 120, 0.9)';
const GRID = 'rgba(100, 108, 120, 0.22)';

interface Props {
  points: ChartPoint[];
  /** Line keys in legend order, so a line keeps its colour whatever is shown. */
  colorDomain: string[];
  /** Legend label of each key. */
  names: Map<string, string>;
  /** A range of a day or more needs the date on the axis too. */
  withDate: boolean;
  /** Draw each line as a moving average (chart only, see smoothPoints). */
  smooth?: boolean;
  onReady: (chart: LatencyChartHandle) => void;
}

/**
 * v1.2.21: the line chart both latency pages draw — a node's targets, or a
 * group's nodes for one target. One line per key; the x scale is a time scale
 * over real Dates, so buckets keep their true spacing, and a bucket that
 * answered nothing is a gap in the line, never a zero or an interpolation.
 *
 * Calm by design: thin lines, no axis lines or ticks, light dashed horizontal
 * grid only, y labels inside the plot, no animation (the page re-reads every
 * minute and a redraw must not make the lines jump). Symbols show on every point
 * only for short lines (up to SYMBOL_MAX_POINTS); longer ones show the point
 * under the cursor. Buckets that lost probes carry a small red marker on the
 * line (on the baseline under a gap). Zoom is a slim slider; the page resets it
 * by remounting the chart. The legend toggles lines.
 *
 * The tooltip lists the lines at the hovered time, highest first: the bucket's
 * own mean (not the smoothed value), loss and retransmit share.
 */
export default function LatencyChart({ points, colorDomain, names, withDate, smooth = false, onReady }: Props) {
  const { t } = useI18n();
  const plotted = useMemo(() => (smooth ? smoothPoints(points) : points), [points, smooth]);
  const markers = useMemo(() => lossMarkers(plotted), [plotted]);
  const symbols = useMemo(() => maxPointsPerKey(points) <= SYMBOL_MAX_POINTS, [points]);
  const labels = useMemo(
    () => ({ allLost: t('latencyAllLost'), loss: t('latencyTipLoss'), retrans: t('latencyCardRetrans') }),
    [t],
  );
  return (
    <Line
      /* @ant-design/charts v2 (G2 v5) options. */
      height={CHART_HEIGHT}
      data={plotted}
      xField="time"
      yField="value"
      colorField="key"
      animate={false}
      connectNulls={false}
      scale={{
        x: { type: 'time' },
        y: { nice: true, domainMin: 0 },
        color: { domain: colorDomain, range: SERIES_COLORS_LIGHT },
      }}
      style={{ lineWidth: 2 }}
      point={symbols ? { shapeField: 'circle', sizeField: 2.5 } : undefined}
      slider={{
        x: {
          // Slim: no data-shadow preview, a thin track, small handles, no value labels.
          sparklineData: [],
          size: 18,
          trackSize: 4,
          trackFill: 'rgba(100, 108, 120, 0.12)',
          selectionFill: 'rgba(99, 102, 241, 0.3)',
          selectionFillOpacity: 1,
          showLabel: false,
          handleIconSize: 10,
        },
      }}
      legend={{
        color: {
          position: 'top',
          layout: { justifyContent: 'flex-end' },
          itemMarker: 'circle',
          itemMarkerSize: 10,
          itemLabelFontSize: 12,
          labelFormatter: (k: string) => names.get(k) ?? k,
        },
      }}
      axis={{
        x: {
          title: false,
          line: false,
          tick: false,
          grid: false,
          tickCount: 6,
          labelAutoHide: true,
          labelAutoRotate: false,
          labelFill: INK,
          labelFontSize: 11,
          labelFormatter: (d: Date | number) => formatChartTime(new Date(d), withDate),
        },
        y: {
          title: false,
          line: false,
          tick: false,
          grid: true,
          gridLineDash: [3, 4],
          gridStroke: GRID,
          gridStrokeOpacity: 1,
          gridLineWidth: 1,
          tickCount: 5,
          labelDirection: 'positive',
          labelFill: INK,
          labelFontSize: 11,
          labelFormatter: (v: number) => `${Math.round(v)} ms`,
        },
      }}
      annotations={[
        {
          type: 'point',
          data: markers,
          encode: { x: 'time', y: 'y' },
          style: { fill: '#ef4444', stroke: '#ffffff', lineWidth: 1, r: 3 },
          tooltip: false,
        },
      ]}
      interaction={{
        tooltip: {
          shared: true,
          crosshairs: true,
          crosshairsStroke: GRID,
          // Highest first; "all lost" (no number) goes last.
          sort: (item: { value: string }) => {
            const v = parseFloat(item.value);
            return Number.isNaN(v) ? Infinity : -v;
          },
        },
      }}
      tooltip={{
        title: (d: ChartPoint) => formatChartTime(d.time, true),
        items: [(d: ChartPoint) => ({ name: d.name, value: tooltipText(d, labels) })],
      }}
      onReady={onReady}
    />
  );
}
