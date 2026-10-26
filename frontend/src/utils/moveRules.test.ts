import { describe, expect, it, vi } from 'vitest';
import type { ForwardRule } from '../api/types';
import { moveRules } from './moveRules';

const rule = (id: number, group = 1, port = 10000 + id) =>
  ({ id, name: `r${id}`, device_group_in: group, listen_port: port }) as unknown as ForwardRule;
const reply = (code: number, message = '') => ({ code, message, data: null });

describe('moveRules', () => {
  it('moves each rule, sending only the new group', async () => {
    const put = vi.fn().mockResolvedValue(reply(0));
    const r = await moveRules([rule(1), rule(2)], 9, put);
    expect(r.moved.map((x) => x.id)).toEqual([1, 2]);
    expect(put).toHaveBeenCalledWith('/rules/1', { device_group_in: 9 });
    expect(put).toHaveBeenCalledWith('/rules/2', { device_group_in: 9 });
  });

  it('skips a rule whose port is taken in the target group and carries on', async () => {
    const put = vi.fn((url: string) =>
      Promise.resolve(url === '/rules/2' ? reply(409, 'port in use') : reply(0)),
    );
    const r = await moveRules([rule(1), rule(2), rule(3)], 9, put);
    expect(r.moved.map((x) => x.id)).toEqual([1, 3]);
    expect(r.occupied.map((x) => x.id)).toEqual([2]);
    expect(r.failed).toEqual([]);
  });

  it('does not call the server for rules already in the target group', async () => {
    const put = vi.fn().mockResolvedValue(reply(0));
    const r = await moveRules([rule(1, 9), rule(2, 1)], 9, put);
    expect(r.alreadyThere.map((x) => x.id)).toEqual([1]);
    expect(put).toHaveBeenCalledTimes(1);
  });

  it('reports other refusals and thrown errors as failures, not as occupied', async () => {
    const put = vi.fn((url: string) =>
      url === '/rules/1'
        ? Promise.resolve(reply(403, 'not allowed'))
        : Promise.reject(new Error('network down')),
    );
    const r = await moveRules([rule(1), rule(2)], 9, put);
    expect(r.occupied).toEqual([]);
    expect(r.failed.map((f) => [f.rule.id, f.message])).toEqual([
      [1, 'not allowed'],
      [2, 'network down'],
    ]);
  });

  it('moves in list order, one at a time', async () => {
    const order: string[] = [];
    const put = vi.fn(async (url: string) => {
      order.push(`start ${url}`);
      await Promise.resolve();
      order.push(`end ${url}`);
      return reply(0);
    });
    await moveRules([rule(1), rule(2)], 9, put);
    expect(order).toEqual(['start /rules/1', 'end /rules/1', 'start /rules/2', 'end /rules/2']);
  });
});
