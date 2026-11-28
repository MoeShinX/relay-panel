import type { ThemeConfig } from 'antd';

// RelayPanel — "雾感丁香紫 / Soft Muted Lavender + Light Zinc".
// Integrated light zinc sidebar (#f3f3f5), crisp white elevated surfaces,
// soft lavender primary accent (#6c5dd8; white text on it is about 5:1).
export const rpTheme: ThemeConfig = {
  token: {
    colorPrimary: '#6c5dd8',
    colorInfo: '#6c5dd8',
    colorInfoBg: '#f6f5fd',
    colorInfoBorder: '#e2defb',
    colorSuccess: '#059669',
    colorWarning: '#d97706',
    colorError: '#e11d48',
    colorLink: '#5b4cc4',
    colorLinkHover: '#4c3db0',
    borderRadius: 8,
    borderRadiusSM: 6,
    borderRadiusLG: 12,
    colorBgLayout: '#f6f6f7',
    colorBgContainer: '#ffffff',
    colorText: '#09090b',
    colorTextSecondary: '#52525b',
    colorTextTertiary: '#71717a',
    colorTextDescription: '#52525b',
    colorBorder: '#e4e4e7',
    colorBorderSecondary: '#ececee',
    fontWeightStrong: 500,
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", ' +
      '"Helvetica Neue", Arial, "PingFang SC", "HarmonyOS Sans SC", ' +
      '"Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans SC", sans-serif',
    fontFamilyCode:
      '"JetBrains Mono", "Cascadia Mono", "Cascadia Code", "SF Mono", ui-monospace, ' +
      'Menlo, Consolas, monospace',
  },
  components: {
    Menu: {
      itemBg: 'transparent',
      subMenuItemBg: 'transparent',
      itemColor: '#27272a',
      itemHoverColor: '#09090b',
      itemHoverBg: 'rgba(24, 24, 27, 0.06)',
      itemSelectedBg: '#ffffff',
      itemSelectedColor: '#09090b',
      activeBarBorderWidth: 0,
      itemBorderRadius: 8,
      itemMarginInline: 10,
      itemMarginBlock: 3,
      itemHeight: 37,
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
      activeShadow: '0 0 0 3px rgba(108, 93, 216, 0.14)',
    },
    InputNumber: {
      controlHeight: 34,
      activeShadow: '0 0 0 3px rgba(108, 93, 216, 0.14)',
    },
    Select: {
      controlHeight: 34,
      controlHeightLG: 42,
    },
    Card: {
      borderRadiusLG: 12,
      headerFontSize: 15,
    },
    Tooltip: {
      colorBgSpotlight: '#18181b',
      borderRadius: 8,
    },
    Table: {
      headerBg: '#fafafa',
      headerColor: '#71717a',
      rowHoverBg: '#fafafa',
      borderColor: '#f0f0f2',
    },
    Modal: {
      borderRadiusLG: 14,
    },
    Segmented: {
      trackBg: '#f1f2f5',
      itemColor: '#52525b',
      itemHoverColor: '#09090b',
      itemSelectedBg: '#ffffff',
      itemSelectedColor: '#09090b',
    },
    Layout: {
      bodyBg: '#f6f6f7',
      headerBg: 'rgba(255, 255, 255, 0.90)',
      siderBg: '#ecedf0',
      triggerBg: '#ecedf0',
      triggerColor: '#3f3f46',
    },
  },
};
