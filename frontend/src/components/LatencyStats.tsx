import { useI18n } from '../i18n/context';
import { formatLoss, formatMs, formatRetrans } from '../utils/latencyChart';

/** One target's figures over the selected range, as the node page's stats row
 *  shows them. Null means "no reading"; `retrans` null / absent means the data
 *  does not carry the figure, and the cell stays empty. */
export interface StatsRow {
  key: string;
  name: string;
  /** Set instead of figures (a target this node cannot probe). */
  note?: string;
  /** The newest minute's mean; `latestAllLost` when that minute answered nothing. */
  latest: number | null;
  latestAllLost?: boolean;
  avg: number | null;
  min: number | null;
  max: number | null;
  loss: number | null;
  retrans: number | null;
}

/**
 * A compact table above the trend chart: per target, the newest reading and the
 * range's average (retransmits left out), fastest, slowest, loss and retransmit
 * share. Numbers use tabular figures so the columns line up; labels are muted.
 * Only what the API already returns: no percentiles.
 */
export default function LatencyStats({ rows, testId }: { rows: StatsRow[]; testId?: string }) {
  const { t } = useI18n();
  return (
    <div className="rp-lat-stats-wrap" data-testid={testId}>
      <table className="rp-lat-stats">
        <thead>
          <tr>
            <th scope="col">{t('latencyStatsTarget')}</th>
            <th scope="col">{t('latencyCardLatest')}</th>
            <th scope="col">{t('latencyCardAvg')}</th>
            <th scope="col">{t('latencyStatsMin')}</th>
            <th scope="col">{t('latencyStatsMax')}</th>
            <th scope="col">{t('latencyCardLoss')}</th>
            <th scope="col">{t('latencyCardRetrans')}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) =>
            r.note ? (
              <tr key={r.key} data-testid={`latency-stats-row-${r.key}`}>
                <th scope="row">{r.name}</th>
                <td colSpan={6} className="rp-lat-stats-note">{r.note}</td>
              </tr>
            ) : (
              <tr key={r.key} data-testid={`latency-stats-row-${r.key}`}>
                <th scope="row">{r.name}</th>
                <td data-col="latest">{r.latestAllLost ? t('latencyAllLost') : formatMs(r.latest)}</td>
                <td data-col="avg">{formatMs(r.avg)}</td>
                <td data-col="min">{formatMs(r.min)}</td>
                <td data-col="max">{formatMs(r.max)}</td>
                <td data-col="loss">{formatLoss(r.loss)}</td>
                <td data-col="retrans">{formatRetrans(r.retrans) ?? ''}</td>
              </tr>
            ),
          )}
        </tbody>
      </table>
    </div>
  );
}
