import { useMemo } from 'react';
import { Line } from '@ant-design/charts';
import { useI18n } from '../i18n/context';
import { LINE_LEGEND_ITEM, LINE_WIDTH, SERIES_COLORS_LIGHT } from '../utils/trafficBuckets';
import {
  SYMBOL_MAX_POINTS,
  classicLossMarkers,
  formatChartTime,
  formatLoss,
  formatMs,
  formatRetrans,
  lossMarkers,
  maxPointsPerKey,
  smoothPoints,
  tooltipText,
  yDomainMin,
  type ChartPoint,
} from '../utils/latencyChart';
import { LEVEL_COLORS } from '../utils/latency';

/** The subset of the chart instance the latency pages use (see @ant-design/plots). */
export interface LatencyChartHandle {
  downloadImage?: (name?: string, type?: string) => string;
}

/** Height of the chart box on both latency pages. The loading, empty and error
 *  states fill the same box, so the page does not jump while data comes and goes. */
export const CHART_HEIGHT = 320;

/** The two looks of the trend chart: the calm one, and the one the pages had
 *  before (y axis from 0, a marker for every lossy bucket on the x axis, a zoom
 *  slider with a preview, avg / min / max / loss in the tooltip). */
export type ChartVariant = 'modern' | 'classic';

/** Axis and grid ink of the modern look. Neutral and translucent, so it reads
 *  on any surface. */
const INK = 'rgba(75, 85, 99, 0.95)';
const GRID = 'rgba(100, 108, 120, 0.22)';

interface Props {
  points: ChartPoint[];
  /** Line keys in legend order, so a line keeps its colour whatever is shown. */
  colorDomain: string[];
  /** Legend label of each key. */
  names: Map<string, string>;
  /** A range of a day or more needs the date on the axis too. */
  withDate: boolean;
  /** Draw each line as a moving average (chart only, see smoothPoints). Works in
   *  both looks. */
  smooth?: boolean;
  /** Which look to draw. */
  variant?: ChartVariant;
  onReady: (chart: LatencyChartHandle) => void;
}

/** What the option builders need from the component. */
interface Ctx {
  plotted: ChartPoint[];
  names: Map<string, string>;
  withDate: boolean;
  tip: (key: string) => string;
}

/** "0.20% · Retrans 17%": the loss text with the retransmit share beside it when
 *  the data carries one. */
function lossText(d: ChartPoint, retransLabel: string): string {
  const loss = formatLoss(d.loss);
  const r = formatRetrans(d.retrans);
  return r == null ? loss : `${loss} · ${retransLabel} ${r}`;
}

/**
 * The calm look: thin lines, no axis lines or ticks, light dashed horizontal
 * grid only, y labels outside but small and muted, no animation (the page
 * re-reads every minute and a redraw must not make the lines jump). The y axis
 * starts just under the lowest value. Symbols on every point only for short
 * lines; longer ones show the point under the cursor. A fully lost bucket gets a
 * small red marker (on the bottom edge, under the gap). Slim zoom slider without
 * a preview. The tooltip lists the lines at the hovered time, highest first: the
 * bucket's own mean (not the smoothed value), loss and retransmit share.
 */
function modernOptions({ plotted, names, withDate, tip }: Ctx) {
  const yMin = yDomainMin(plotted);
  const labels = { allLost: tip('latencyAllLost'), loss: tip('latencyTipLoss'), retrans: tip('latencyCardRetrans') };
  return {
    animate: false,
    connectNulls: false,
    scale: { x: { type: 'time' }, y: { nice: true, domainMin: yMin } },
    style: { lineWidth: 2 },
    point: maxPointsPerKey(plotted) <= SYMBOL_MAX_POINTS ? { shapeField: 'circle', sizeField: 2.5 } : undefined,
    slider: {
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
    },
    legend: {
      color: {
        position: 'top',
        layout: { justifyContent: 'flex-end' },
        itemMarker: 'circle',
        itemMarkerSize: 10,
        itemLabelFontSize: 12,
        labelFormatter: (k: string) => names.get(k) ?? k,
      },
    },
    axis: {
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
        labelFill: INK,
        labelFontSize: 10,
        labelFormatter: (v: number) => `${Math.round(v)} ms`,
      },
    },
    annotations: [
      {
        type: 'point',
        data: lossMarkers(plotted, yMin),
        encode: { x: 'time', y: 'y' },
        style: { fill: LEVEL_COLORS.bad, stroke: '#ffffff', lineWidth: 1, r: 3 },
        tooltip: false,
      },
    ],
    interaction: {
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
    },
    tooltip: {
      title: (d: ChartPoint) => formatChartTime(d.time, true),
      items: [(d: ChartPoint) => ({ name: d.name, value: tooltipText(d, labels) })],
    },
  };
}

