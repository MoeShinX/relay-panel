import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { message } from 'antd';
import type { DndContextProps } from '@dnd-kit/core';

const { mockGet, mockPut } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPut: vi.fn() }));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
const { mockIsMobile } = vi.hoisted(() => ({ mockIsMobile: vi.fn() }));
// The latest props the page gave DndContext, so a test can deliver a drop to
// onDragEnd directly (jsdom has no layout, so a real drag cannot be simulated).
const { dnd } = vi.hoisted(() => ({ dnd: { props: null as null | DndContextProps } }));

vi.mock('../api/client', () => ({
  default: { get: mockGet, put: mockPut, delete: vi.fn() },
}));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));
vi.mock('../hooks/useIsMobile', () => ({ useIsMobile: mockIsMobile }));
vi.mock('@dnd-kit/core', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@dnd-kit/core')>();
  return {
    ...actual,
    DndContext: (props: DndContextProps) => {
      dnd.props = props;
      return <actual.DndContext {...props} />;
    },
  };
});

import NodeStatus from './NodeStatus';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });
const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

/** One online node of group `gid`; `order` is the group's display order. */
const node = (gid: number, order?: number) => ({
  group_id: gid,
  group_name: `grp-${gid}`,
  group_sort_order: order,
  node_id: `n${gid}`,
  online: true,
  connections: 0,
});

/** Group ids in the order their headers appear on the page. */
const shownOrder = () =>
  Array.from(document.querySelectorAll('.ant-collapse'))
    .map((el) => /grp-(\d+)/.exec(el.textContent ?? '')?.[1])
    .map(Number);

const handles = () => screen.queryAllByLabelText('dragToReorderGroup');

/** Serve /nodes (admin) or /nodes/shared (user) plus the calls the page makes. */
function serve(nodes: unknown[], opts: { shared?: boolean; allGroups?: number[] } = {}) {
  mockGet.mockImplementation((url: string) => {
    if (url === (opts.shared ? '/nodes/shared' : '/nodes')) return Promise.resolve(ok(nodes));
    if (url === '/groups') return Promise.resolve(ok((opts.allGroups ?? []).map((id) => ({ id }))));
    if (url === '/system/version') return Promise.resolve({ current_version: '1.2.0' });
    if (url === '/nodes/live-rates') return Promise.resolve(ok([]));
    return Promise.reject(new Error(`unexpected ${url}`));
  });
}

const drop = (active: number, over: number) =>
  act(async () => {
    await dnd.props!.onDragEnd!({ active: { id: active }, over: { id: over } } as never);
  });

beforeEach(() => {
  mockGet.mockReset();
  mockPut.mockReset();
  mockUseAuth.mockReset();
  mockIsMobile.mockReset();
  mockIsMobile.mockReturnValue(false);
  dnd.props = null;
  vi.useFakeTimers();
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('group order on the node-status page', () => {
  it('lists the groups in the server order, not by id', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 3), node(2, 1), node(3, 2)]);
    render(<NodeStatus />);
    await flush();
    expect(shownOrder()).toEqual([2, 3, 1]);
  });

  it('a regular user sees the same order, without handles', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    serve([node(1, 3), node(2, 1), node(3, 2)], { shared: true });
    render(<NodeStatus />);
    await flush();
    expect(shownOrder()).toEqual([2, 3, 1]);
    expect(handles()).toHaveLength(0);
  });

  it('on a phone an admin sees the order but no handles', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockIsMobile.mockReturnValue(true);
    serve([node(1, 2), node(2, 1)]);
    render(<NodeStatus />);
    await flush();
    expect(shownOrder()).toEqual([2, 1]);
    expect(handles()).toHaveLength(0);
  });

  it('only an admin on a wide screen gets a handle, one per group', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1), node(2, 2)]);
    render(<NodeStatus />);
    await flush();
    expect(handles()).toHaveLength(2);
  });

  it('a leftover record of a deleted group goes last and cannot be dragged', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(5, undefined), node(1, 2), node(2, 1)]);
    render(<NodeStatus />);
    await flush();
    expect(shownOrder()).toEqual([2, 1, 5]);
    expect(handles()).toHaveLength(2);
  });

  it('a click on the handle does not fold the group; a click on the title does', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1)]);
    const { container } = render(<NodeStatus />);
    await flush();
    const item = () => container.querySelector('.ant-collapse-item')!;
    expect(item().className).toContain('ant-collapse-item-active');

    fireEvent.click(handles()[0]);
    expect(item().className).toContain('ant-collapse-item-active');

    fireEvent.click(screen.getByText('grp-1'));
    expect(item().className).not.toContain('ant-collapse-item-active');
  });

  it('the handle takes no room until the keyboard reaches it', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1)]);
    render(<NodeStatus />);
    await flush();
    const handle = handles()[0];
    expect(handle.style.width).toBe('0px');
    fireEvent.focus(handle);
    expect(handle.style.width).toBe('');
    fireEvent.blur(handle);
    expect(handle.style.width).toBe('0px');
  });

  it('Enter on the handle does not fold the group', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1)]);
    const { container } = render(<NodeStatus />);
    await flush();
    fireEvent.keyDown(handles()[0], { key: 'Enter', code: 'Enter' });
    expect(container.querySelector('.ant-collapse-item')!.className).toContain(
      'ant-collapse-item-active',
    );
  });
});

