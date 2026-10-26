import { Button, Form, Input, Space, Typography } from 'antd';
import { DeleteOutlined, PlusOutlined } from '@ant-design/icons';
import { isConnectHost } from '../utils/connectHosts';

/**
 * The connect addresses of a device group, one input per address, the way a
 * rule's targets are entered: a first row, "add connection" for more.
 *
 * Each address is an IPv4/IPv6 literal or a host name. `legacy` lists the values
 * the group already had; those stay acceptable while unchanged, so a group
 * holding something older and looser can still be edited and saved.
 *
 * Lives in the form under `connect_hosts` (a string array); the page joins it
 * back into the single `connect_host` column.
 */
export default function ConnectHostsEditor({
  t, legacy = [], canAdd = true, required = true,
}: {
  t: (key: string) => string;
  legacy?: string[];
  canAdd?: boolean;
  required?: boolean;
}) {
  return (
    <Form.List name="connect_hosts" initialValue={['']}>
      {(fields, { add, remove }) => (
        <Space orientation="vertical" style={{ width: '100%', marginBottom: 24 }}>
          <Typography.Text strong>
            {required && <span style={{ color: 'var(--ant-color-error, #ff4d4f)' }}>* </span>}
            {t('connectHost')}
          </Typography.Text>
          {fields.map(({ key, ...field }, index) => (
            <Space key={key} align="baseline" style={{ display: 'flex' }}>
              <Form.Item
                {...field}
                rules={[
                  { required: required && index === 0, message: t('connectHostInvalid') },
                  {
                    validator: (_, v: string | undefined) => {
                      const s = (v ?? '').trim();
                      if (!s || isConnectHost(s) || legacy.includes(s)) return Promise.resolve();
                      return Promise.reject(new Error(t('connectHostInvalid')));
                    },
                  },
                ]}
                style={{ marginBottom: 0, minWidth: 280 }}
              >
                <Input aria-label={t('connectHost')} placeholder={t('connectHostPlaceholder')} />
              </Form.Item>
              <Button
                icon={<DeleteOutlined />} danger aria-label={t('deleteConnection')}
                disabled={fields.length <= 1} onClick={() => remove(field.name)}
              />
            </Space>
          ))}
          {canAdd && (
            <Button size="small" icon={<PlusOutlined />} onClick={() => add('')}>{t('addConnection')}</Button>
          )}
          <Typography.Text type="secondary">{t('connectHostHint')}</Typography.Text>
        </Space>
      )}
    </Form.List>
  );
}
