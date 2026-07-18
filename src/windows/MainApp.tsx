import { useState } from 'react';
import Settings from './Settings';
import './MainApp.css';

type Page = 'connection' | 'about';

const NAV: { id: Page; label: string }[] = [
  { id: 'connection', label: '连接' },
  { id: 'about', label: '关于' },
];

export default function MainApp() {
  const [page, setPage] = useState<Page>('connection');

  return (
    <div className="shell">
      <aside className="rail" data-tauri-drag-region="">
        <div className="rail__mark" data-tauri-drag-region="">
          <span className="wordmark">Compleo</span>
          <span className="caret" aria-hidden="true" />
        </div>

        <nav className="rail__nav">
          {NAV.map((item) => (
            <button
              key={item.id}
              className={`nav ${page === item.id ? 'nav--on' : ''}`}
              onClick={() => setPage(item.id)}
            >
              <span className="nav__mark" aria-hidden="true" />
              {item.label}
            </button>
          ))}
        </nav>

        <div className="rail__foot" data-tauri-drag-region="">
          <span className="gloss">complēre · 填补</span>
        </div>
      </aside>

      <main className="stage">
        {page === 'connection' && <Settings />}
        {page === 'about' && <About />}
      </main>
    </div>
  );
}

function About() {
  return (
    <div className="about">
      <p className="about__mark">
        Compleo<span className="caret caret--steady" aria-hidden="true" />
      </p>
      <p className="about__gloss">
        拉丁语 <span className="lat">complēre</span> — 填补、使完整
      </p>
      <p className="about__body">
        它读懂你屏幕上的对话，替你补全下一句。
      </p>
      <dl className="about__meta">
        <div>
          <dt>唤起</dt>
          <dd><kbd>⌘</kbd><kbd>.</kbd></dd>
        </div>
        <div>
          <dt>版本</dt>
          <dd>0.1.0</dd>
        </div>
      </dl>
    </div>
  );
}
