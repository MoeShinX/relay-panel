import { useCallback, useEffect, useMemo, useState } from 'react';
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Button, Card, Empty, Result, Segmented, Select, Spin } from 'antd';
import api from '../api/client';
import type { ApiEnvelope, DeviceGroup, LatencyGroupCount } from '../api/types';
import { useI18n } from '../i18n/context';
import { granularityAllowed, type LatencyGranularity, type LatencyRange } from '../utils/latency';
import GroupLatency from './GroupLatency';

export const LAST_LATENCY_GROUP_KEY = 'relaypanel_latency_group';
const SEGMENTED_MAX_GROUPS = 5;

interface MonitoredGroup {
  id: number;
  name: string;
  targets: number;
}

function readStoredGroupId(): number | null {
  try {
    const raw = window.localStorage.getItem(LAST_LATENCY_GROUP_KEY);
    if (!raw) return null;
    const parsed = Number(raw);
    return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
  } catch {
    return null;
  }
}

function writeStoredGroupId(id: number): void {
  try {
    window.localStorage.setItem(LAST_LATENCY_GROUP_KEY, String(id));
  } catch {
    // Storage may be unavailable in private browsing or restricted contexts.
  }
}

function parsePositiveInt(raw: string | null): number | null {
  if (!raw || !/^\d+$/.test(raw.trim())) return null;
  const n = Number(raw.trim());
  return Number.isSafeInteger(n) && n > 0 ? n : null;
}

/**
 * Legacy route `/groups/:groupId/latency` redirects to `/latency?group=<id>`,
 * preserving any other query parameters.
 */
export function LegacyGroupLatencyRedirect() {
  const { groupId } = useParams();
  const [searchParams] = useSearchParams();
  const next = new URLSearchParams(searchParams);
  if (groupId) next.set('group', groupId);
  const qs = next.toString();
  return <Navigate to={qs ? `/latency?${qs}` : '/latency'} replace />;
}

/**
 * Top-level admin Network Monitor page (`/latency`).
 * Lists device groups that have latency targets configured (`/admin/latency/groups`),
 * provides a group switcher when multiple groups are monitored, and embeds
 * `GroupLatency` for the selected group.
 */
export default function NetworkMonitor() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const [groups, setGroups] = useState<MonitoredGroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);

  // Preserve chosen time window across group switches while resetting
  // group-specific state (selected target, focused node, filters) via key={activeId}.
  const [range, setRange] = useState<LatencyRange>('24h');
  const [granularity, setGranularity] = useState<LatencyGranularity>('1h');

  const onRangeChange = useCallback(
    (next: LatencyRange) => {
      setRange(next);
      if (!granularityAllowed(next, granularity)) {
        setGranularity('1h');
      }
    },
    [granularity],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setFailed(false);
    try {
      const [latencyRes, groupsRes] = await Promise.allSettled([
        api.get<unknown, ApiEnvelope<LatencyGroupCount[]>>('/admin/latency/groups'),
        api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups'),
      ]);
      if (latencyRes.status !== 'fulfilled' || latencyRes.value.code !== 0) {
        setFailed(true);
        setGroups([]);
        return;
      }
      const counts = (latencyRes.value.data || []).filter((c) => c.targets > 0);
      const allGroups =
        groupsRes.status === 'fulfilled' && groupsRes.value.code === 0
          ? (groupsRes.value.data || [])
          : [];
      const byId = new Map(allGroups.map((g, idx) => [g.id, { name: g.name, order: idx }]));
      const monitored: MonitoredGroup[] = counts
        .map((c) => ({
          id: c.group_id,
          name: byId.get(c.group_id)?.name || `#${c.group_id}`,
          targets: c.targets,
          order: byId.get(c.group_id)?.order ?? Number.MAX_SAFE_INTEGER,
        }))
        .sort((a, b) => a.order - b.order || a.id - b.id)
        .map(({ id, name, targets }) => ({ id, name, targets }));
      setGroups(monitored);
    } catch {
      setFailed(true);
      setGroups([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const rawGroupParam = searchParams.get('group');
  const urlGroupId = parsePositiveInt(rawGroupParam);

  const activeGroupId = useMemo(() => {
    if (groups.length === 0) return null;
    const has = (id: number | null) => id != null && groups.some((g) => g.id === id);
    if (rawGroupParam != null) {
      return has(urlGroupId) ? urlGroupId : groups[0].id;
    }
    const stored = readStoredGroupId();
    if (has(stored)) return stored;
    return groups[0].id;
  }, [groups, rawGroupParam, urlGroupId]);

  // If URL had an invalid or stale `?group=` param, rewrite it with replace to
  // the fallback group; always persist the resolved active group to localStorage.
  useEffect(() => {
    if (loading || activeGroupId == null) return;
    writeStoredGroupId(activeGroupId);
    if (rawGroupParam != null && urlGroupId !== activeGroupId) {
      const next = new URLSearchParams(searchParams);
      next.set('group', String(activeGroupId));
      setSearchParams(next, { replace: true });
    }
  }, [loading, activeGroupId, rawGroupParam, urlGroupId, searchParams, setSearchParams]);

  const onSelectGroup = (nextId: number) => {
    writeStoredGroupId(nextId);
    const next = new URLSearchParams(searchParams);
    next.set('group', String(nextId));
    setSearchParams(next, { replace: true });
  };

  if (loading) {
    return (
      <div className="rp-lat" data-testid="network-monitor-loading">
        <Card size="small" className="rp-lat-card">
          <div style={{ textAlign: 'center', padding: 64 }}>
            <Spin />
          </div>
        </Card>
      </div>
    );
  }

  if (failed) {
    return (
      <div className="rp-lat" data-testid="network-monitor-failed">
        <Result
          status="warning"
          title={t('networkMonitorLoadFailed')}
          extra={
            <Button type="primary" onClick={load}>
              {t('refresh')}
            </Button>
          }
        />
      </div>
    );
  }

  if (groups.length === 0 || activeGroupId == null) {
    return (
      <div className="rp-lat" data-testid="network-monitor-empty">
        <Result
          icon={<Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />}
          title={t('networkMonitorEmpty')}
          extra={
            <Button type="primary" onClick={() => navigate('/groups')}>
              {t('networkMonitorGoGroups')}
            </Button>
          }
        />
      </div>
    );
  }

  const switcher =
    groups.length > 1 ? (
      <div
        data-testid="monitor-group-switcher"
        style={{
          marginBottom: 14,
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          flexWrap: 'wrap',
        }}
      >
        {groups.length <= SEGMENTED_MAX_GROUPS ? (
          <Segmented
            data-testid="monitor-group-segmented"
            value={activeGroupId}
            onChange={(v) => onSelectGroup(Number(v))}
            options={groups.map((g) => ({
              value: g.id,
              label: g.name,
            }))}
          />
        ) : (
          <Select
            data-testid="monitor-group-select"
            aria-label={t('networkMonitorSelectGroup')}
            placeholder={t('networkMonitorSelectGroup')}
            showSearch
            optionFilterProp="label"
            style={{ minWidth: 260 }}
            value={activeGroupId}
            onChange={(v) => onSelectGroup(Number(v))}
            options={groups.map((g) => ({
              value: g.id,
              label: g.name,
            }))}
          />
        )}
      </div>
    ) : null;

  return (
    <div data-testid="network-monitor-page">
      {switcher}
      <GroupLatency
        key={activeGroupId}
        groupId={activeGroupId}
        hideBack
        range={range}
        granularity={granularity}
        onRangeChange={onRangeChange}
        onGranularityChange={setGranularity}
      />
    </div>
  );
}
