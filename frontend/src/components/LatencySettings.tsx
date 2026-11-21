import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Button, Empty, Input, InputNumber, Popconfirm, Select, Space, Spin, Tooltip, Typography, message } from 'antd';
import { ClearOutlined, DeleteOutlined, HolderOutlined, PlusOutlined } from '@ant-design/icons';
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import api from '../api/client';
import type { ApiEnvelope, LatencyConfig } from '../api/types';
import { useI18n } from '../i18n/context';
import {
  CARRIERS,
  KINDS,
  LATENCY_DEFAULT_INTERVAL,
  LATENCY_MAX_INTERVAL,
  LATENCY_MAX_NAME,
  LATENCY_MAX_TARGETS,
  LATENCY_MIN_INTERVAL,
  intervalValid,
  rowError,
  toPayload,
  type LatencyKind,
  type TargetRow,
} from '../utils/latency';

const { Text } = Typography;

const CARRIER_KEY = {
  telecom: 'carrierTelecom',
  unicom: 'carrierUnicom',
  mobile: 'carrierMobile',
  other: 'carrierOther',
} as const;

const DEFAULT_TCP_PORT = 443;

function rowsFrom(cfg: LatencyConfig): TargetRow[] {
  return cfg.targets.map((t) => ({
    key: `id-${t.id}`,
    id: t.id,
    name: t.name,
    carrier: t.carrier,
    kind: t.kind,
    host: t.host,
    port: t.port,
  }));
}

interface RowProps {
  row: TargetRow;
  index: number;
  showErrors: boolean;
  onChange: (key: string, patch: Partial<TargetRow>) => void;
  onRemove: (key: string) => void;
}

/** One editable target: a drag handle, its fields, and a delete button. */
function SortableTargetRow({ row, index, showErrors, onChange, onRemove }: RowProps) {
  const { t } = useI18n();
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } =
    useSortable({ id: row.key });
  const err = showErrors ? rowError(row) : null;
  // Which field the error belongs to, so only that one turns red.
  const bad = {
    name: err === 'latencyErrName' || err === 'latencyErrNameLong',
    host: err === 'latencyErrHost',
    port: err === 'latencyErrPort',
  };
  const dragLabel = t('latencyDragSort');
  return (
    <div
      ref={setNodeRef}
      data-testid={`latency-row-${index}`}
      style={{
        transform: CSS.Transform.toString(transform ? { ...transform, x: 0, scaleX: 1, scaleY: 1 } : null),
        transition,
        opacity: isDragging ? 0.8 : undefined,
        position: 'relative',
        zIndex: isDragging ? 10 : undefined,
        display: 'flex',
        flexWrap: 'wrap',
        alignItems: 'flex-start',
        gap: 8,
        padding: '6px 0',
      }}
    >
      <Button
        type="text"
        size="small"
        ref={setActivatorNodeRef}
        {...attributes}
        {...listeners}
        aria-label={dragLabel}
        title={dragLabel}
        icon={<HolderOutlined />}
        style={{ cursor: 'grab', touchAction: 'none', marginTop: 3 }}
      />
      <div>
        <Input
          aria-label={t('name')}
          placeholder={t('name')}
          maxLength={LATENCY_MAX_NAME * 2}
          status={bad.name ? 'error' : undefined}
          value={row.name}
          onChange={(e) => onChange(row.key, { name: e.target.value })}
          style={{ width: 130 }}
        />
      </div>
      <Select
        aria-label={t('latencyCarrier')}
        value={row.carrier}
        onChange={(carrier) => onChange(row.key, { carrier })}
        options={CARRIERS.map((c) => ({ value: c, label: t(CARRIER_KEY[c]) }))}
        style={{ width: 90 }}
      />
      <Select
        aria-label={t('latencyKind')}
        value={row.kind}
        onChange={(kind: LatencyKind) =>
          onChange(row.key, {
            kind,
            // ICMP has no port; TCP needs one, so give it the usual default.
            port: kind === 'icmp' ? null : (row.port ?? DEFAULT_TCP_PORT),
          })
        }
        options={KINDS.map((k) => ({ value: k, label: t(k === 'tcp' ? 'kindTcp' : 'kindIcmp') }))}
        style={{ width: 120 }}
      />
      <Input
        aria-label={t('latencyHost')}
        placeholder={t('latencyHost')}
        status={bad.host ? 'error' : undefined}
        value={row.host}
        onChange={(e) => onChange(row.key, { host: e.target.value })}
        style={{ flex: '1 1 140px', minWidth: 140 }}
      />
      <InputNumber
        aria-label={t('port')}
        placeholder={t('port')}
        min={1}
        max={65535}
        controls={false}
        disabled={row.kind === 'icmp'}
        status={bad.port ? 'error' : undefined}
        value={row.kind === 'icmp' ? null : row.port}
        onChange={(v) => onChange(row.key, { port: v == null ? null : Math.trunc(Number(v)) })}
        style={{ width: 80 }}
      />
      <Tooltip title={t('latencyRemoveTarget')}>
        <Button
          type="text"
          danger
          size="small"
          aria-label={t('latencyRemoveTarget')}
          icon={<DeleteOutlined />}
          onClick={() => onRemove(row.key)}
          style={{ marginTop: 3 }}
        />
      </Tooltip>
      {err ? (
        <Text type="danger" style={{ flexBasis: '100%', fontSize: 12, paddingLeft: 36 }}>
          {t(err)}
        </Text>
      ) : null}
    </div>
  );
}

