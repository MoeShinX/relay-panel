import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));

vi.mock('../api/client', () => ({
  default: { get: mockGet, put: vi.fn(), post: vi.fn() },
}));

import NotifySettings from './NotifySettings';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const config = {
  enabled: true,
  notify_offline: true,
  offline_alert_secs: 180,
  notify_recovery: true,
  notify_version_outdated: false,
  repeat_alert_minutes: 30,
  telegram_enabled: true,
  telegram_chat_id: '-1001234567890',
  telegram_bot_token_set: true,
  email_enabled: false,
  smtp_host: '',
  smtp_port: 465,
  smtp_username: '',
  smtp_password_set: false,
  smtp_from: '',
  smtp_to: '',
  smtp_tls: true,
};

describe('NotifySettings', () => {
  it('groups alert rules, channel status, and recent delivery history', async () => {
    mockGet.mockImplementation((url: string) => {
      if (url.endsWith('/history')) {
        return Promise.resolve(ok([{
          id: '1',
          created_at: '2026-08-07T10:00:00Z',
          event: 'offline',
          channel: 'telegram',
          status: 'failed',
          node_key: '1:node-a',
          detail: 'chat not found',
        }]));
      }
      return Promise.resolve(ok(config));
    });

    await act(async () => { render(<NotifySettings />); });

    await waitFor(() => expect(screen.getByText('notifyRules')).toBeInTheDocument());
    expect(screen.getByText('notificationChannels')).toBeInTheDocument();
    expect(screen.getByText('deliveryHistory')).toBeInTheDocument();
    expect(screen.getByText('channelConfiguredEnabled')).toBeInTheDocument();
    expect(screen.getByText('chat not found')).toBeInTheDocument();
  });
});
