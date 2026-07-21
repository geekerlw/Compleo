import { useEffect, useState, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useTheme } from './hooks/useTheme';
import './windows/Overlay.css';

type Status = 'idle' | 'streaming' | 'done' | 'error';

function App() {
  useTheme();
  const [thinking, setThinking] = useState('');
  const [reply, setReply] = useState('');
  const [status, setStatus] = useState<Status>('idle');
  const [visible, setVisible] = useState(false);
  const thinkingRef = useRef('');
  const replyRef = useRef('');

  useEffect(() => {
    const reset = () => {
      setThinking('');
      setReply('');
      thinkingRef.current = '';
      replyRef.current = '';
    };

    const subs = [
      listen<string>('show-recommendation', (e) => {
        reset();
        setReply(e.payload);
        setStatus('done');
        setVisible(true);
      }),
      listen('hide-recommendation', () => {
        reset();
        setVisible(false);
        setStatus('idle');
      }),
      listen('stream-start', () => {
        reset();
        setStatus('streaming');
        setVisible(true);
      }),
      listen<string>('stream-chunk', (e) => {
        try {
          const msg = JSON.parse(e.payload);
          if (msg.type === 'thinking') {
            thinkingRef.current += msg.text;
            setThinking(thinkingRef.current);
          } else if (msg.type === 'content') {
            replyRef.current += msg.text;
            setReply(replyRef.current);
          }
        } catch {
          replyRef.current += e.payload;
          setReply(replyRef.current);
        }
      }),
      listen('stream-done', () => setStatus('done')),
      listen<string>('stream-error', (e) => {
        setReply(e.payload);
        setStatus('error');
      }),
    ];
    return () => { subs.forEach((s) => s.then((f) => f())); };
  }, []);

  if (!visible) return null;

  const trimmedReply = reply.trim();
  const streaming = status === 'streaming';

  return (
    <div className={`ov ov--${status}`}>
      <div className="ov__body">
        {thinking && (
          <p className="ov__thinking">{thinking}</p>
        )}

        {trimmedReply ? (
          <p className={`ov__reply ${status === 'error' ? 'ov__reply--error' : ''}`}>
            {trimmedReply}
            {streaming && <span className="caret" aria-hidden="true" />}
          </p>
        ) : streaming ? (
          <p className="ov__waiting">
            {thinking ? '补全中' : '读取对话'}
            <span className="caret" aria-hidden="true" />
          </p>
        ) : null}
      </div>

      <footer className="ov__foot">
        {status === 'done' && <span className="ov__hint"><span className="ov__ok">已复制</span> · ⌘V 粘贴 · esc 关闭</span>}
        {streaming && <span className="ov__hint">生成中</span>}
        {status === 'error' && <span className="ov__hint ov__hint--error">出错了 · esc 关闭</span>}
      </footer>
    </div>
  );
}

export default App;