interface SettingsProps {
  /** The device group whose targets and interval these are. */
  groupId: number;
  /** Called with the group's target count after every successful save. */
  onSaved?: (targets: number) => void;
  /** Called after the group's latency history was cleared (a page that shows
   *  this group's data can reload it). */
  onCleared?: () => void;
}

/**
 * v1.2.21: one device group's latency monitoring — the probe interval and the
 * list of targets every node of the group probes (a group with none probes
 * nothing). Shown in a drawer on the device-group page. Saving replaces the
 * group's whole configuration; targets that keep their row keep their history,
 * a removed one takes its history with it (the hint says so).
 */
export default function LatencySettings({ groupId, onSaved, onCleared }: SettingsProps) {
  const { t } = useI18n();
  const [interval, setIntervalSecs] = useState<number | null>(LATENCY_DEFAULT_INTERVAL);
  const [rows, setRows] = useState<TargetRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [showErrors, setShowErrors] = useState(false);
  const [removedSaved, setRemovedSaved] = useState(false);
  const nextKey = useRef(0);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const res = await api.get<unknown, ApiEnvelope<LatencyConfig>>(`/admin/groups/${groupId}/latency/config`);
      if (res.code !== 0 || !res.data) {
        setLoadFailed(true);
        return;
      }
      setIntervalSecs(res.data.interval_secs);
      setRows(rowsFrom(res.data));
      setRemovedSaved(false);
    } catch {
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, [groupId]);

  useEffect(() => {
    load();
  }, [load]);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return;
    setRows((cur) => {
      const from = cur.findIndex((r) => r.key === active.id);
      const to = cur.findIndex((r) => r.key === over.id);
      return from < 0 || to < 0 ? cur : arrayMove(cur, from, to);
    });
  };

  const add = () => {
    setRows((cur) => {
      if (cur.length >= LATENCY_MAX_TARGETS) return cur;
      nextKey.current += 1;
      return [
        ...cur,
        {
          key: `new-${nextKey.current}`,
          name: '',
          carrier: 'telecom',
          kind: 'tcp',
          host: '',
          port: DEFAULT_TCP_PORT,
        },
      ];
    });
  };

  const change = (key: string, patch: Partial<TargetRow>) =>
    setRows((cur) => cur.map((r) => (r.key === key ? { ...r, ...patch } : r)));

  const remove = (key: string) => {
    setRows((cur) => {
      const gone = cur.find((r) => r.key === key);
      if (gone?.id != null) setRemovedSaved(true);
      return cur.filter((r) => r.key !== key);
    });
  };

  const save = async () => {
    if (!intervalValid(interval)) {
      message.error(t('latencyErrInterval'));
      return;
    }
    setShowErrors(true);
    for (let i = 0; i < rows.length; i += 1) {
      const err = rowError(rows[i]);
      if (err) {
        message.error(t('latencyErrRow').replace('{n}', String(i + 1)).replace('{msg}', t(err)));
        return;
      }
    }
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<LatencyConfig>>(
        `/admin/groups/${groupId}/latency/config`,
        toPayload(interval, rows),
      );
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('settingsSaveFailed'));
        return;
      }
      setIntervalSecs(res.data.interval_secs);
      setRows(rowsFrom(res.data));
      setShowErrors(false);
      setRemovedSaved(false);
      message.success(t('settingsSaved'));
      onSaved?.(res.data.targets.length);
    } catch {
      message.error(t('settingsSaveFailed'));
    } finally {
      setSaving(false);
    }
  };

  // Delete the stored history of every target and node of this group. The
  // targets and settings stay; the server says how many rows went.
  const clearHistory = async () => {
    setClearing(true);
    try {
      const res = await api.delete<unknown, ApiEnvelope<{ deleted: number }>>(
        `/admin/groups/${groupId}/latency/history`,
      );
      if (res.code !== 0) {
        message.error(res.message || t('latencyClearFailed'));
        return;
      }
      const n = res.data?.deleted;
      message.success(n == null ? t('latencyHistoryCleared') : t('latencyHistoryClearedN').replace('{n}', String(n)));
      onCleared?.();
    } catch {
      message.error(t('latencyClearFailed'));
    } finally {
      setClearing(false);
    }
  };

  const full = rows.length >= LATENCY_MAX_TARGETS;

  return (
    <div data-testid="latency-settings">
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12, marginBottom: 12 }}>
        <Text type="secondary" style={{ flex: 1, fontSize: 12 }}>
          {t('latencyMonitorHint')}
        </Text>
        <Button type="primary" data-testid="latency-save" loading={saving} disabled={loading || loadFailed} onClick={save}>{t('save')}</Button>
      </div>
      {loading ? (
        <div style={{ textAlign: 'center', padding: 24 }}><Spin /></div>
      ) : loadFailed ? (
        <Alert
          type="warning"
          showIcon
          message={t('settingsLoadFailed')}
          action={<Button size="small" onClick={load}>{t('refresh')}</Button>}
        />
      ) : (
        <>
          <Space align="center" size={8} wrap style={{ marginBottom: 12, display: 'flex' }}>
            <Text>{t('latencyInterval')}</Text>
            <InputNumber
              aria-label={t('latencyInterval')}
              min={LATENCY_MIN_INTERVAL}
              max={LATENCY_MAX_INTERVAL}
              precision={0}
              status={interval != null && !intervalValid(interval) ? 'error' : undefined}
              value={interval}
              onChange={(v) => setIntervalSecs(v == null ? null : Number(v))}
              style={{ width: 90 }}
            />
            <Text type="secondary" style={{ fontSize: 12 }}>{t('latencyIntervalHint')}</Text>
          </Space>

          <Space align="center" size={8} wrap style={{ marginBottom: 4, display: 'flex' }}>
            <Text strong>{t('latencyTargets')}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {rows.length} / {LATENCY_MAX_TARGETS}
            </Text>
          </Space>

          {rows.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('latencyNoTargetsYet')} />
          ) : (
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
              <SortableContext items={rows.map((r) => r.key)} strategy={verticalListSortingStrategy}>
                {rows.map((r, i) => (
                  <SortableTargetRow
                    key={r.key}
                    row={r}
                    index={i}
                    showErrors={showErrors}
                    onChange={change}
                    onRemove={remove}
                  />
                ))}
              </SortableContext>
            </DndContext>
          )}

          <Space size={12} wrap style={{ marginTop: 8 }}>
            <Button icon={<PlusOutlined />} onClick={add} disabled={full}>
              {t('latencyAddTarget')}
            </Button>
            {full ? <Text type="secondary" style={{ fontSize: 12 }}>{t('latencyTargetLimit')}</Text> : null}
            {removedSaved ? <Text type="warning" style={{ fontSize: 12 }}>{t('latencyDeleteWarning')}</Text> : null}
          </Space>

          <div style={{ marginTop: 24, paddingTop: 16, borderTop: '1px solid var(--ant-color-split, #f0f0f0)' }}>
            <Popconfirm
              title={t('latencyClearHistoryConfirm')}
              okText={t('latencyClearHistory')}
              cancelText={t('cancel')}
              okButtonProps={{ danger: true }}
              disabled={clearing}
              onConfirm={clearHistory}
            >
              <Button danger icon={<ClearOutlined />} loading={clearing} disabled={clearing} data-testid="latency-clear-history">
                {t('latencyClearHistory')}
              </Button>
            </Popconfirm>
          </div>
        </>
      )}
    </div>
  );
}
