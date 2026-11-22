import { useRef, useState, type ReactNode } from 'react';
import { Button, Card, Empty, Segmented, Space, Spin, Switch, Tooltip, Typography } from 'antd';
import { DownloadOutlined, UndoOutlined } from '@ant-design/icons';
import { useI18n } from '../i18n/context';
import { useIsMobile } from '../hooks/useIsMobile';
import { useLatencyChartStyle } from '../hooks/useLatencyChartStyle';
import { useLatencySmooth } from '../hooks/useLatencySmooth';
import type { ChartPoint } from '../utils/latencyChart';
import LatencyChart, { CHART_HEIGHT, type ChartVariant, type LatencyChartHandle } from './LatencyChart';

const { Text } = Typography;

/** A box of the chart's height. The chart, its loading veil, its empty state
 *  and a failed load all live in one of these, so the card keeps its size
 *  whatever the data is doing. */
export function ChartBox({ children, veil = false }: { children: ReactNode; veil?: boolean }) {
  return (
    <div className="rp-lat-chart-box" style={{ height: CHART_HEIGHT }}>
      {children}
      {veil ? (
        <div className="rp-lat-chart-veil" data-testid="latency-chart-loading">
          <Spin />
        </div>
      ) : null}
    </div>
  );
}

/** A centred message in the chart box (loading, or a state with nothing to draw). */
export function ChartBoxMessage({ children }: { children: ReactNode }) {
  return <div className="rp-lat-chart-msg">{children}</div>;
}

interface Props {
  /** Left of the card header: what the chart shows. */
  title: ReactNode;
  /** The series picker, when there are more lines than colours. */
  picker?: ReactNode;
  /** Figures for the range, between the header and the chart. */
  stats?: ReactNode;
  points: ChartPoint[];
  colorDomain: string[];
  names: Map<string, string>;
  withDate: boolean;
  /** How often the range re-reads itself, as text ("Refreshes every minute"). */
  autoRefreshNote: string;
  /** A read is in flight over data already on screen: shown as a veil. */
  loading?: boolean;
  /** Hide the chart's internal legend when an external interactive KPI strip is rendered. */
  hideLegend?: boolean;
  /** File name stem of the PNG download. */
  downloadName: string;
}

/**
 * The trend card both latency pages share: a compact header (what is shown on
 * the left; picker, look switch, smooth switch, refresh note, reset and
 * download on the right), then a chart box of fixed height. The smooth switch
 * only changes how the lines are drawn (see smoothPoints), the look switch
 * picks the chart's option set (modern / classic); both are remembered per
 * browser.
 */
export default function LatencyTrendCard({
  title,
  picker,
  stats,
  points,
  colorDomain,
  names,
  withDate,
  autoRefreshNote,
  loading = false,
  hideLegend = false,
  downloadName,
}: Props) {
  const { t } = useI18n();
  const isMobile = useIsMobile();
  const [smooth, setSmooth] = useLatencySmooth();
  const [variant, setVariant] = useLatencyChartStyle();
  // Bumped to remount the chart, which is how "reset zoom" returns the slider
  // to the full range.
  const [chartKey, setChartKey] = useState(0);
  const chartRef = useRef<LatencyChartHandle | null>(null);
  const hasPoints = points.length > 0;

  return (
    <Card
      size="small"
      className="rp-lat-card"
      title={title}
      extra={
        <Space size={[10, 4]} wrap className="rp-lat-card-tools">
          {isMobile ? null : picker}
          <Segmented<ChartVariant>
            size="small"
            data-testid="latency-chart-style"
            aria-label={t('latencyChartStyle')}
            value={variant}
            onChange={setVariant}
            options={[
              { value: 'modern', label: t('latencyStyleModern') },
              { value: 'classic', label: t('latencyStyleClassic') },
            ]}
          />
          <Tooltip title={t('latencySmoothTip')}>
            <label className="rp-lat-smooth">
              <Switch size="small" checked={smooth} onChange={setSmooth} data-testid="latency-smooth" aria-label={t('latencySmooth')} />
              <span>{t('latencySmooth')}</span>
            </label>
          </Tooltip>
          <Text type="secondary" style={{ fontSize: 12 }}>{autoRefreshNote}</Text>
          <Button type="text" size="small" icon={<UndoOutlined />} onClick={() => setChartKey((k) => k + 1)}>
            {t('latencyChartReset')}
          </Button>
          <Button
            type="text"
            size="small"
            icon={<DownloadOutlined />}
            disabled={!hasPoints}
            onClick={() => chartRef.current?.downloadImage?.(downloadName)}
          >
            {t('latencyChartDownload')}
          </Button>
        </Space>
      }
    >
      {isMobile && picker ? <div style={{ marginBottom: 8 }}>{picker}</div> : null}
      {stats}
      <ChartBox veil={loading}>
        {!hasPoints ? (
          <ChartBoxMessage>
            <Empty description={t('latencyChartEmpty')} image={Empty.PRESENTED_IMAGE_SIMPLE} />
          </ChartBoxMessage>
        ) : (
          <LatencyChart
            // Rebuilt on a look change: the chart keeps some parts of its first
            // configuration (the zoom slider and its preview, the legend layout),
            // so switching in place would leave the old look's slider behind.
            key={`${variant}-${chartKey}`}
            points={points}
            colorDomain={colorDomain}
            names={names}
            withDate={withDate}
            smooth={smooth}
            variant={variant}
            hideLegend={hideLegend}
            onReady={(chart) => {
              chartRef.current = chart;
            }}
          />
        )}
      </ChartBox>
    </Card>
  );
}
