import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { mockGet, mockPut } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPut: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet, put: mockPut } }));

import SiteSettings from './SiteSettings';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });
const site = (over: Record<string, unknown> = {}) => ({
  site_name: 'RelayPanel', subtitle: '', contact: '', announcement: '', announcement_type: 'info',
  show_node_labels: false, ...over,
});

beforeEach(() => {
  mockGet.mockReset();
  mockPut.mockReset();
});

describe('the "show node labels to users" switch', () => {
  it('is off by default and is saved with the rest of the site settings', async () => {
    mockGet.mockResolvedValue(ok(site()));
    mockPut.mockResolvedValue(ok(site({ show_node_labels: true })));
    const user = userEvent.setup();
    render(<SiteSettings />);

    const toggle = await screen.findByRole('switch');
    expect(toggle).not.toBeChecked();
    await user.click(toggle);
    await user.click(screen.getByRole('button', { name: /save|保存/ }));

    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    expect(mockPut.mock.calls[0][0]).toBe('/admin/settings/site');
    expect(mockPut.mock.calls[0][1]).toMatchObject({ show_node_labels: true });
  });

  it('reflects what is stored', async () => {
    mockGet.mockResolvedValue(ok(site({ show_node_labels: true })));
    render(<SiteSettings />);
    expect(await screen.findByRole('switch')).toBeChecked();
  });
});
