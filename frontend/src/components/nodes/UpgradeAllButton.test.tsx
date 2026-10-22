import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Modal, message } from 'antd';

const { mockPost } = vi.hoisted(() => ({ mockPost: vi.fn() }));
vi.mock('../../api/client', () => ({ default: { post: mockPost } }));

import { NodeGroupSection } from './NodeGroupSection';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { zhCN } from '../../i18n/zh-CN';

// The real zh-CN strings: the counts in the labels and the toasts are what the
// admin reads, so these tests read them too.
const t = ((key: keyof typeof zhCN) => zhCN[key]) as unknown as Tfn;

const row = (over: Partial<NodeDisplayRow>): NodeDisplayRow => ({
  group_id: 1, group_name: 'g1', node_id: 'n1', online: true,
  node_version: '1.0.0', install_method: 'systemd', ws_connected: true,
  public_ipv4: '10.0.0.1',
  ...over,
});

// antd renders the confirm title twice (heading + accessible copy): look at the heading.
const expectTitle = (text: string) =>
  waitFor(() => expect(document.querySelector('.ant-modal-confirm-title')).toHaveTextContent(text));

const ok = () => Promise.resolve({ code: 0, message: 'ok', data: null });

const renderGroup = (rows: NodeDisplayRow[], onBatchUpgraded: (() => void) | null = vi.fn()) =>
  render(
    <NodeGroupSection
      rows={rows}
      panelProtocol={0}
      latestNodeVersion="1.1.0"
      nodeVersionCheckFailed={false}
      isMobile={false}
      t={t}
      openDetail={vi.fn()}
      onUpgrade={vi.fn()}
      onBatchUpgraded={onBatchUpgraded ?? undefined}
    />,
  );

const mixed = [
  row({ node_id: 'a1', public_ipv4: '10.0.0.1' }),
  row({ node_id: 'a2', public_ipv4: '10.0.0.2' }),
  row({ node_id: 'off', public_ipv4: '10.0.0.3', online: false }),
  row({ node_id: 'man', public_ipv4: '10.0.0.4', install_method: 'manual' }),
  row({ node_id: 'new', public_ipv4: '10.0.0.5', node_version: '1.1.0' }),
];

beforeEach(() => {
  mockPost.mockReset();
});
afterEach(() => {
  Modal.destroyAll();
  message.destroy();
});

