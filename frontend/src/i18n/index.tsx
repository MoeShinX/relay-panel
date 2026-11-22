import { useState, useCallback, useEffect, type ReactNode } from 'react';
import { zhCN, type Dict } from './zh-CN';
import { enUS } from './en-US';
import { I18nContext, type I18nContextValue, type Lang } from './context';

export type { Lang } from './context';

const STORAGE_KEY = 'relaypanel_lang';

const dictionaries: Record<Lang, Dict> = {
  'zh-CN': zhCN,
  'en-US': enUS,
};

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    return (stored === 'en-US' || stored === 'zh-CN') ? stored : 'zh-CN';
  });

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  const setLang = useCallback((next: Lang) => {
    localStorage.setItem(STORAGE_KEY, next);
    setLangState(next);
  }, []);

  const t = useCallback((key: keyof Dict) => {
    return dictionaries[lang][key] ?? String(key);
  }, [lang]);

  const value: I18nContextValue = { t, lang, setLang };

  return (
    <I18nContext.Provider value={value}>
      {children}
    </I18nContext.Provider>
  );
}
