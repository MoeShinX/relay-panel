import { Select, Typography } from 'antd';
import { useI18n } from '../i18n/context';
import { useIsMobile } from '../hooks/useIsMobile';
import { MAX_SERIES } from '../utils/trafficBuckets';

const { Text } = Typography;

interface Props {
  /** Every line the chart could draw, in the chart's order. */
  options: { value: string; label: string }[];
  /** The lines drawn now. */
  value: string[];
  /** Nobody has picked yet: `value` is the automatic top ten. */
  automatic: boolean;
  /** null: back to automatic (the select was cleared). */
  onChange: (next: string[] | null) => void;
  placeholder: string;
  testId?: string;
}

/**
 * "Show which lines" for a line chart with more series than the palette has
 * colours. At most MAX_SERIES can be ticked, so the drawn lines always get ten
 * distinct colours; at the cap the other options are disabled. Rendered by the
 * chart only when it has more than MAX_SERIES series.
 */
export default function SeriesPicker({ options, value, automatic, onChange, placeholder, testId }: Props) {
  const { t } = useI18n();
  const mobile = useIsMobile();
  const full = value.length >= MAX_SERIES;
  return (
    <Select
      mode="multiple"
      size="small"
      allowClear
      virtual={false}
      maxTagCount="responsive"
      data-testid={testId}
      aria-label={placeholder}
      placeholder={placeholder}
      optionFilterProp="label"
      value={value}
      onChange={(v: string[]) => onChange(v.length > 0 ? v : null)}
      options={options.map((o) => ({ ...o, disabled: full && !value.includes(o.value) }))}
      style={mobile ? { width: '100%' } : { width: 280 }}
      popupRender={(menu) => (
        <>
          {menu}
          <div style={{ padding: '6px 12px 2px' }}>
            <Text type="secondary" style={{ fontSize: 12, display: 'block' }} data-testid="series-picker-hint">
              {t('seriesPickMax')}
            </Text>
            {automatic ? (
              <Text type="secondary" style={{ fontSize: 12, display: 'block' }} data-testid="series-picker-auto">
                {t('seriesPickAuto')}
              </Text>
            ) : null}
          </div>
        </>
      )}
    />
  );
}
