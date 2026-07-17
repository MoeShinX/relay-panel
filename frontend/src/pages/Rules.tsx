import { Table, Button, Modal, Form, Input, InputNumber, Select, Space, message, Popconfirm, Tag, Alert, Typography, Dropdown, Switch, Tabs, Spin, Tooltip } from 'antd';
import type { MenuProps } from 'antd';
import { PlusOutlined, ReloadOutlined, EditOutlined, ApiOutlined, CopyOutlined, DownloadOutlined, UploadOutlined, PauseCircleOutlined, PlayCircleOutlined, DeleteOutlined, ArrowUpOutlined, ArrowDownOutlined, MedicineBoxOutlined, QuestionCircleOutlined, ThunderboltOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, ForwardRule, DeviceGroup, User, UserSelf, RuleTargetInput, DiagnoseResponse, NodeDiagnoseStatus, DiagnoseTargetResult, SharedGroupSummary, RestartResponse } from '../api/types';
import { MIN_AUTO_RESTART_MINUTES } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';
import { useAuth } from '../auth/useAuth';
import { asValidatedEntry, buildExportJSON, parseDest, ruleTargets, validateImportEntry } from '../utils/rulesIO';

const { Text } = Typography;
const { TextArea } = Input;

function targetSummary(rule: ForwardRule): string {
  const targets = ruleTargets(rule).filter(t => t.enabled);
  const first = targets[0] ?? ruleTargets(rule)[0];
  if (!first) return '-';
  const suffix = targets.length > 1 ? ` (+${targets.length - 1})` : '';
  return `${first.host}:${first.port}${suffix}`;
}

function formTargets(values: { targets?: RuleTargetInput[]; target_addr?: string; target_port?: number }): RuleTargetInput[] {
  const targets = values.targets ?? [];
  return targets.map(t => ({ host: t.host?.trim() ?? '', port: Number(t.port), enabled: t.enabled !== false }));
}

function payloadWithTargets<T extends Record<string, unknown>>(values: T & { targets?: RuleTargetInput[]; target_addr?: string; target_port?: number }) {
  const targets = formTargets(values);
  if (targets.length < 1) {
    throw new Error('targets must have at least one entry');
  }
  const first = targets[0];
  return {
    ...values,
    target_addr: first.host,
    target_port: first.port,
    targets,
  };
}

