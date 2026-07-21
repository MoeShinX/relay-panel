import { Card, Segmented, Select, Space, Spin, Empty, message } from 'antd';
import { Column } from '@ant-design/charts';
import { useCallback, useEffect, useMemo, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, TrafficHistoryResponse } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';
import { foldBuckets } from '../utils/trafficBuckets';
import type { TrafficPoint } from '../utils/trafficBuckets';

/** The panel's indigo accent (--rp-primary). Validated against the light
 *  surface with the palette checker: lightness band, chroma floor and 3:1
 *  contrast all pass. Single series → one hue, no legend (the title names it). */
const SERIES_COLOR = '#6366f1';

type Range = '1d' | '7d' | '30d';

interface Props {
  /** Rules for the admin drill-down Select. Omit to hide the filter (the
   *  Account page: the API pins non-admins to their own uid anyway). */
  rules?: { id: number; name: string }[];
}

/**
 * v1.2.0: traffic-history chart (1d hourly / 7d / 30d daily).
 *
 * The primary series is BILLED traffic — the same number the quota deducts, so
 * this chart can never disagree with "已用流量". Real (unrated) up/down live in
 * the tooltip; on a rate≠1 line the two visibly differ, and showing only the
 * real bytes would read as the panel over-charging.
 *
 * Buckets are stored in UTC; grouping into days happens HERE, in the viewer's
 * timezone — a server-side UTC day starts at 08:00 for a CST viewer and would
 * misfile "yesterday's" traffic.
 */
export default function TrafficChart({ rules }: Props) {
  const { t } = useI18n();
  const [range, setRange] = useState<Range>('7d');
  const [ruleId, setRuleId] = useState<number | undefined>(undefined);
  const [resp, setResp] = useState<TrafficHistoryResponse | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const qs = new URLSearchParams({ range });
      if (ruleId !== undefined) qs.set('rule_id', String(ruleId));
      const res = await api.get<unknown, ApiEnvelope<TrafficHistoryResponse>>(
        `/stats/traffic?${qs}`,
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
  }, [range, ruleId, t]);

  useEffect(() => { load(); }, [load]);

  /** Zero-fill + viewer-timezone bucketing. Lives in utils/trafficBuckets so
   *  the timezone handling — the part that actually breaks — is unit-tested
   *  rather than only observable by squinting at a rendered canvas. */
  const points: TrafficPoint[] = useMemo(
    () => (resp ? foldBuckets(resp.buckets, resp.since, range === '1d') : []),
    [resp, range],
  );

  const hasData = points.some((p) => p.billed > 0 || p.real_upload > 0 || p.real_download > 0);

  return (
    <Card
      title={t('trafficHistory')}
      style={{ marginTop: 16 }}
      extra={
        <Space wrap>
          {rules && rules.length > 0 && (
            <Select
              style={{ minWidth: 160 }}
              allowClear
              placeholder={t('allRules')}
              value={ruleId}
              onChange={(v: number | undefined) => setRuleId(v)}
              options={rules.map((r) => ({ value: r.id, label: r.name }))}
            />
          )}
          <Segmented
            value={range}
            onChange={(v) => setRange(v as Range)}
            options={[
              { value: '1d', label: t('range1d') },
              { value: '7d', label: t('range7d') },
              { value: '30d', label: t('range30d') },
            ]}
          />
        </Space>
      }
    >
      <Spin spinning={loading}>
        {!loading && !hasData ? (
          <Empty description={t('noTrafficYet')} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <Column
            /* @ant-design/charts v2 (G2 v5) options — NOT the v1 G2Plot shape
               (columnStyle/xAxis/yAxis), which v2 ignores. Note for anyone
               debugging "the chart is blank" in an embedded/背景 tab: G2 v5
               renders on requestAnimationFrame, and a hidden tab never fires
               rAF — the canvas stays empty with zero errors. Check
               document.visibilityState before suspecting this config. */
            height={260}
            data={points}
            xField="label"
            yField="billed"
            legend={false}
            style={{ fill: SERIES_COLOR, radiusTopLeft: 4, radiusTopRight: 4 }}
            axis={{
              // 30 days of labels don't fit; let the axis thin them out.
              x: { labelAutoHide: true, labelAutoRotate: false, title: false },
              y: {
                title: false,
                labelFormatter: (v: number) => formatBytes(v),
              },
            }}
            tooltip={{
              title: (d: TrafficPoint) => d.label,
              items: [
                (d: TrafficPoint) => ({
                  name: t('billedTraffic'),
                  value: formatBytes(d.billed),
                  color: SERIES_COLOR,
                }),
                (d: TrafficPoint) => ({ name: t('realUpload'), value: formatBytes(d.real_upload) }),
                (d: TrafficPoint) => ({ name: t('realDownload'), value: formatBytes(d.real_download) }),
              ],
            }}
          />
        )}
      </Spin>
    </Card>
  );
}
