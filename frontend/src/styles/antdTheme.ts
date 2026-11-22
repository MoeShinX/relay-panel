import type { ThemeConfig } from 'antd';

// RelayPanel — "石墨靛蓝 / Modern Minimal Light (Linear · Stripe direction)".
// Cool architectural slate canvas (#f4f5f8), crisp pure-white elevated cards,
// obsidian-graphite sidebar (#111318), refined indigo accent (#4f46e5), and
// desaturated semantic tokens.
export const rpTheme: ThemeConfig = {
  token: {
    colorPrimary: '#4f46e5',
    colorInfo: '#4f46e5',
    colorSuccess: '#059669',
    colorWarning: '#d97706',
    colorError: '#e11d48',
    colorLink: '#4f46e5',
    colorLinkHover: '#4338ca',
    borderRadius: 8,
    borderRadiusSM: 6,
    borderRadiusLG: 12,
    colorBgLayout: '#f4f5f8',
    colorBgContainer: '#ffffff',
    colorText: '#0f172a',
    colorTextSecondary: '#475569',
    colorTextTertiary: '#64748b',
    colorTextDescription: '#475569',
    colorBorder: '#e2e8f0',
    colorBorderSecondary: '#eaecf0',
    fontWeightStrong: 600,
    fontFamily:
      '"Noto Sans SC", system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", ' +
      '"PingFang SC", "Microsoft YaHei", "Helvetica Neue", Arial, sans-serif',
  },
  components: {
    Menu: {
      darkItemBg: 'transparent',
      darkSubMenuItemBg: 'transparent',
      darkItemColor: '#94a3b8',
      darkItemHoverColor: '#f8fafc',
      darkItemHoverBg: 'rgba(255, 255, 255, 0.06)',
      darkItemSelectedBg: 'rgba(99, 102, 241, 0.18)',
      darkItemSelectedColor: '#ffffff',
      itemBorderRadius: 8,
      itemMarginInline: 10,
      itemMarginBlock: 3,
      itemHeight: 38,
    },
    Button: {
      primaryShadow: 'none',
      defaultShadow: 'none',
      dangerShadow: 'none',
      fontWeight: 500,
      controlHeight: 34,
      controlHeightLG: 42,
      controlHeightSM: 28,
    },
    Input: {
      controlHeight: 34,
      controlHeightLG: 42,
      activeShadow: '0 0 0 3px rgba(79, 70, 229, 0.12)',
    },
    InputNumber: {
      controlHeight: 34,
      activeShadow: '0 0 0 3px rgba(79, 70, 229, 0.12)',
    },
    Select: {
      controlHeight: 34,
      controlHeightLG: 42,
    },
    Card: {
      borderRadiusLG: 12,
      headerFontSize: 15,
    },
    Table: {
      headerBg: '#f8fafc',
      headerColor: '#475569',
      rowHoverBg: '#f5f7ff',
      borderColor: '#eef0f4',
    },
    Modal: {
      borderRadiusLG: 14,
    },
    Layout: {
      bodyBg: '#f4f5f8',
      headerBg: 'rgba(255, 255, 255, 0.86)',
      siderBg: '#111318',
      triggerBg: '#111318',
      triggerColor: '#94a3b8',
    },
  },
};