/**
 * The look the pages had before the polish: the y axis from 0, G2's own axes
 * and grid, a red marker on the x axis for every bucket that lost anything, the
 * zoom slider with its preview, and avg / min / max / loss per line in the
 * tooltip (retransmit share appended to the loss when the data has one; the max
 * is the retransmit-free one the API returns now). The chart box is the same
 * fixed height as in the modern look.
 */
function classicOptions({ plotted, names, withDate, tip }: Ctx) {
  const retransLabel = tip('latencyCardRetrans');
  return {
    scale: { x: { type: 'time' }, y: { nice: true, domainMin: 0 } },
    style: { lineWidth: LINE_WIDTH },
    slider: { x: {} },
    legend: {
      color: {
        position: 'top',
        layout: { justifyContent: 'flex-end' },
        ...LINE_LEGEND_ITEM,
        labelFormatter: (k: string) => names.get(k) ?? k,
      },
    },
    axis: {
      x: {
        title: false,
        labelAutoHide: true,
        labelAutoRotate: false,
        labelFormatter: (d: Date | number) => formatChartTime(new Date(d), withDate),
      },
      y: { title: false, labelFormatter: (v: number) => `${v} ms` },
    },
    annotations: [
      {
        type: 'point',
        data: classicLossMarkers(plotted),
        encode: { x: 'time', y: () => 0 },
        style: { fill: '#ff4d4f', stroke: '#ff4d4f', r: 3, fillOpacity: 0.85 },
        tooltip: false,
      },
    ],
    tooltip: {
      title: (d: ChartPoint) => formatChartTime(d.time, true),
      items: [
        (d: ChartPoint) => ({
          name: `${d.name} · ${tip('latencyTipAvg')}`,
          value: d.raw == null ? tip('latencyAllLost') : formatMs(d.raw),
        }),
        (d: ChartPoint) => ({ name: tip('latencyTipMin'), value: formatMs(d.min) }),
        (d: ChartPoint) => ({ name: tip('latencyTipMax'), value: formatMs(d.max) }),
        (d: ChartPoint) => ({ name: tip('latencyTipLoss'), value: lossText(d, retransLabel) }),
      ],
    },
  };
}

/**
 * v1.2.21: the line chart both latency pages draw — a node's targets, or a
 * group's nodes for one target. One line per key; the x scale is a time scale
 * over real Dates, so buckets keep their true spacing, and a bucket that
 * answered nothing is a gap in the line, never a zero or an interpolation. The
 * page resets the zoom by remounting the chart; the legend toggles lines.
 * `variant` only picks the option set (see modernOptions / classicOptions): the
 * data, the smoothing and the colours are the same.
 */
export default function LatencyChart({
  points,
  colorDomain,
  names,
  withDate,
  smooth = false,
  variant = 'modern',
  onReady,
}: Props) {
  const { t } = useI18n();
  const plotted = useMemo(() => (smooth ? smoothPoints(points) : points), [points, smooth]);
  const { scale, ...rest } = useMemo(() => {
    const ctx: Ctx = { plotted, names, withDate, tip: t };
    return variant === 'classic' ? classicOptions(ctx) : modernOptions(ctx);
  }, [variant, plotted, names, withDate, t]);
  return (
    <Line
      /* @ant-design/charts v2 (G2 v5) options. */
      {...rest}
      height={CHART_HEIGHT}
      data={plotted}
      xField="time"
      yField="value"
      colorField="key"
      scale={{ ...scale, color: { domain: colorDomain, range: SERIES_COLORS_LIGHT } }}
      onReady={onReady}
    />
  );
}
