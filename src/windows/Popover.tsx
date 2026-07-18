import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './Popover.css';

type AppState = 'idle' | 'capturing' | 'generating' | 'ready' | 'error';

const STATE: Record<AppState, { label: string; tone: string }> = {
  idle: { label: '就绪', tone: 'var(--verdigris)' },
  capturing: { label: '读取对话', tone: 'var(--azure)' },
  generating: { label: '补全中', tone: 'var(--caret)' },
  ready: { label: '已复制', tone: 'var(--verdigris)' },
  error: { label: '出错了', tone: 'var(--rust)' },
};

export default function Popover() {
  const [state, setState] = useState<AppState>('idle');
  const [lastReply, setLastReply] = useState('');
  const [model, setModel] = useState('');

  useEffect(() => {
    invoke<{ model: string }>('get_config').then((c) => setModel(c.model)).catch(() => {});
    const u1 = listen<string>('app-state', (e) => setState(e.payload as AppState));
    const u2 = listen<string>('last-reply', (e) => setLastReply(e.payload));
    return () => { u1.then((f) => f()); u2.then((f) => f()); };
  }, []);

  const { label, tone } = STATE[state];
  const working = state === 'capturing' || state === 'generating';

  return (
    <div className="pop">
      <header className="pop__top">
        <span className="pop__mark">
          Compleo<span className={`caret ${working ? '' : 'caret--steady'}`} aria-hidden="true" />
        </span>
        {model && <span className="pop__model">{model}</span>}
      </header>

      <div className="pop__state">
        <span className="pop__dot" style={{ background: tone }} />
        <span className="pop__label">{label}</span>
      </div>

      <div className="pop__reply">
        {lastReply
          ? <p className="pop__text">{lastReply}</p>
          : <p className="pop__empty">按 <kbd>⌘</kbd><kbd>.</kbd> 生成第一条回复</p>}
      </div>

      <div className="pop__actions">
        <button className="pact pact--main" onClick={() => invoke('open_main_window_cmd')}>
          设置
        </button>
        <button className="pact" onClick={() => invoke('quit_app')}>
          退出
        </button>
      </div>
    </div>
  );
}
