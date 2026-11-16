import { useMemo } from 'react';
import { Line } from '@ant-design/charts';
import { useI18n } from '../i18n/context';
import { LINE_LEGEND_ITEM, LINE_WIDTH, SERIES_COLORS_LIGHT } from '../utils/trafficBuckets';
import {
  formatChartTime,
  formatLoss,
  formatMs,
  lossMarkers,
  type ChartPoint,
} from '../utils/latencyChart';

/** The subset of the chart instance the latency pages use (see @ant-design/plots). */
export interface LatencyChartHandle {
  downloadImage?: (name?: string, type?: string) => string;
}

interface Props {
  points: ChartPoint[];
  /** Line keys in legend order, so a line keeps its colour whatever is shown. */
  colorDomain: string[];
  /** Legend label of each key. */
  names: Map<string, string>;
  /** A range of a day or more needs the date on the axis too. */
  withDate: boolean;
  onReady: (chart: LatencyChartHandle) => void;
}

/**
 * v1.2.21: the line chart both latency pages draw — a node's targets, or a
 * group's nodes for one target. One line per key; the x scale is a time scale
 * over real Dates, so buckets keep their true spacing and a gap in the data
 * shows as a gap. Buckets that lost probes get a marker on the baseline, so a
 * brief drop is as visible as an outage. Zoom is the slider; the page resets it
 * by remounting the chart. The legend toggles lines (the chart's default).
 */
export default function LatencyChart({ points, colorDomain, names, withDate, onReady }: Props) {
  const { t } = useI18n();
  const markers = useMemo(() => lossMarkers(points), [points]);
  return (
    <Line
      /* @ant-design/charts v2 (G2 v5) options. */
      height={340}
      data={points}
      xField="time"
      yField="value"
      colorField="key"
      scale={{
        x: { type: 'time' },
        y: { nice: true },
        color: { domain: colorDomain, range: SERIES_COLORS_LIGHT },
      }}
      style={{ lineWidth: LINE_WIDTH }}
      slider={{ x: {} }}
      legend={{
        color: {
          position: 'top',
          layout: { justifyContent: 'flex-end' },
          ...LINE_LEGEND_ITEM,
          labelFormatter: (k: string) => names.get(k) ?? k,
        },
      }}
      axis={{
        x: {
          title: false,
          labelAutoHide: true,
          labelAutoRotate: false,
          labelFormatter: (d: Date | number) => formatChartTime(new Date(d), withDate),
        },
        y: { title: false, labelFormatter: (v: number) => `${v} ms` },
      }}
      annotations={[
        {
          type: 'point',
          data: markers,
          encode: { x: 'time', y: () => 0 },
          style: { fill: '#ff4d4f', stroke: '#ff4d4f', r: 3, fillOpacity: 0.85 },
          tooltip: false,
        },
      ]}
      tooltip={{
        title: (d: ChartPoint) => formatChartTime(d.time, true),
        items: [
          (d: ChartPoint) => ({ name: `${d.name} · ${t('latencyTipAvg')}`, value: d.value == null ? t('latencyAllLost') : formatMs(d.value) }),
          (d: ChartPoint) => ({ name: t('latencyTipMin'), value: formatMs(d.min) }),
          (d: ChartPoint) => ({ name: t('latencyTipMax'), value: formatMs(d.max) }),
          (d: ChartPoint) => ({ name: t('latencyTipLoss'), value: formatLoss(d.loss) }),
        ],
      }}
      onReady={onReady}
    />
  );
}
