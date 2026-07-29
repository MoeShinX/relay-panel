import { Card, Segmented, Space, Spin, Empty, message } from 'antd';
import { Line } from '@ant-design/charts';
import { useCallback, useEffect, useMemo, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, NodeMetricsResponse } from '../api/types';
import { useI18n } from '../i18n/context';
import { parseUtcBucket, SERIES_COLORS_LIGHT } from '../utils/trafficBuckets';
import type { Metric } from '../utils/nodeMetrics';
import { formatMetric, multiNodeGroupNames, seriesName } from '../utils/nodeMetrics';

type Range = '1d' | '7d';

/** A plotted point: one node's value at one bucket. */
interface Point {
  label: string;
  node: string;
  value: number;
}

/**
 * v1.2.4: node CPU / memory / connection history.
 *
 * Node status is a snapshot each report overwrites, so "why was it slow last
 * night" previously had no data behind it. This charts the hourly rollup.
 *
 * ONE chart with a metric switch rather than three stacked charts: three of
 * these below the traffic chart would push everything else off the screen, and
 * you look at one metric at a time anyway.
 *
 * Lines are per node and never aggregated — the mean CPU of three nodes hides
 * the one machine that is pinned at 100%, which is precisely the thing you
 * opened this chart to find.
 *
 * The series plots the hourly AVERAGE; the tooltip carries the peak beside it.
 * An average alone flattens exactly the spike that caused the stall.
 */
export default function NodeMetricsChart() {
  const { t } = useI18n();
  const [range, setRange] = useState<Range>('1d');
  const [metric, setMetric] = useState<Metric>('cpu');
  const [resp, setResp] = useState<NodeMetricsResponse | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.get<unknown, ApiEnvelope<NodeMetricsResponse>>(
        `/stats/node-metrics?range=${range}`,
      );
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('loadFailed'));
        return;
      }
      setResp(res.data);
    } catch {
      message.error(t('loadFailed'));
    } finally {
      setLoading(false);
    }
  }, [range, t]);

  useEffect(() => { load(); }, [load]);

  /** Peak per (bucket, node), so the tooltip can show it next to the average. */
  const peaks = useMemo(() => {
    const m = new Map<string, number>();
    for (const b of resp?.buckets ?? []) {
      const peak = metric === 'cpu' ? b.cpu_max : metric === 'mem' ? b.mem_max : b.conn_max;
      m.set(`${b.bucket}\u0000${b.node_id}`, peak);
    }
    return m;
  }, [resp, metric]);

  const multiNodeGroups = useMemo(() => multiNodeGroupNames(resp?.buckets ?? []), [resp]);

  const points: Point[] = useMemo(() => {
    if (!resp) return [];
    return resp.buckets.map(b => {
      // Buckets are stored UTC; render them in the viewer's timezone, the same
      // reason the traffic chart folds client-side.
      const d = parseUtcBucket(b.bucket);
      const label = range === '1d'
        ? `${String(d.getHours()).padStart(2, '0')}:00`
        : `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:00`;
      const value = metric === 'cpu' ? b.cpu_avg : metric === 'mem' ? b.mem_avg : b.conn_avg;
      return {
        label,
        node: seriesName(b, multiNodeGroups),
        value,
        _key: `${b.bucket}\u0000${b.node_id}`,
      } as Point & { _key: string };
    });
  }, [resp, metric, range, multiNodeGroups]);

  const hasData = points.length > 0;

  const fmt = (v: number) => formatMetric(metric, v);

  return (
    <Card
      title={t('nodeMetricsHistory')}
      style={{ marginTop: 16 }}
      extra={
        <Space wrap>
          <Segmented
            value={metric}
            onChange={(v) => setMetric(v as Metric)}
            options={[
              { value: 'cpu', label: t('metricCpu') },
              { value: 'mem', label: t('metricMem') },
              { value: 'conn', label: t('metricConn') },
            ]}
          />
          <Segmented
            value={range}
            onChange={(v) => setRange(v as Range)}
            options={[
              { value: '1d', label: t('range1d') },
              { value: '7d', label: t('range7d') },
            ]}
          />
        </Space>
      }
    >
      <Spin spinning={loading}>
        {!loading && !hasData ? (
          <Empty description={t('noNodeMetricsYet')} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <Line
            /* @ant-design/charts v2 (G2 v5) options — NOT the v1 G2Plot shape.
               If this renders blank in an embedded tab, check
               document.visibilityState first: G2 draws on rAF and a hidden tab
               never fires it. */
            height={260}
            data={points}
            xField="label"
            yField="value"
            colorField="node"
            scale={{ color: { range: SERIES_COLORS_LIGHT } }}
            legend={{ color: { position: 'top', layout: { justifyContent: 'flex-end' } } }}
            axis={{
              x: { labelAutoHide: true, labelAutoRotate: false, title: false },
              y: { title: false, labelFormatter: fmt },
            }}
            tooltip={{
              title: (d: Point) => d.label,
              items: [
                (d: Point & { _key?: string }) => ({
                  name: `${d.node} · ${t('metricAvg')}`,
                  value: fmt(d.value),
                }),
                (d: Point & { _key?: string }) => ({
                  name: t('metricPeak'),
                  value: fmt(peaks.get(d._key ?? '') ?? d.value),
                }),
              ],
            }}
          />
        )}
      </Spin>
    </Card>
  );
}
