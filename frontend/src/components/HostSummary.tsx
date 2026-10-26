import { Tooltip } from 'antd';
import { splitHosts } from '../utils/connectHosts';

/**
 * A group's connect address(es) the way the rules table shows targets: the
 * first one, "(+N)" when there are more, and the full list on hover.
 */
export default function HostSummary({ value, empty }: { value: string | null | undefined; empty: React.ReactNode }) {
  const hosts = splitHosts(value);
  if (hosts.length === 0) return <>{empty}</>;
  const summary = (
    <span className="rp-mono">{hosts[0]}{hosts.length > 1 ? ` (+${hosts.length - 1})` : ''}</span>
  );
  if (hosts.length === 1) return summary;
  return (
    <Tooltip title={<div>{hosts.map(h => <div key={h} className="rp-mono">{h}</div>)}</div>}>
      {summary}
    </Tooltip>
  );
}