describe('dropping a group', () => {
  it('moves it at once and saves the whole order (insert, not swap)', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    // Group 9 has no node record, so the page does not show it. On the server it
    // is third; it must still be third after the shown groups are rearranged.
    serve([node(1, 1), node(2, 2), node(3, 4), node(4, 5)], { allGroups: [1, 2, 9, 3, 4] });
    mockPut.mockResolvedValue(ok(null));
    render(<NodeStatus />);
    await flush();
    expect(shownOrder()).toEqual([1, 2, 3, 4]);

    await drop(4, 2); // 4 dropped on 2 -> [1, 4, 2, 3]
    expect(shownOrder()).toEqual([1, 4, 2, 3]);
    expect(mockPut).toHaveBeenCalledTimes(1);
    expect(mockPut).toHaveBeenCalledWith('/groups/order', { ids: [1, 4, 9, 2, 3] });
  });

  it('keeps the new order through the refresh that follows', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1), node(2, 2)], { allGroups: [1, 2] });
    mockPut.mockResolvedValue(ok(null));
    render(<NodeStatus />);
    await flush();
    await drop(2, 1);
    expect(shownOrder()).toEqual([2, 1]);

    // The server now answers with the order that was saved.
    serve([node(1, 2), node(2, 1)], { allGroups: [1, 2] });
    await flush(5000);
    expect(shownOrder()).toEqual([2, 1]);
  });

  it('puts the old order back and says so when saving fails', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1), node(2, 2)], { allGroups: [1, 2] });
    mockPut.mockRejectedValue(new Error('network'));
    const errorSpy = vi.spyOn(message, 'error').mockImplementation(() => ({}) as never);
    render(<NodeStatus />);
    await flush();

    await drop(2, 1);
    expect(shownOrder()).toEqual([1, 2]);
    expect(errorSpy).toHaveBeenCalledWith('reorderGroupsFailed');
  });

  it('treats an error answer from the server as a failure too', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1), node(2, 2)], { allGroups: [1, 2] });
    mockPut.mockResolvedValue({ code: 400, message: 'bad', data: null });
    const errorSpy = vi.spyOn(message, 'error').mockImplementation(() => ({}) as never);
    render(<NodeStatus />);
    await flush();

    await drop(2, 1);
    expect(shownOrder()).toEqual([1, 2]);
    expect(errorSpy).toHaveBeenCalledTimes(1);
  });

  it('does nothing when a group is dropped on itself or outside the list', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    serve([node(1, 1), node(2, 2)], { allGroups: [1, 2] });
    render(<NodeStatus />);
    await flush();

    await drop(1, 1);
    await act(async () => {
      await dnd.props!.onDragEnd!({ active: { id: 1 }, over: null } as never);
    });
    expect(mockPut).not.toHaveBeenCalled();
    expect(shownOrder()).toEqual([1, 2]);
  });
});
