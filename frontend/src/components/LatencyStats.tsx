import { useI18n } from '../i18n/context';
import type { LatencyKind } from '../utils/latency';
import { formatLoss, formatMs, slowText } from '../utils/latencyChart';

/** One target's figures over the selected range, as the node page's stats row
 *  shows them. Null means "no reading". `slow` null / absent means the data
 *  does not carry the figure: a TCP row then says "—" (never 0%), an ICMP row
 *  has no such figure at all and the cell stays empty. */
export interface StatsRow {
  key: string;
  name: string;
  kind?: LatencyKind;
  /** Set instead of figures (a target this node cannot probe). */
  note?: string;
  /** The newest minute's mean; `latestAllLost` when that minute answered nothing. */
  latest: number | null;
  latestAllLost?: boolean;
  /** The newest sample is too old to be called current: shown as "—" and not
   *  counted as the current state of the line. */
  latestStale?: boolean;
  /** Tooltip of a stale cell ("No new data · last 21:40"). */
  latestStaleTip?: string;
  /** Filtered: slow handshakes left out where the node reported them. */
  avg: number | null;
  min: number | null;
  max: number | null;
  /** Raw: every answered probe, slow handshakes included. */
  rawAvg?: number | null;
  rawMax?: number | null;
  /** Failed probes (TCP) or packet loss (ICMP), in percent. */
  loss: number | null;
  slow: number | null;
}

/**
 * A compact table above the trend chart: per target, the newest reading, the
 * range's average and maximum without the slow handshakes, the same two with
 * them ("raw"), the fastest, the failed / lost share and the slow-handshake
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
            <th scope="col">{t('latencyStatsAvgFiltered')}</th>
            <th scope="col">{t('latencyStatsMin')}</th>
            <th scope="col">{t('latencyStatsMaxFiltered')}</th>
            <th scope="col" title={t('latencyStatsRawTip')}>{t('latencyStatsRawAvg')}</th>
            <th scope="col" title={t('latencyStatsRawTip')}>{t('latencyStatsRawMax')}</th>
            <th scope="col" title={t('latencyStatsFailLossTip')}>{t('latencyStatsFailLoss')}</th>
            <th scope="col">{t('latencySlowFull')}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) =>
            r.note ? (
              <tr key={r.key} data-testid={`latency-stats-row-${r.key}`}>
                <th scope="row">{r.name}</th>
                <td colSpan={8} className="rp-lat-stats-note">{r.note}</td>
              </tr>
            ) : (
              <tr key={r.key} data-testid={`latency-stats-row-${r.key}`}>
                <th scope="row">{r.name}</th>
                <td data-col="latest" data-stale={r.latestStale ? 'true' : undefined} title={r.latestStale ? r.latestStaleTip : undefined}>
                  {r.latestStale ? '—' : r.latestAllLost ? t('latencyAllLost') : formatMs(r.latest)}
                </td>
                <td data-col="avg">{formatMs(r.avg)}</td>
                <td data-col="min">{formatMs(r.min)}</td>
                <td data-col="max">{formatMs(r.max)}</td>
                <td data-col="raw-avg">{formatMs(r.rawAvg ?? r.avg)}</td>
                <td data-col="raw-max">{formatMs(r.rawMax ?? r.max)}</td>
                <td data-col="loss" title={r.kind === 'tcp' ? t('latencyCardFailed') : t('latencyCardLoss')}>
                  {formatLoss(r.loss)}
                </td>
                <td data-col="slow">{slowText(r.slow, r.kind) ?? ''}</td>
              </tr>
            ),
          )}
        </tbody>
      </table>
    </div>
  );
}
