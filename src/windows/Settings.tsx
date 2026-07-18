import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './Settings.css';

interface Config {
  api_key: string;
  base_url: string;
  model: string;
  hotkey: string;
}

const EMPTY: Config = { api_key: '', base_url: '', model: '', hotkey: 'Cmd+.' };

export default function Settings() {
  const [config, setConfig] = useState<Config>(EMPTY);
  const [showKey, setShowKey] = useState(false);
  const [note, setNote] = useState<{ kind: 'ok' | 'bad'; text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<Config>('get_config')
      .then(setConfig)
      .catch((e) => flash('bad', `读取配置失败：${e}`));
  }, []);

  const flash = (kind: 'ok' | 'bad', text: string) => {
    setNote({ kind, text });
    setTimeout(() => setNote(null), 2500);
  };

  const set = (k: keyof Config) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setConfig((c) => ({ ...c, [k]: e.target.value }));

  const save = async () => {
    setSaving(true);
    try {
      await invoke('save_config', { config });
      flash('ok', '已保存');
    } catch (e) {
      flash('bad', `保存失败：${e}`);
    }
    setSaving(false);
  };

  return (
    <div className="cfg">
      <section className="group">
        <div className="group__head">
          <h2 className="group__title">连接</h2>
          <span className="group__rule" />
        </div>

        <label className="row">
          <span className="row__key">API 密钥</span>
          <span className="row__field">
            <input
              className="in in--mono"
              type={showKey ? 'text' : 'password'}
              value={config.api_key}
              onChange={set('api_key')}
              placeholder="sk-…"
              spellCheck={false}
            />
            <button className="reveal" onClick={() => setShowKey((v) => !v)}>
              {showKey ? '隐藏' : '显示'}
            </button>
          </span>
        </label>

        <label className="row">
          <span className="row__key">接口地址</span>
          <input
            className="in in--mono"
            type="text"
            value={config.base_url}
            onChange={set('base_url')}
            placeholder="https://api.openai.com/v1"
            spellCheck={false}
          />
        </label>

        <label className="row">
          <span className="row__key">模型</span>
          <input
            className="in in--mono"
            type="text"
            value={config.model}
            onChange={set('model')}
            placeholder="gpt-4o-mini"
            spellCheck={false}
          />
        </label>
      </section>

      <section className="group">
        <div className="group__head">
          <h2 className="group__title">触发</h2>
          <span className="group__rule" />
        </div>
        <div className="trigger">
          <span className="trigger__keys">
            <kbd>⌘</kbd><kbd>.</kbd>
          </span>
          <span className="trigger__note">在任意聊天窗口唤起补全</span>
        </div>
      </section>

      <div className="commit">
        <button className="save" onClick={save} disabled={saving}>
          {saving ? '保存中…' : '保存'}
        </button>
        {note && <span className={`note note--${note.kind}`}>{note.text}</span>}
      </div>
    </div>
  );
}
