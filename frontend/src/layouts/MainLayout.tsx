import { Layout, Menu, Button, Space, Typography, Segmented, Modal, Form, Input, message, Spin } from 'antd';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useState, Suspense } from 'react';
import {
  DashboardOutlined,
  ApiOutlined,
  CloudServerOutlined,
  UserOutlined,
  LogoutOutlined,
  LockOutlined,
  SettingOutlined,
  ShoppingOutlined,
} from '@ant-design/icons';
import { useI18n } from '../i18n/context';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';
import { useAuth } from '../auth/useAuth';
import { makePasswordValidator } from '../utils/password';

const { Sider, Content, Header } = Layout;
const { Text } = Typography;

export default function MainLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { t, lang, setLang } = useI18n();
  const { isAdmin, logout: authLogout } = useAuth();
  const [changePwOpen, setChangePwOpen] = useState(false);
  const [pwForm] = Form.useForm();
  const [pwSubmitting, setPwSubmitting] = useState(false);

  // v0.4.11 PR2: role-based navigation.
  // Admin: Dashboard → 个人中心, 转发规则, 设备分组, 节点状态, 隧道配置, 用户管理, 系统设置
  // Regular: 个人中心, 我的规则, 可用节点
  // v1.0.7: 仪表盘 (/) is admin-only — the regular-user dashboard was removed
  // (redirects to /account), so regular users no longer get this menu entry.
  const dashboardItem = { key: '/', icon: <DashboardOutlined />, label: t('dashboard') };
  const sharedItems = [
    { key: '/account', icon: <UserOutlined />, label: t('personalCenter') },
    { key: '/shop', icon: <ShoppingOutlined />, label: t('shop') },
    { key: '/rules', icon: <ApiOutlined />, label: t('myRules') },
    { key: '/nodes', icon: <CloudServerOutlined />, label: t('availableNodes') },
  ];
  const adminOnlyItems = [
    { key: '/groups', icon: <CloudServerOutlined />, label: t('deviceGroups') },
    { key: '/plans', icon: <ShoppingOutlined />, label: t('planManagement') },
    { key: '/users', icon: <UserOutlined />, label: t('users') },
    { key: '/settings', icon: <SettingOutlined />, label: t('systemSettings') },
  ];
  const menuItems = isAdmin
    ? [dashboardItem, ...sharedItems, ...adminOnlyItems]
    : sharedItems;

  const logout = () => {
    authLogout();
    navigate('/login');
  };

  const handleChangePassword = async (values: { current_password: string; new_password: string }) => {
    setPwSubmitting(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>('/user/password', values);
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      message.success(t('passwordChanged'));
      setChangePwOpen(false);
      pwForm.resetFields();
    } catch {
      message.error(t('passwordChangeFailed'));
    } finally {
      setPwSubmitting(false);
    }
  };

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Sider
        collapsible
        breakpoint="lg"
        width={220}
        style={{ background: 'var(--rp-sidebar-bg)' }}
      >
        <div style={{
          height: 'var(--rp-header-height)',
          display: 'flex', alignItems: 'center', justifyContent: 'center',
          color: '#fff', fontSize: 17, fontWeight: 600, letterSpacing: 0.5,
        }}>
          RelayPanel
        </div>
        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[location.pathname]}
          items={menuItems}
          onClick={({ key }) => navigate(key)}
          style={{ borderRight: 0 }}
        />
      </Sider>
      <Layout>
        <Header style={{
          background: '#fff', height: 'var(--rp-header-height)',
          padding: '0 24px', lineHeight: 'var(--rp-header-height)',
          display: 'flex', justifyContent: 'flex-end', alignItems: 'center',
          borderBottom: '1px solid var(--rp-border)',
        }}>
          <Space size="middle">
            <Segmented
              size="small"
              value={lang}
              onChange={(v) => setLang(v as 'zh-CN' | 'en-US')}
              options={[
                { value: 'zh-CN', label: t('langZhCN') },
                { value: 'en-US', label: t('langEnUS') },
              ]}
            />
            <Text type="secondary" style={{ fontSize: 13 }}>
              {isAdmin ? t('admin') : t('user')}
            </Text>
            <Button type="text" size="small" icon={<LockOutlined />} onClick={() => setChangePwOpen(true)}>
              {t('changePassword')}
            </Button>
            <Button type="text" size="small" icon={<LogoutOutlined />} onClick={logout}>
              {t('logout')}
            </Button>
          </Space>
        </Header>
        <Content style={{ margin: 'var(--rp-content-padding)', background: 'var(--rp-bg)' }}>
          {/* v1.2 (PR4): lazy-loaded pages (router.tsx) suspend here on first
              navigation to their chunk, showing a centered spinner instead of a
              blank pane. */}
          <Suspense fallback={<div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>}>
            <Outlet />
          </Suspense>
        </Content>
      </Layout>

      <Modal
        title={t('changePassword')}
        open={changePwOpen}
        onCancel={() => { setChangePwOpen(false); pwForm.resetFields(); }}
        onOk={() => pwForm.submit()}
        okText={t('save')}
        cancelText={t('cancel')}
        confirmLoading={pwSubmitting}
      >
        <Form form={pwForm} onFinish={handleChangePassword} layout="vertical">
          <Form.Item
            name="current_password"
            label={t('currentPassword')}
            rules={[{ required: true }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item
            name="new_password"
            label={t('newPassword')}
            rules={[
              { required: true, message: t('passwordRequired') },
              { validator: makePasswordValidator(t('newPasswordTooShort'), t('passwordTooLong')) },
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm_password"
            label={t('confirmPassword')}
            dependencies={['new_password']}
            rules={[
              { required: true },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error(t('passwordsDoNotMatch')));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        </Form>
      </Modal>
    </Layout>
  );
}