/** Trigger a browser download of a text file. */
function downloadText(filename: string, text: string) {
  const blob = new Blob([text], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

export default function Rules() {
  const { t } = useI18n();
  const { isAdmin, user } = useAuth();
  const [searchParams] = useSearchParams();
  // v0.4.20: admin can manage another user's rules via /rules?owner_uid=X.
  const filterOwnerUid: number | null = isAdmin
    ? (parseInt(searchParams.get('owner_uid') || '') || null)
    : null;
  const [rules, setRules] = useState<ForwardRule[]>([]);
  const [groups, setGroups] = useState<DeviceGroup[]>([]);
  // v0.4.11 PR3: shared inbound groups (admin-owned) for regular users.
  const [sharedGroups, setSharedGroups] = useState<SharedGroupSummary[]>([]);
  // v0.4.12 PR1: true when /groups/shared failed to load (DB error). A regular
  // user then sees a load-failure notice and rule creation is blocked, instead
  // of a misleading empty inbound dropdown.
  const [sharedLoadFailed, setSharedLoadFailed] = useState(false);
  const [users, setUsers] = useState<User[]>([]);
  // v1.0.7: a regular user's own traffic quota (admins read each owner's quota
  // from `users` instead). Used to flag rules whose owner is out of traffic —
  // those rules stop forwarding even though their `paused` flag stays false.
  const [selfQuota, setSelfQuota] = useState<{ used: number; limit: number } | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState('');
  const [importGroupId, setImportGroupId] = useState<number | undefined>(undefined);
  const [importResults, setImportResults] = useState<string[]>([]);
  const [editing, setEditing] = useState<ForwardRule | null>(null);
  const [loading, setLoading] = useState(false);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();
  // v0.4.8: rule diagnosis modal state.
  const [diagnosing, setDiagnosing] = useState<ForwardRule | null>(null);
  const [diagnoseLoading, setDiagnoseLoading] = useState(false);
  const [diagnoseResult, setDiagnoseResult] = useState<DiagnoseResponse | null>(null);
  // v0.4.9: group-name column + filter. selectedGroup === null means "all".
  // (Explicit null, not !selectedGroup, so a future id of 0 wouldn't be falsy.)
  const [selectedGroup, setSelectedGroup] = useState<number | null>(null);
  const [selectedRowKeys, setSelectedRowKeys] = useState<number[]>([]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // v0.4.10: /admin/users is admin-only and NOT in the main Promise.all —
      // a regular user would 403 and block the whole page load. The owner
      // column / selector are hidden for non-admins (they only ever own their
      // own rules), so the users list is fetched separately and only when
      // isAdmin. A failure here leaves users empty but rules/groups still load.
      // v0.4.20: admin can filter rules by owner_uid.
      // Admin on own page → filter to their own rules; admin viewing another
      // user → use filterOwnerUid; regular user → backend filters automatically.
      const ownerUid = filterOwnerUid ?? (isAdmin ? (user?.id ?? null) : null);
      const rulesUrl = ownerUid ? `/rules?owner_uid=${ownerUid}` : '/rules';
      const [r, g] = await Promise.all([
        api.get<unknown, ApiEnvelope<ForwardRule[]>>(rulesUrl),
        api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups'),
      ]);
      setRules(r.data || []);
      setGroups(g.data || []);
      if (isAdmin) {
        try {
          const u = await api.get<unknown, ApiEnvelope<User[]>>('/admin/users');
          setUsers(u.data || []);
        } catch {
          // Non-fatal: owner column falls back to "#uid" labels.
          setUsers([]);
        }
        setSelfQuota(null);
      } else {
        setUsers([]);
        // v1.0.7: a regular user only ever sees their own rules, so one /user/me
        // read gives the quota needed to flag all of them. Non-fatal on failure.
        try {
          const me = await api.get<unknown, ApiEnvelope<UserSelf>>('/user/me');
          setSelfQuota(me.data ? { used: me.data.traffic_used, limit: me.data.traffic_limit } : null);
        } catch {
          setSelfQuota(null);
        }
      }
      // v0.4.12 PR1: shared inbound groups (admin-owned) for regular users.
      // The endpoint wraps the payload in ApiResponse — a non-zero code is a
      // load failure (NOT an empty "no lines" state), so we flag it and block
      // rule creation rather than show an empty inbound dropdown.
      // Admins get an empty list (they manage groups directly).
      if (!isAdmin) {
        try {
          const sg = await api.get<unknown, ApiEnvelope<SharedGroupSummary[]>>('/groups/shared');
          if (sg.code !== 0) {
            setSharedLoadFailed(true);
            setSharedGroups([]);
          } else {
            setSharedLoadFailed(false);
            setSharedGroups(sg.data || []);
          }
        } catch {
          setSharedLoadFailed(true);
          setSharedGroups([]);
        }
      } else {
        setSharedLoadFailed(false);
        setSharedGroups([]);
      }
    } finally { setLoading(false); }
  }, [filterOwnerUid, isAdmin, user?.id]);

  useEffect(() => { load(); }, [load]);

  // User lookup map for the "owner" column.
  const userMap = new Map(users.map(u => [u.id, u.username]));
  // v1.0.7: owner-quota lookup for the "traffic exhausted" status tag. Admins
  // resolve each rule's owner from `users`; a regular user uses their own quota
  // (their rules are all self-owned). traffic_limit === 0 means unlimited.
  const userById = useMemo(() => new Map(users.map(u => [u.id, u])), [users]);
  const ruleOverQuota = (r: ForwardRule): boolean => {
    if (isAdmin) {
      const u = userById.get(r.uid);
      return !!u && u.traffic_limit > 0 && u.traffic_used >= u.traffic_limit;
    }
    return !!selfQuota && selfQuota.limit > 0 && selfQuota.used >= selfQuota.limit;
  };
  // v0.4.9: group lookup map for the "group name" column + filter. Memoized so
  // the column render + filter options share one derivation.
  const groupMap = useMemo(() => new Map(groups.map(g => [g.id, g])), [groups]);
  // v1.0.8: group-name + listen-IP lookup for the rule columns. A regular user
  // does NOT own the (admin-owned) device groups, so GET /groups returns none
  // for them and the columns rendered "未知分组 / 未配置". Their AUTHORIZED
  // groups come from /groups/shared (SharedGroupSummary, which carries name +
  // connect_host) — merge both so name/IP resolve for admins and users alike.
  const groupInfo = useMemo(() => {
    const m = new Map<number, { name: string; connect_host: string }>();
    for (const g of groups) m.set(g.id, { name: g.name, connect_host: g.connect_host });
    for (const g of sharedGroups) {
      if (!m.has(g.id)) m.set(g.id, { name: g.name, connect_host: g.connect_host });
    }
    return m;
  }, [groups, sharedGroups]);
  // The rules actually shown: filtered by the selected inbound group, or all
  // when selectedGroup === null. Computed once so the table + count stay in sync.
  const visibleRules = useMemo(
    () => rules.filter(r => selectedGroup === null || r.device_group_in === selectedGroup),
    [rules, selectedGroup],
  );

  const handleCreate = async (values: {
    name: string; listen_port: number | null; protocol: string;
    public_transport?: string;
    ws_path?: string;
    device_group_in: number; device_group_out: number | null;
    forward_mode: string;
    target_addr?: string; target_port?: number; targets?: RuleTargetInput[];
    load_balance_strategy?: string;
    upload_limit_mbps?: number;
    download_limit_mbps?: number;
    tunnel_profile_id?: number | null;
    owner_uid?: number | null;
  }) => {
    // v0.4.20: WS/TLS tunnel hidden — always raw, no profile.
    // v0.4.20: forward_mode always direct, no outbound group.
    // v0.4.20: owner determined by entry point (filterOwnerUid from URL).
    const owner_uid = filterOwnerUid ?? undefined;
    // v0.4.20: reject empty targets before payload generation.
    if (formTargets(values).length < 1) {
      message.error(t('targetRequired'));
      return;
    }
    const payload = payloadWithTargets({
      ...values,
      listen_port: values.listen_port || null,
      public_transport: 'raw',
      tunnel_profile_id: null,
      forward_mode: 'direct',
      route_mode: 'direct',
      device_group_out: null,
      owner_uid,
    });
    try {
      const res = await api.post<unknown, ApiEnvelope<null>>('/rules', payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('ruleCreated'));
      setCreateOpen(false);
      createForm.resetFields();
      load();
    } catch { message.error(t('failedCreateRule')); }
  };

  const handleEdit = (r: ForwardRule) => {
    setEditing(r);
    editForm.setFieldsValue({
      name: r.name, listen_port: r.listen_port, protocol: r.protocol,
      device_group_in: r.device_group_in,
      target_addr: r.target_addr, target_port: r.target_port,
      targets: ruleTargets(r),
      load_balance_strategy: r.load_balance_strategy ?? 'first',
      upload_limit_mbps: r.upload_limit_mbps ?? 0,
      download_limit_mbps: r.download_limit_mbps ?? 0,
      max_connections: r.max_connections ?? 0,
      auto_restart_minutes: r.auto_restart_minutes ?? 0,
    });
    setEditOpen(true);
  };

  /** Copy: open the create modal pre-filled with the rule's values, but with
   *  a "-copy" name suffix and no listen_port (auto-assign). */
  const handleCopy = (r: ForwardRule) => {
    createForm.setFieldsValue({
      name: `${r.name}-copy`,
      listen_port: null,
      protocol: r.protocol,
      device_group_in: r.device_group_in,
      target_addr: r.target_addr,
      target_port: r.target_port,
      targets: ruleTargets(r),
      load_balance_strategy: r.load_balance_strategy ?? 'first',
      upload_limit_mbps: r.upload_limit_mbps ?? 0,
      download_limit_mbps: r.download_limit_mbps ?? 0,
      // v1.2.0: max_connections / auto_restart_minutes are NOT carried over —
      // they're edit-only (the create path can't store them), so the copy starts
      // uncapped and the user sets them via Edit if wanted.
    });
    setCreateOpen(true);
  };

  /** Export all rules as JSON download. */
  const handleExportAll = () => {
    downloadText(`relaypanel-rules-${new Date().toISOString().slice(0, 10)}.json`, buildExportJSON(rules));
    message.success(t('exported'));
  };

  /** Export only the currently-selected rules as JSON download. */
  const handleExportSelected = () => {
    const selected = rules.filter(r => selectedRowKeys.includes(r.id));
    if (selected.length === 0) return;
    downloadText(`relaypanel-rules-selected-${new Date().toISOString().slice(0, 10)}.json`, buildExportJSON(selected));
    message.success(t('exported'));
  };

const IMPORT_DEFAULTS = {
  protocol: 'tcp_udp' as const,
  public_transport: 'raw' as const,
  forward_mode: 'direct' as const,
  route_mode: 'direct' as const,
  load_balance_strategy: 'first' as const,
  upload_limit_mbps: 0,
  download_limit_mbps: 0,
};
  const handleImport = async () => {
    if (!importGroupId) {
      message.error(t('selectInboundGroup'));
      return;
    }
    let parsed: unknown;
    try { parsed = JSON.parse(importText); } catch {
      message.error(t('importInvalidJson')); return;
    }
    const entries = Array.isArray(parsed) ? parsed : [parsed];
    if (entries.length === 0) {
      message.error(t('importInvalidFormat')); return;
    }
    const results: string[] = [];
    for (const e of entries) {
      const label = (typeof e === 'object' && e !== null && !Array.isArray(e))
        ? String((e as { name?: unknown })['name'] ?? '?')
        : '?';
      const err = validateImportEntry(e);
      if (err) { results.push(`❌ ${label}: ${err}`); continue; }
      const entry = asValidatedEntry(e);
      const targets = entry.dest.map(d => {
        // validateImportEntry already rejected any unparseable dest above, so
        // parseDest is non-null here; fall back to a safe default defensively.
        const p = parseDest(d) ?? { host: '', port: 0 };
        return { host: p.host, port: p.port, enabled: true };
      });
      const first = targets[0];
      try {
        const res = await api.post<unknown, ApiEnvelope<null>>('/rules', {
          name: entry.name,
          listen_port: entry.listen_port,
          ...IMPORT_DEFAULTS,
          device_group_in: importGroupId,
          target_addr: first.host,
          target_port: first.port,
          targets,
          // v1.0.6: attribute to the target user when an admin imports via the
          // user-management entry (/rules?owner_uid=X); else owner = caller.
          owner_uid: filterOwnerUid ?? undefined,
        });
        if (res.code === 0) results.push(`✅ ${entry.name}:${entry.listen_port}`);
        else results.push(`❌ ${entry.name}: ${res.message}`);
      } catch { results.push(`❌ ${entry.name}: network error`); }
    }
    if (results.length === 0) { message.error(t('importInvalidFormat')); return; }
    setImportResults(results);
    load(); // refresh rules in background
  };
  const handleUpdate = async (values: {
    name?: string; listen_port?: number; protocol?: string;
    device_group_in?: number;
    target_addr?: string; target_port?: number; targets?: RuleTargetInput[];
    load_balance_strategy?: string;
    upload_limit_mbps?: number;
    download_limit_mbps?: number;
    max_connections?: number;
    auto_restart_minutes?: number;
  }) => {
    if (!editing) return;
    const payload: Record<string, unknown> = {};
    if (values.name !== undefined && values.name !== editing.name) payload.name = values.name;
    if (values.listen_port !== undefined && values.listen_port !== editing.listen_port) payload.listen_port = values.listen_port;
    if (values.protocol !== undefined && values.protocol !== editing.protocol) payload.protocol = values.protocol;
    if (values.device_group_in !== undefined && values.device_group_in !== editing.device_group_in) payload.device_group_in = values.device_group_in;
    const newTargets = formTargets(values);
    const oldTargets = ruleTargets(editing);
    if (JSON.stringify(newTargets) !== JSON.stringify(oldTargets)) {
      if (newTargets.length < 1) {
        message.error(t('targetRequired'));
        return;
      }
      const first = newTargets[0];
      payload.target_addr = first.host;
      payload.target_port = first.port;
      payload.targets = newTargets;
    }
    if (values.load_balance_strategy !== undefined && values.load_balance_strategy !== (editing.load_balance_strategy ?? 'first')) {
      payload.load_balance_strategy = values.load_balance_strategy;
    }
    const newUp = values.upload_limit_mbps ?? 0;
    const newDown = values.download_limit_mbps ?? 0;
    if (newUp !== (editing.upload_limit_mbps ?? 0) || newDown !== (editing.download_limit_mbps ?? 0)) {
      payload.upload_limit_mbps = newUp;
      payload.download_limit_mbps = newDown;
    }
    // v1.2.0: send both together when either changed. The API defaults an
    // omitted one to the rule's current value, so sending a single field is
    // safe — but sending the pair keeps the request self-describing.
    const newMaxConn = values.max_connections ?? 0;
    const newAutoRestart = values.auto_restart_minutes ?? 0;
    if (newMaxConn !== (editing.max_connections ?? 0) || newAutoRestart !== (editing.auto_restart_minutes ?? 0)) {
      payload.max_connections = newMaxConn;
      payload.auto_restart_minutes = newAutoRestart;
    }
    if (Object.keys(payload).length === 0) { setEditOpen(false); return; }
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/rules/${editing.id}`, payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('ruleUpdated'));
      setEditOpen(false);
      load();
    } catch { message.error(t('failedUpdateRule')); }
  };

  const handleDelete = async (id: number) => {
    await api.delete(`/rules/${id}`);
    message.success(t('ruleDeleted'));
    load();
  };

  const handleBatchDelete = async () => {
    const ids = selectedRowKeys as number[];
    if (ids.length === 0) return;
    // v1.0.9: tally per-rule success/failure instead of assuming Promise.all
    // means everything worked — a delete can fail (e.g. 404/permission) and the
    // old code still reported the full count as deleted.
    const results = await Promise.all(ids.map(async id => {
      try {
        const res = await api.delete<unknown, ApiEnvelope<null>>(`/rules/${id}`);
        return res.code === 0;
      } catch { return false; }
    }));
    const ok = results.filter(Boolean).length;
    const fail = results.length - ok;
    if (fail === 0) {
      message.success(t('batchDeleteSuccess').replace('{count}', String(ok)));
    } else {
      message.warning(t('batchPartial').replace('{ok}', String(ok)).replace('{fail}', String(fail)));
    }
    setSelectedRowKeys([]);
    load();
  };

  /** v1.0.7: batch pause/resume. Each rule goes through PUT /rules/{id}
   *  {paused}. Resume can be rejected per-rule (403) when the rule points at a
   *  device group the user is no longer authorized for, so we tally ok/fail
   *  instead of assuming success. */
  const handleBatchSetPaused = async (paused: boolean) => {
    const ids = selectedRowKeys as number[];
    if (ids.length === 0) return;
    const results = await Promise.all(ids.map(async id => {
      try {
        const res = await api.put<unknown, ApiEnvelope<null>>(`/rules/${id}`, { paused });
        return res.code === 0;
      } catch { return false; }
    }));
    const ok = results.filter(Boolean).length;
    const fail = results.length - ok;
    if (fail === 0) {
      message.success((paused ? t('batchPauseSuccess') : t('batchResumeSuccess')).replace('{count}', String(ok)));
    } else {
      message.warning(t('batchPartial').replace('{ok}', String(ok)).replace('{fail}', String(fail)));
    }
    setSelectedRowKeys([]);
    load();
  };

  /** v1.2.0: restart one rule — drop its live connections and rebuild its
   *  listeners on every node of its inbound group. The rule's paused state is
   *  untouched; this is not a pause/resume round-trip.
   *
   *  `restarted` (nodes actually reached) drives the message rather than the
   *  HTTP code: the request can succeed while restarting nothing, e.g. every
   *  node is too old to understand the command. Reporting that as success would
   *  hide exactly the case the user needs to act on. */
  const handleRestart = async (r: ForwardRule) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<RestartResponse>>(`/rules/${r.id}/restart`, {});
      if (res.code !== 0) {
        message.error(res.message || t('restartFailed'));
        return;
      }
      const data = res.data;
      const outdated = (data?.nodes ?? []).filter(n => n.state === 'unsupported').length;
      const offline = (data?.nodes ?? []).filter(n => n.state === 'control_channel_offline').length;
      if ((data?.restarted ?? 0) > 0) {
        let msg = t('restartSuccess').replace('{count}', String(data?.restarted ?? 0));
        if (outdated > 0) msg += ` ${t('restartOutdatedSuffix').replace('{count}', String(outdated))}`;
        if (offline > 0) msg += ` ${t('restartOfflineSuffix').replace('{count}', String(offline))}`;
        if (outdated > 0 || offline > 0) message.warning(msg);
        else message.success(msg);
      } else if (outdated > 0) {
        message.warning(t('restartAllOutdated').replace('{count}', String(outdated)));
      } else if (offline > 0) {
        message.warning(t('restartAllOffline').replace('{count}', String(offline)));
      } else {
        message.warning(t('restartNoNodes'));
      }
    } catch {
      message.error(t('restartFailed'));
    }
  };

  /** v1.2.0: batch restart. Per-rule POST like batch pause/resume — there is no
   *  bulk endpoint. A rule can fail individually (paused → 400, or not owned →
   *  404), so tally ok/fail rather than assuming Promise.all means success. */
  const handleBatchRestart = async () => {
    const ids = selectedRowKeys as number[];
    if (ids.length === 0) return;
    const results = await Promise.all(ids.map(async id => {
      try {
        const res = await api.post<unknown, ApiEnvelope<RestartResponse>>(`/rules/${id}/restart`, {});
        // Reaching zero nodes is not a success worth reporting as one.
        return res.code === 0 && (res.data?.restarted ?? 0) > 0;
      } catch { return false; }
    }));
    const ok = results.filter(Boolean).length;
    const fail = results.length - ok;
    if (fail === 0) {
      message.success(t('batchRestartSuccess').replace('{count}', String(ok)));
    } else {
      message.warning(t('batchPartial').replace('{ok}', String(ok)).replace('{fail}', String(fail)));
    }
    setSelectedRowKeys([]);
  };

  /** v0.4.8: run a diagnosis for a rule. The panel fans the probe out to the
   *  rule's inbound-group nodes over WS and waits up to 8s for results. */
  const handleDiagnose = async (r: ForwardRule) => {
    setDiagnosing(r);
    setDiagnoseResult(null);
    setDiagnoseLoading(true);
    try {
      const res = await api.post<unknown, ApiEnvelope<DiagnoseResponse>>(`/rules/${r.id}/diagnose`);
      if (res.code === 0 && res.data) {
        setDiagnoseResult(res.data);
      } else {
        message.error(res.message || t('diagnoseFailed'));
      }
    } catch {
      message.error(t('diagnoseFailed'));
    } finally {
      setDiagnoseLoading(false);
    }
  };

  /** Toggle a rule's paused state via the dedicated paused field on the update
   *  API. Paused rules stay in the DB but the node stops forwarding (get_config
   *  filters WHERE paused = 0). This is the only way to pause a rule — before
   *  v0.3.0 the paused column existed but had no API to flip it. */
  const handleTogglePause = async (r: ForwardRule) => {
    const nextPaused = !r.paused;
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/rules/${r.id}`, { paused: nextPaused });
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(nextPaused ? t('rulePaused') : t('ruleResumed'));
      load();
    } catch { message.error(t('failedUpdateRule')); }
  };

  const protoTags = (p: string) => {
    if (p === 'tcp_udp') return <><Tag color="blue">TCP</Tag><Tag color="purple">UDP</Tag></>;
    if (p === 'udp') return <Tag color="purple">UDP</Tag>;
    return <Tag color="blue">TCP</Tag>;
  };

  const allColumns = [
    { title: t('id'), dataIndex: 'id', key: 'id', width: 60 },
    // v0.4.9: inbound group name. Hidden on small screens (responsive md+) so
    // the mobile view keeps the core columns. Lookup misses → "未知分组 (#ID)".
    {
      title: t('groupName'), key: 'group_name', width: 140, responsive: ['md' as const],
      render: (_: unknown, r: ForwardRule) => {
        const g = groupInfo.get(r.device_group_in);
        return g
          ? <Tag>{g.name}</Tag>
          : <Text type="secondary">{t('unknownGroup')} (#{r.device_group_in})</Text>;
      },
    },
    { title: t('name'), dataIndex: 'name', key: 'name' },
    {
      title: t('listenIp'), key: 'listen_ip', width: 160,
      render: (_: unknown, r: ForwardRule) => {
        const host = groupInfo.get(r.device_group_in)?.connect_host ?? '';
        return host
          ? <span className="rp-mono">{host}</span>
          : <Text type="secondary">{t('notConfigured')}</Text>;
      },
    },
    { title: t('listenPort'), dataIndex: 'listen_port', key: 'listen_port', render: (v: number) => <span className="rp-mono">{v}</span> },
    {
      title: t('protocol'), dataIndex: 'protocol', key: 'protocol',
      render: (p: string, r: ForwardRule) => (
        <Space size={4}>
          {protoTags(p)}
          {r.paused && <Tag color="red">{t('paused')}</Tag>}
          {!r.paused && ruleOverQuota(r) && (
            <Tooltip title={t('quotaExhaustedHint')}>
              <Tag color="orange">{t('quotaExhausted')}</Tag>
            </Tooltip>
          )}
        </Space>
      ),
    },
    {
      title: t('target'), key: 'target',
      render: (_: unknown, r: ForwardRule) => {
        // v1.0.9: a multi-target rule shows "first (+N)"; hovering lists every
        // enabled target IP so the admin can see the failover/round-robin pool.
        const all = ruleTargets(r).filter(t => t.enabled).map(t => `${t.host}:${t.port}`);
        const summary = <span className="rp-mono">{targetSummary(r)}</span>;
        return (
          <Space size={4} wrap>
            {all.length > 1 ? (
              <Tooltip title={<div>{all.map((s, i) => <div key={i} className="rp-mono">{s}</div>)}</div>}>
                {summary}
              </Tooltip>
            ) : summary}
            {r.load_balance_strategy && r.load_balance_strategy !== 'first' && (
              <Tag color="cyan">{r.load_balance_strategy === 'round_robin' ? t('lbRoundRobin') : t('lbFailover')}</Tag>
            )}
          </Space>
        );
      },
    },
    {
      // v0.4.14 PR3: owner is the rule's OWN uid — NOT the inbound group's uid.
      // An admin can create a rule on behalf of a user, and a regular user can
      // attach to an admin-owned shared group, so the rule owner and the group
      // owner often differ. Resolve the username from the rule's uid; fall back
      // to "#uid" when the user list isn't available.
      title: t('owner'), key: 'owner', width: 100,
      render: (_: unknown, r: ForwardRule) =>
        <Text>{userMap.get(r.uid) ?? `#${r.uid}`}</Text>,
    },
    { title: t('traffic'), dataIndex: 'traffic_used', key: 'traffic_used', render: (v: number) => formatBytes(v) },
    {
      title: t('action'), key: 'action', width: 260,
      render: (_: unknown, r: ForwardRule) => (
        <Space>
          <Button
            size="small" type="text"
            icon={r.paused ? <PlayCircleOutlined /> : <PauseCircleOutlined />}
            onClick={() => handleTogglePause(r)}
          >
            {r.paused ? t('resume') : t('pause')}
          </Button>
          <Button size="small" type="text" icon={<EditOutlined />} onClick={() => handleEdit(r)}>{t('edit')}</Button>
          <Button size="small" type="text" icon={<CopyOutlined />} onClick={() => handleCopy(r)}>{t('copy')}</Button>
          {/* v0.4.9: diagnosis is TCP-only — disable for pure-UDP rules. */}
          <Button size="small" type="text" icon={<MedicineBoxOutlined />} disabled={r.protocol === 'udp'} onClick={() => handleDiagnose(r)} title={r.protocol === 'udp' ? t('diagnoseUdpUnsupported') : t('diagnose')}>{t('diagnose')}</Button>
          {/* v1.2.0: restart drops every live connection on this rule, so it is
              behind a confirm (diagnose is read-only and isn't). Disabled while
              paused — there are no listeners to restart. */}
          <Popconfirm
            title={t('restartConfirmTitle')}
            description={t('restartConfirmDesc')}
            onConfirm={() => handleRestart(r)}
            okButtonProps={{ danger: true }}
            disabled={r.paused}
          >
            <Button
              size="small"
              type="text"
              icon={<ThunderboltOutlined />}
              disabled={r.paused}
              title={r.paused ? t('restartPausedHint') : t('restart')}
            >
              {t('restart')}
            </Button>
          </Popconfirm>
          <Popconfirm title={t('deleteRuleConfirm')} onConfirm={() => handleDelete(r.id)}>
            <Button danger size="small" type="text">{t('delete')}</Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];
  // v0.4.10: hide the owner column for regular users — they only ever own
  // their own rules, and /admin/users is never fetched for them (so userMap
  // is empty and the column would show "-" everywhere).
  const columns = isAdmin ? allColumns : allColumns.filter(c => c.key !== 'owner');

  const inGroups = groups.filter(g => g.group_type === 'in');
  // v0.4.12 PR1: inbound group selection. Admins pick from their OWN 'in'
  // groups. Regular users pick ONLY from admin-owned shared 'in' groups
  // (/groups/shared) — never their own historical groups, which the backend
  // also rejects. This keeps the UI and the API invariant in lock-step.
  const sharedInGroups = sharedGroups.filter(g => g.group_type === 'in');
  const allInGroups = isAdmin ? inGroups : sharedInGroups;
  const protocolOptions = [
    { value: 'tcp_udp', label: t('tcpUdp') },
    { value: 'tcp', label: 'TCP' },
    { value: 'udp', label: 'UDP' },
  ];
  const strategyOptions = [
    { value: 'first', label: t('lbFirst') },
    { value: 'round_robin', label: t('lbRoundRobin') },
    { value: 'failover', label: t('lbFailover') },
  ];
  const isUdp = (p?: string) => p === 'udp' || p === 'tcp_udp';

  const createGroupId = Form.useWatch('device_group_in', createForm);
  const editGroupId = Form.useWatch('device_group_in', editForm);
  const createProto = Form.useWatch('protocol', createForm);
  const editProto = Form.useWatch('protocol', editForm);

  const hostForForm = (gid?: number) => {
    if (!gid) return '';
    // v1.0.7: a regular user doesn't own the admin device groups, so `groups`
    // is empty for them — resolve the connect host from the merged groupInfo
    // (which also folds in their authorized shared groups) instead.
    return groupInfo.get(gid)?.connect_host ?? '';
  };
  const renderHostHint = (gid?: number) => {
    const host = hostForForm(gid);
    return (
      <Alert
        type="info" showIcon style={{ marginBottom: 12, padding: '4px 12px' }}
        title={t('currentInboundHost').replace('{host}', host || t('notConfigured'))}
      />
    );
  };

  /** v1.2.0: connection cap + scheduled restart. Shared by the create and edit
   *  forms so the two can't drift (the rate-limit block above predates this and
   *  is still duplicated).
   *
   *  Both fields are 0 = off. The cap's `extra` says the count is PER NODE and
   *  TCP-only, because neither is guessable: a rule on 3 nodes admits 3x the
   *  number typed here, and setting it on a UDP-only rule does nothing. */
  const renderConnectionControls = () => (
    <>
      <Form.Item
        name="max_connections"
        label={t('maxConnections')}
        extra={t('maxConnectionsHint')}
        initialValue={0}
      >
        <InputNumber min={0} style={{ width: '100%' }} placeholder="0" />
      </Form.Item>
      <Form.Item
        name="auto_restart_minutes"
        label={t('autoRestart')}
        extra={t('autoRestartHint').replace('{min}', String(MIN_AUTO_RESTART_MINUTES))}
        initialValue={0}
        rules={[{
          // Mirrors the API's floor. 0 = off is always allowed; anything between
          // 1 and the floor would drop connections faster than clients reconnect.
          validator: (_, value) => {
            const v = Number(value ?? 0);
            if (v === 0 || v >= MIN_AUTO_RESTART_MINUTES) return Promise.resolve();
            return Promise.reject(new Error(
              t('autoRestartTooSmall').replace('{min}', String(MIN_AUTO_RESTART_MINUTES))
            ));
          },
        }]}
      >
        <InputNumber min={0} style={{ width: '100%' }} addonAfter={t('minutes')} placeholder="0" />
      </Form.Item>
    </>
  );

  const renderTargetsEditor = () => (
    <Form.List name="targets" initialValue={[{ host: '', port: undefined as unknown as number, enabled: true }]}>
      {(fields, { add, remove, move }) => (
        <Space orientation="vertical" style={{ width: '100%' }}>
          <Text strong>{t('targets')}</Text>
          {fields.map((field, index) => (
            <Space key={field.key} align="baseline" wrap>
              <Form.Item
                {...field}
                name={[field.name, 'host']}
                label={t('address')}
                rules={[{ required: true }]}
                style={{ marginBottom: 8 }}
              >
                <Input placeholder={t('targetAddress')} style={{ width: 180 }} />
              </Form.Item>
              <Form.Item
                {...field}
                name={[field.name, 'port']}
                label={t('port')}
                rules={[
                  { required: true, message: t('targetPortInvalid') },
                  {
                    validator: (_, v) => {
                      if (v == null || v === '' || !Number.isFinite(Number(v)) || Number(v) < 1 || Number(v) > 65535) {
                        return Promise.reject(new Error(t('targetPortInvalid')));
                      }
                      return Promise.resolve();
                    },
                  },
                ]}
                style={{ marginBottom: 8 }}
              >
                <InputNumber min={1} max={65535} placeholder={t('targetPort')} style={{ width: 110 }} />
              </Form.Item>
              <Form.Item
                {...field}
                name={[field.name, 'enabled']}
                valuePropName="checked"
                initialValue={true}
                style={{ marginBottom: 8 }}
              >
                <Switch size="small" />
              </Form.Item>
              <Button size="small" icon={<ArrowUpOutlined />} aria-label={t('moveTargetUp')} disabled={index === 0} onClick={() => move(index, index - 1)} />
              <Button size="small" icon={<ArrowDownOutlined />} aria-label={t('moveTargetDown')} disabled={index === fields.length - 1} onClick={() => move(index, index + 1)} />
              <Button size="small" danger icon={<DeleteOutlined />} aria-label={t('deleteTarget')} disabled={fields.length <= 1} onClick={() => remove(field.name)} />
            </Space>
          ))}
          <Button size="small" icon={<PlusOutlined />} onClick={() => add({ host: '', port: undefined as unknown as number, enabled: true })}>{t('addTarget')}</Button>
        </Space>
      )}
    </Form.List>
  );

  const exportMenuItems: MenuProps['items'] = [
    { key: 'export-all', label: t('exportAll'), icon: <DownloadOutlined />, onClick: handleExportAll },
    { key: 'import', label: t('import'), icon: <UploadOutlined />, onClick: () => setImportOpen(true) },
  ];

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title"><ApiOutlined /> {t('forwardRules')}</h2>
        <Space>
          {/* v0.4.9: filter by inbound group. Only groups that actually have
              rules are offered, so the list stays short for large fleets. */}
          <Select
            style={{ minWidth: 180 }}
            allowClear
            placeholder={t('filterByGroup')}
            value={selectedGroup ?? undefined}
            onChange={(v: number | undefined) => { setSelectedGroup(v ?? null); setSelectedRowKeys([]); }}
            options={Array.from(new Set(rules.map(r => r.device_group_in)))
              .map(gid => {
                const g = groupMap.get(gid);
                return { value: gid, label: g ? g.name : `${t('unknownGroup')} (#${gid})` };
              })}
          />
          {selectedRowKeys.length > 0 && (
            <Button icon={<DownloadOutlined />} onClick={handleExportSelected}>
              {t('batchExport')} ({selectedRowKeys.length})
            </Button>
          )}
          {selectedRowKeys.length > 0 && (
            <Button icon={<PlayCircleOutlined />} onClick={() => handleBatchSetPaused(false)}>
              {t('batchResume')} ({selectedRowKeys.length})
            </Button>
          )}
          {selectedRowKeys.length > 0 && (
            <Button icon={<PauseCircleOutlined />} onClick={() => handleBatchSetPaused(true)}>
              {t('batchPause')} ({selectedRowKeys.length})
            </Button>
          )}
          {selectedRowKeys.length > 0 && (
            <Popconfirm
              title={t('batchRestartConfirm').replace('{count}', String(selectedRowKeys.length))}
              description={t('restartConfirmDesc')}
              onConfirm={handleBatchRestart}
              okButtonProps={{ danger: true }}
            >
              <Button icon={<ThunderboltOutlined />}>
                {t('batchRestart')} ({selectedRowKeys.length})
              </Button>
            </Popconfirm>
          )}
          {selectedRowKeys.length > 0 && (
            <Popconfirm
              title={t('batchDeleteConfirm').replace('{count}', String(selectedRowKeys.length))}
              onConfirm={handleBatchDelete}
              okButtonProps={{ danger: true }}
            >
              <Button danger icon={<DeleteOutlined />}>
                {t('batchDelete')} ({selectedRowKeys.length})
              </Button>
            </Popconfirm>
          )}
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Dropdown menu={{ items: exportMenuItems }}>
            <Button icon={<DownloadOutlined />}>{t('exportImport')}</Button>
          </Dropdown>
          <Button type="primary" icon={<PlusOutlined />} disabled={!isAdmin && sharedLoadFailed} onClick={() => { createForm.resetFields(); setCreateOpen(true); }}>{t('addRule')}</Button>
        </Space>
      </div>
      {/* v0.4.20: admin viewing another user's rules — show who. */}
      {filterOwnerUid && (
        <Alert type="info" showIcon style={{ marginBottom: 12 }}
          title={t('viewingUserRules').replace('{user}', userMap.get(filterOwnerUid) ?? `#${filterOwnerUid}`)}
        />
      )}
      {/* v0.4.12 PR1: a regular user whose shared-lines fetch failed sees a
          load-failure notice; rule creation is disabled above so they can't
          submit against an empty/unknown inbound list. */}
      {!isAdmin && sharedLoadFailed && (
        <Alert
          type="error"
          showIcon
          style={{ marginBottom: 12 }}
          title={t('loadFailed')}
          description={t('loadFailedRetry')}
        />
      )}
      <Table
        rowSelection={{ selectedRowKeys, onChange: (keys) => setSelectedRowKeys(keys as number[]) }}
        dataSource={visibleRules} columns={columns} rowKey="id" loading={loading}
        pagination={{ pageSize: 20 }} scroll={{ x: 1200 }}
      />

      <Modal title={t('addRule')} open={createOpen} onCancel={() => setCreateOpen(false)} onOk={() => createForm.submit()} okText={t('create')} cancelText={t('cancel')} width={620}>
        <Form form={createForm} onFinish={handleCreate} layout="vertical">
          <Tabs items={[
            {
              key: 'basic',
              label: t('tabBasic'),
              children: (<>
                <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input placeholder="my-rule" /></Form.Item>
                {/* v0.4.20: owner is determined by the entry point — admins use
                    /rules?owner_uid=X from the user management page; regular
                    users always own their own rules. */}
                {filterOwnerUid && (
                  <Alert type="info" showIcon style={{ marginBottom: 12 }}
                    title={t('creatingRuleFor').replace('{user}', userMap.get(filterOwnerUid) ?? `#${filterOwnerUid}`)}
                  />
                )}
                {renderHostHint(createGroupId)}
                <Form.Item name="listen_port" label={t('listenPort')} extra={t('listenPortHint')}><InputNumber min={1} max={65535} style={{ width: '100%' }} placeholder="auto" /></Form.Item>
                <Form.Item name="protocol" label={t('protocol')} rules={[{ required: true }]} initialValue="tcp_udp"
                  extra={isUdp(createProto) ? t('entryTransportUdpOnlyRaw') : undefined}>
                  <Select
                    options={protocolOptions}
                  />
                </Form.Item>
                {/* v0.4.20: WS/TLS tunnel hidden — public_transport always raw. */}
                <Form.Item name="public_transport" hidden initialValue="raw"><Input /></Form.Item>
                {/* v0.4.20: forward_mode always direct. */}
                <Form.Item name="forward_mode" hidden initialValue="direct"><Input /></Form.Item>
                <Form.Item name="device_group_in" label={t('inboundGroup')} rules={[{ required: true }]}>
                  <Select options={allInGroups.map(g => ({ value: g.id, label: g.name }))} placeholder={allInGroups.length ? t('select') : t('createGroupFirst')} />
                </Form.Item>
              </>),
            },
            {
              key: 'forward',
              label: t('tabForward'),
              children: (<>
                {renderTargetsEditor()}
                <Form.Item name="load_balance_strategy" label={t('loadBalanceStrategy')} initialValue="first">
                  <Select options={strategyOptions} />
                </Form.Item>
                <Alert
                  type="info"
                  showIcon
                  style={{ fontSize: 12, marginBottom: 16 }}
                  title={t('lbStrategyBlockTitle')}
                  description={
                    <div>
                      <div>• {t('lbFirstDesc')}</div>
                      <div>• {t('lbRoundRobinDesc')}</div>
                      <div>• {t('lbFailoverDesc')}</div>
                      <div style={{ marginTop: 8, color: '#888' }}>{t('lbStrategyBlockFooter')}</div>
                    </div>
                  }
                />
                <Form.Item
                  label={<span>{t('rateLimits')} <Tooltip title={<span style={{ whiteSpace: 'pre-line' }}>{t('rateLimitsTooltip')}</span>} overlayStyle={{ maxWidth: 340 }}><QuestionCircleOutlined style={{ color: '#999' }} /></Tooltip></span>}
                  extra={t('rateLimitsHint')}
                >
                  <Space orientation="vertical" style={{ width: '100%' }}>
                    <Form.Item name="upload_limit_mbps" noStyle initialValue={0}><InputNumber min={0} addonBefore={t('uploadLimit')} addonAfter="Mbps" style={{ width: '100%' }} placeholder="0" /></Form.Item>
                    <Form.Item name="download_limit_mbps" noStyle initialValue={0}><InputNumber min={0} addonBefore={t('downloadLimit')} addonAfter="Mbps" style={{ width: '100%' }} placeholder="0" /></Form.Item>
                  </Space>
                </Form.Item>
                {/* v1.2.0: the connection cap / auto-restart controls are
                    deliberately EDIT-ONLY. The atomic create transaction
                    (create_rule_with_guard) doesn't carry them, so offering them
                    here would silently discard whatever the user typed. They are
                    also settings you tune after watching a rule misbehave, not
                    ones you can guess up front. */}
              </>),
            },
          ]} />
        </Form>
      </Modal>

      <Modal title={t('editRule')} open={editOpen} onCancel={() => setEditOpen(false)} onOk={() => editForm.submit()} okText={t('save')} cancelText={t('cancel')} width={620}>
        <Form form={editForm} onFinish={handleUpdate} layout="vertical">
          <Tabs items={[
            {
              key: 'basic',
              label: t('tabBasic'),
              children: (<>
                <Form.Item name="name" label={t('name')}><Input /></Form.Item>
                {renderHostHint(editGroupId)}
                <Form.Item name="listen_port" label={t('listenPort')}><InputNumber min={1} max={65535} style={{ width: '100%' }} /></Form.Item>
                <Form.Item name="protocol" label={t('protocol')}
                  extra={isUdp(editProto) ? t('entryTransportUdpOnlyRaw') : undefined}>
                  <Select
                    options={protocolOptions}
                  />
                </Form.Item>
                {/* v0.4.20: WS/TLS tunnel hidden — public_transport always raw. */}
                <Form.Item name="public_transport" hidden initialValue="raw"><Input /></Form.Item>
                {/* v0.4.20: forward_mode always direct. */}
                <Form.Item name="forward_mode" hidden initialValue="direct"><Input /></Form.Item>
                <Form.Item name="device_group_in" label={t('inboundGroup')}><Select options={allInGroups.map(g => ({ value: g.id, label: g.name }))} /></Form.Item>
              </>),
            },
            {
              key: 'forward',
              // v1.0.9: force-render so the targets Form.List mounts even while
              // the Basic tab is active. Without this, editing only a Basic field
              // (e.g. listen_port) and submitting without opening this tab left
              // `values.targets` unregistered — handleUpdate then read it as
              // "targets cleared" and rejected with "add at least one target".
              forceRender: true,
              label: t('tabForward'),
              children: (<>
                {renderTargetsEditor()}
                <Form.Item name="load_balance_strategy" label={t('loadBalanceStrategy')} initialValue="first">
                  <Select options={strategyOptions} />
                </Form.Item>
                <Alert
                  type="info"
                  showIcon
                  style={{ fontSize: 12, marginBottom: 16 }}
                  title={t('lbStrategyBlockTitle')}
                  description={
                    <div>
                      <div>• {t('lbFirstDesc')}</div>
                      <div>• {t('lbRoundRobinDesc')}</div>
                      <div>• {t('lbFailoverDesc')}</div>
                      <div style={{ marginTop: 8, color: '#888' }}>{t('lbStrategyBlockFooter')}</div>
                    </div>
                  }
                />
                <Form.Item
                  label={<span>{t('rateLimits')} <Tooltip title={<span style={{ whiteSpace: 'pre-line' }}>{t('rateLimitsTooltip')}</span>} overlayStyle={{ maxWidth: 340 }}><QuestionCircleOutlined style={{ color: '#999' }} /></Tooltip></span>}
                  extra={t('rateLimitsHint')}
                >
                  <Space orientation="vertical" style={{ width: '100%' }}>
                    <Form.Item name="upload_limit_mbps" noStyle initialValue={0}><InputNumber min={0} addonBefore={t('uploadLimit')} addonAfter="Mbps" style={{ width: '100%' }} placeholder="0" /></Form.Item>
                    <Form.Item name="download_limit_mbps" noStyle initialValue={0}><InputNumber min={0} addonBefore={t('downloadLimit')} addonAfter="Mbps" style={{ width: '100%' }} placeholder="0" /></Form.Item>
                  </Space>
                </Form.Item>
                {renderConnectionControls()}
              </>),
            },
          ]} />
        </Form>
      </Modal>

      <Modal title={t('import')} open={importOpen} onCancel={() => { setImportOpen(false); setImportText(''); setImportResults([]); }}
        onOk={importResults.length > 0 ? undefined : handleImport}
        okText={importResults.length > 0 ? t('close') : t('import')}
        cancelText={t('cancel')} width={600}
        footer={importResults.length > 0 ? <Button onClick={() => { setImportOpen(false); setImportText(''); setImportResults([]); }}>{t('close')}</Button> : undefined}
      >
        {importResults.length === 0 ? (
          <>
            <Form.Item label={t('selectInboundGroup')}>
              <Select value={importGroupId} onChange={setImportGroupId}
                options={(isAdmin ? groups.filter(g => g.group_type === 'in') : sharedGroups)
                  .map(g => ({ value: g.id, label: `${g.name} (#${g.id})` }))}
                placeholder={t('selectDeviceGroups')} style={{ width: '100%' }} />
            </Form.Item>
            <Alert type="info" showIcon style={{ marginBottom: 12 }}
              title={t('importHint')} />
            <TextArea value={importText} onChange={e => setImportText(e.target.value)}
              rows={10} placeholder='[{"dest":["1.2.3.4:8080"],"listen_port":38446,"name":"SK5"}]' />
          </>
        ) : (
          <div style={{ maxHeight: 300, overflowY: 'auto' }} aria-live="polite" aria-label={t('import')}>
            {importResults.map((r, i) => <div key={i} style={{ fontFamily: 'var(--rp-font-mono)', fontSize: 13, lineHeight: 1.8 }}>{r}</div>)}
          </div>
        )}
      </Modal>

      {/* v0.4.8: rule diagnosis modal — three sections: ingress node, target
          egress, and panel dispatch summary. */}
      <Modal
        title={diagnosing ? `${t('diagnoseTitle')} · ${diagnosing.name} (#${diagnosing.id})` : t('diagnoseTitle')}
        open={!!diagnosing}
        onCancel={() => { setDiagnosing(null); setDiagnoseResult(null); }}
        footer={<Button onClick={() => { setDiagnosing(null); setDiagnoseResult(null); }}>{t('close')}</Button>}
        width={720}
      >
        {diagnoseLoading ? (
          <div style={{ textAlign: 'center', padding: 32 }} aria-live="polite" aria-busy="true"><Spin tip={t('diagnoseRunning')} /></div>
        ) : diagnoseResult ? (
          <>
            <Alert type="info" showIcon style={{ marginBottom: 16 }}
              title={t('diagnoseScopeHint')} />
            {/* v0.4.14: only the relay-node's OWN TCP diagnosis is shown — the
                node's listener status + its node→target TCP connectivity/latency.
                The latency is the node→target TCP handshake time, NOT a client
                end-to-end latency. */}
            <Typography.Title level={5}>{t('diagnoseIngress')}</Typography.Title>
            {diagnoseResult.nodes.length === 0 ? (
              <Text type="secondary">{t('diagnoseNoNodes')}</Text>
            ) : (
              <Space orientation="vertical" style={{ width: '100%' }}>
                {diagnoseResult.nodes.map((n, i) => (
                  <DiagnoseNodeRow key={i} node={n} t={t} isAdmin={isAdmin} />
                ))}
              </Space>
            )}
          </>
        ) : (
          <Text type="secondary">{t('diagnoseIdle')}</Text>
        )}
      </Modal>
    </>
  );
}

/** Render one node's diagnosis row. v0.4.15: the visible label is
 *  "分组名 · 公网IP" (or "分组名 · IP 未上报"), NEVER the raw node_id. node_id is
 *  admin-only (tooltip for troubleshooting); a regular user sees just the
 *  label. Same shape across all four statuses; the status tag + details differ. */
function DiagnoseNodeRow({ node, t, isAdmin }: { node: NodeDiagnoseStatus; t: (k: string) => string; isAdmin: boolean }) {
  const label = `${node.group_name || '-'} · ${node.public_ip || t('diagnoseIpMissing')}`;
  const labelText = <Text strong>{label}</Text>;
  // node_id is internal — only an admin gets the troubleshooting tooltip.
  const labelWithId = isAdmin
    ? <Tooltip title={t('diagnoseNodeIdLabel') + node.node_id}>{labelText}</Tooltip>
    : labelText;
  return (
    <div>
      <Space wrap align="center">
        {labelWithId}
        {node.status === 'result' && (
          <>
            <Tag color={node.listener_running ? 'green' : 'red'}>
              {node.listener_running ? t('diagnoseListenerRunning') : t('diagnoseListenerStopped')}
            </Tag>
            {node.listen_port ? <Text type="secondary">:{node.listen_port}</Text> : null}
            {node.protocol ? <Tag>{node.protocol}</Tag> : null}
            {node.transport ? <Tag>{node.transport}</Tag> : null}
          </>
        )}
        {node.status === 'unsupported' && (
          <Text type="warning">{t('diagnoseUnsupportedPrefix')}{node.node_version}{t('diagnoseUnsupportedSuffix')}</Text>
        )}
        {node.status === 'control_channel_offline' && (
          <Text type="secondary">{t('diagnoseOffline')}</Text>
        )}
        {node.status === 'timeout' && (
          <Tag color="orange">{t('diagnoseTimeout')}</Tag>
        )}
      </Space>
      {node.status === 'result' && node.results.length > 0 && (
        <Table<DiagnoseTargetResult> size="small" pagination={false} style={{ marginTop: 8 }}
          dataSource={node.results} rowKey="address"
          columns={[
            { title: t('diagnoseTarget'), dataIndex: 'address', key: 'address', render: (v: string) => <span className="rp-mono">{v}</span> },
            { title: t('diagnoseOutcome'), key: 'outcome', render: (_: unknown, r: DiagnoseTargetResult) => <ProbeOutcomeTag o={r.outcome} t={t} /> },
          ]}
        />
      )}
    </div>
  );
}

function ProbeOutcomeTag({ o, t }: { o: DiagnoseTargetResult['outcome']; t: (k: string) => string }) {
  // v0.4.9: 'route_only' variant removed — diagnosis is TCP-only.
  if (o === 'timeout') return <Tag color="orange">{t('diagnoseOutcomeTimeout')}</Tag>;
  if ('reachable' in o) return <Tag color="green">{t('diagnoseOutcomeReachable')} {o.reachable.elapsed_ms}ms</Tag>;
  if ('failed' in o) return <Tag color="red">{t('diagnoseOutcomeFailed')}: {o.failed.error}</Tag>;
  return <Tag>?</Tag>;
}
