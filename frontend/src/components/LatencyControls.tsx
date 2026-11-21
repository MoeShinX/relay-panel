import { Segmented, Select, Typography } from 'antd';
import { useI18n } from '../i18n/context';
import {
  GRANULARITIES,
  RANGES,
  granularityAllowed,
  type LatencyGranularity,
  type LatencyRange,
} from '../utils/latency';

const { Text } = Typography;

const GRAN_LABEL = {
  auto: 'latencyGranDefault',
  '1m': 'latencyGran1m',
  '5m': 'latencyGran5m',
  '1h': 'latencyGran1h',
} as const;

const RANGE_LABEL = {
  '1h': 'latencyRange1h',
  '6h': 'latencyRange6h',
  '24h': 'latencyRange24h',
  '7d': 'latencyRange7d',
  '30d': 'latencyRange30d',
} as const;

interface Props {
  range: LatencyRange;
  granularity: LatencyGranularity;
  onRange: (r: LatencyRange) => void;
  onGranularity: (g: LatencyGranularity) => void;
}

/** The time-range switch and the granularity picker of the latency pages, at
 *  the same height, wrapping together on a narrow screen. A granularity the
 *  range cannot answer (minutes over 7 or 30 days) is disabled; the page falls
 *  back to the default when the range moves under it. */
export default function LatencyControls({ range, granularity, onRange, onGranularity }: Props) {
  const { t } = useI18n();
  return (
    <div className="rp-lat-controls">
      <Segmented<LatencyRange>
        data-testid="latency-range"
        value={range}
        onChange={onRange}
        options={RANGES.map((r) => ({ value: r, label: t(RANGE_LABEL[r]) }))}
      />
      <span className="rp-lat-gran">
        <Text type="secondary">{t('latencyGranularity')}</Text>
        <Select
          aria-label={t('latencyGranularity')}
          data-testid="latency-granularity"
          value={granularity}
          onChange={onGranularity}
          style={{ width: 110 }}
          options={GRANULARITIES.map((g) => ({
            value: g,
            label: t(GRAN_LABEL[g]),
            disabled: !granularityAllowed(range, g),
          }))}
        />
      </span>
    </div>
  );
}
