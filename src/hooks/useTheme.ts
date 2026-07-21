import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/**
 * Applies the configured theme to the document root.
 * Listens for 'theme-changed' events to sync across all windows.
 */
export function useTheme() {
  useEffect(() => {
    const applyTheme = (theme: string) => {
      document.documentElement.setAttribute('data-theme', theme || 'system');
    };

    // Load initial theme from config
    invoke<{ theme?: string }>('get_config')
      .then((config) => applyTheme(config.theme || 'system'))
      .catch(() => applyTheme('system'));

    // Listen for theme changes from other windows
    const unlisten = listen<string>('theme-changed', (e) => {
      applyTheme(e.payload);
    });

    return () => { unlisten.then((f) => f()); };
  }, []);
}