describe('group "upgrade this group" button', () => {
  it('counts only the nodes the per-node button would accept', () => {
    renderGroup(mixed);
    expect(screen.getByTestId('upgrade-group-1')).toHaveTextContent('升级本组 (2)');
  });

  it('is absent when nothing in the group can be upgraded', () => {
    renderGroup([mixed[2], mixed[3], mixed[4]]);
    expect(screen.queryByTestId('upgrade-group-1')).not.toBeInTheDocument();
  });

  it('is absent without the admin callback', () => {
    renderGroup(mixed, null);
    expect(screen.queryByTestId('upgrade-group-1')).not.toBeInTheDocument();
  });

  it('is absent while the node-version check has failed', () => {
    render(
      <NodeGroupSection rows={mixed} panelProtocol={0} latestNodeVersion="" nodeVersionCheckFailed={true}
        isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={vi.fn()} onBatchUpgraded={vi.fn()} />,
    );
    expect(screen.queryByTestId('upgrade-group-1')).not.toBeInTheDocument();
  });

  it('does not fold the group when clicked', async () => {
    const user = userEvent.setup();
    const { container } = renderGroup(mixed);
    expect(container.querySelector('.ant-collapse-item-active')).not.toBeNull();
    await user.click(screen.getByTestId('upgrade-group-1'));
    await expectTitle('升级 2 台节点');
    expect(container.querySelector('.ant-collapse-item-active')).not.toBeNull();
  });

  it('asks first: lists the nodes, the target and the restart warning, and sends nothing', async () => {
    const user = userEvent.setup();
    renderGroup(mixed);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await expectTitle('升级 2 台节点');
    expect(screen.getByText(/同时升级到 v1\.1\.0/)).toBeInTheDocument();
    expect(screen.getByText(/所有节点会同时重启/)).toBeInTheDocument();
    // The modal's own list, not the table behind it.
    const items = document.querySelectorAll('.ant-modal-confirm li');
    expect(Array.from(items).map((li) => li.textContent)).toEqual([
      '10.0.0.1 · v1.0.0',
      '10.0.0.2 · v1.0.0',
    ]);
    expect(mockPost).not.toHaveBeenCalled();
  });

  it('sends nothing when the confirmation is cancelled', async () => {
    const user = userEvent.setup();
    renderGroup(mixed);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await user.click(await screen.findByRole('button', { name: /取\s*消/ }));
    expect(mockPost).not.toHaveBeenCalled();
  });

  it('lists only the first 10 nodes and says how many more', async () => {
    const user = userEvent.setup();
    const many = Array.from({ length: 12 }, (_, i) =>
      row({ node_id: `m${i}`, public_ipv4: `10.0.1.${i}` }));
    renderGroup(many);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await expectTitle('升级 12 台节点');
    const items = Array.from(document.querySelectorAll('.ant-modal-confirm li')).map((li) => li.textContent);
    expect(items).toHaveLength(11);
    expect(items[9]).toBe('10.0.1.9 · v1.0.0');
    expect(items[10]).toBe('……另外还有 2 台');
  });

  it('on confirm posts once per upgradable node, together, and nothing for the rest', async () => {
    const user = userEvent.setup();
    mockPost.mockImplementation(ok);
    const onDone = vi.fn();
    renderGroup(mixed, onDone);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await user.click(await screen.findByText('确认升级'));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(mockPost).toHaveBeenCalledTimes(2);
    expect(mockPost).toHaveBeenCalledWith('/nodes/1/upgrade/a1', {});
    expect(mockPost).toHaveBeenCalledWith('/nodes/1/upgrade/a2', {});
    expect(await screen.findByText(/已向 2 台节点下发升级/)).toBeInTheDocument();
  });

  it('fires every request before any of them has answered (not one by one)', async () => {
    const user = userEvent.setup();
    mockPost.mockImplementation(() => new Promise(() => {})); // never answers
    renderGroup(mixed);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await user.click(await screen.findByText('确认升级'));
    await waitFor(() => expect(mockPost).toHaveBeenCalledTimes(2));
    // Still in flight: the button shows loading and cannot be pressed again.
    expect(screen.getByTestId('upgrade-group-1')).toHaveClass('ant-btn-loading');
    await user.click(screen.getByTestId('upgrade-group-1'));
    expect(mockPost).toHaveBeenCalledTimes(2);
    expect(document.querySelectorAll('.ant-modal-confirm')).toHaveLength(1);
  });

  it('reports a partial failure with the count and the reason from the panel', async () => {
    const user = userEvent.setup();
    mockPost.mockImplementation((url: string) =>
      url.endsWith('/a2')
        ? Promise.resolve({ code: 409, message: '节点当前无 WS 控制通道', data: null })
        : ok());
    const onDone = vi.fn();
    renderGroup(mixed, onDone);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await user.click(await screen.findByText('确认升级'));
    expect(await screen.findByText(/成功 1 台，失败 1 台/)).toBeInTheDocument();
    expect(screen.getByText('10.0.0.2：节点当前无 WS 控制通道')).toBeInTheDocument();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('reports a request that was rejected, using the server message when there is one', async () => {
    const user = userEvent.setup();
    mockPost.mockImplementation((url: string) =>
      url.endsWith('/a1')
        ? Promise.reject({ response: { data: { message: '没有权限' } } })
        : Promise.reject(new Error('Network Error')));
    renderGroup(mixed);
    await user.click(screen.getByTestId('upgrade-group-1'));
    await user.click(await screen.findByText('确认升级'));
    expect(await screen.findByText(/成功 0 台，失败 2 台/)).toBeInTheDocument();
    expect(screen.getByText('10.0.0.1：没有权限')).toBeInTheDocument();
    expect(screen.getByText('10.0.0.2：下发失败')).toBeInTheDocument();
  });
});
