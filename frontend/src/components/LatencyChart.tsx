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
  lossLabel,
  lossMarkers,
  maxPointsPerKey,
  showRaw,
  slowText,
  smoothPoints,
  tooltipText,
  yDomainMin,
  type ChartPoint,
  type TooltipLabels,
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
const INK = '#64748b';
const GRID = 'rgba(148, 163, 184, 0.24)';

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
  /** Hide internal legend when an external interactive legend strip is shown above. */
  hideLegend?: boolean;
  onReady: (chart: LatencyChartHandle) => void;
}

/** What the option builders need from the component. */
interface Ctx {
  plotted: ChartPoint[];
  names: Map<string, string>;
  withDate: boolean;
  hideLegend: boolean;
  tip: (key: string) => string;
}

/** The labels of a chart's tooltips, in the page's language. */
function tooltipLabels(tip: (key: string) => string): TooltipLabels {
  return {
    allLost: tip('latencyAllLost'),
    loss: tip('latencyTipLoss'),
    failed: tip('latencyTipFailed'),
    slow: tip('latencyCardSlow'),
    rawAvg: tip('latencyTipRaw'),
  };
}

/** "0.20% · Slow 17%": the loss (TCP: failed) text with the slow-handshake
 *  share beside it where the target has one (a TCP target whose data lacks it
 *  says "—", an ICMP target has none). */
function lossText(d: ChartPoint, labels: TooltipLabels): string {
  const loss = formatLoss(d.loss);
  const s = slowText(d.slow, d.kind);
  return s == null ? loss : `${loss} · ${labels.slow} ${s}`;
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
function modernOptions({ plotted, names, withDate, hideLegend, tip }: Ctx) {
  const yMin = yDomainMin(plotted);
  const labels = tooltipLabels(tip);
  return {
    animate: false,
    connectNulls: false,
    scale: { x: { type: 'time' }, y: { nice: true, domainMin: yMin } },
    style: { lineWidth: 2.2 },
    point: maxPointsPerKey(plotted) <= SYMBOL_MAX_POINTS ? { shapeField: 'circle', sizeField: 2.5 } : undefined,
    slider: {
      x: {
        // Slim: no data-shadow preview, a thin track, small handles, no value labels.
        sparklineData: [],
        size: 16,
        trackSize: 4,
        trackFill: 'rgba(148, 163, 184, 0.16)',
        selectionFill: 'rgba(24, 24, 27, 0.22)',
        selectionFillOpacity: 1,
        showLabel: false,
        handleIconSize: 9,
      },
    },
    legend: hideLegend ? false : {
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
function classicOptions({ plotted, names, withDate, hideLegend, tip }: Ctx) {
  const labels = tooltipLabels(tip);
  return {
    scale: { x: { type: 'time' }, y: { nice: true, domainMin: 0 } },
    style: { lineWidth: LINE_WIDTH },
    slider: {
      x: {
        size: 18,
        trackSize: 5,
        trackFill: 'rgba(148, 163, 184, 0.16)',
        selectionFill: 'rgba(24, 24, 27, 0.18)',
        selectionFillOpacity: 1,
        handleIconSize: 10,
      },
    },
    legend: hideLegend ? false : {
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
          value:
            d.raw == null
              ? tip('latencyAllLost')
              : showRaw(d.raw, d.rawAvg, d.slow)
                ? `${formatMs(d.raw)} (${labels.rawAvg} ${formatMs(d.rawAvg)})`
                : formatMs(d.raw),
        }),
        (d: ChartPoint) => ({ name: tip('latencyTipMin'), value: formatMs(d.min) }),
        (d: ChartPoint) => ({ name: tip('latencyTipMax'), value: formatMs(d.max) }),
        (d: ChartPoint) => ({ name: lossLabel(d.kind, labels), value: lossText(d, labels) }),
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
  hideLegend = false,
  onReady,
}: Props) {
  const { t } = useI18n();
  const plotted = useMemo(() => (smooth ? smoothPoints(points) : points), [points, smooth]);
  const { scale, ...rest } = useMemo(() => {
    const ctx: Ctx = { plotted, names, withDate, hideLegend, tip: t };
    return variant === 'classic' ? classicOptions(ctx) : modernOptions(ctx);
  }, [variant, plotted, names, withDate, hideLegend, t]);
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
