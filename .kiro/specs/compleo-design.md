# Compleo - AI Reply Assistant

## 概述

Compleo 是一个 macOS 菜单栏常驻应用，监听用户当前聊天窗口上下文，通过全局快捷键触发 AI 推荐回复，结果写入剪贴板并浮窗展示，用户直接在原应用粘贴使用。

名称来源：拉丁语 "compleo"，意为"完成/填充"，契合补全与回复的核心语义。

---

## 设计决策清单

| 维度 | 决策 |
|------|------|
| 技术栈 | Tauri 2.0 + Rust 后端 + React 前端 |
| 平台 | MVP macOS only，架构预留跨平台抽象层 |
| 截屏/OCR | `apple-vision` crate（macOS Vision Framework Rust 绑定）+ `core-graphics` crate 截屏 |
| 截屏范围 | 优先 AX 定位输入框坐标后截取上方聊天区域，降级截整个活跃窗口 |
| 触发方式 | 全局快捷键 `Cmd + .`（用户可自定义），自动判断模式 |
| 重复触发 | 非 Idle 状态下快捷键触发被忽略 |
| 模式判断 | Accessibility API 读输入框：有内容→补全模式，空→回复模式；AX 失败时降级为 Reply 模式 |
| LLM 调用 | Streaming 输出，OCR 原始文本直接作为上下文，不做预解析结构化 |
| LLM 上下文 | MVP 只用当前截屏 OCR 文本，不拉历史记录（历史只存不用，留给 v0.2） |
| 语言风格 | 由 LLM 从 OCR 上下文自动推断，不做应用级区分 |
| 结构化整理 | 后台定期批量整理（LLM Wiki 理念），非实时，v0.2 |
| 记忆/存储 | SQLite，按应用显示名分组，保留 30 天（MVP 只存不用于实时推理） |
| 输出方式 | 推荐结果写入剪贴板 + 浮窗显示完整推荐内容（streaming 逐字显示） |
| 浮窗行为 | 不抢焦点（overlay），定位在输入框附近，5 秒超时自动消失 |
| 浮窗关闭 | 超时自动消失 / 再次 Cmd+. 关闭旧浮窗 / 全局 Esc 提前关闭 |
| 多 LLM 后端 | 统一抽象接口，MVP 先支持 OpenAI，模型可配置 |
| API Key | 用户自带，费用自负 |
| System Prompt | 内置合理默认值 + 引导语（帮助 LLM 理解 OCR 文本结构），用户可修改 |
| 应用识别 | `NSWorkspace` 获取前台应用显示名 |
| 状态展示 | 菜单栏图标 + 状态指示（空闲/识别中/生成中/就绪/错误） |
| 错误处理 | 浮窗显示可操作的错误信息（配置缺失/网络/API 错误），非静默失败 |
| 默认快捷键 | `Cmd + .`（接受系统冲突，用户可自定义换掉） |
| 权限引导 | 首次启动一次性引导页，授予屏幕录制 + 辅助功能权限，完成后重启 |

---

## 系统架构

```
┌─────────────────────────────────────────────────┐
│                   Tauri Shell                    │
│  ┌───────────┐  ┌───────────┐  ┌────────────┐  │
│  │  Tray &   │  │  Floating │  │  Settings  │  │
│  │  Status   │  │  Window   │  │  Panel     │  │
│  └─────┬─────┘  └─────┬─────┘  └─────┬──────┘  │
│        │ React         │ React        │ React   │
├────────┼───────────────┼──────────────┼─────────┤
│        │    Tauri IPC (invoke/event)   │         │
├────────┴───────────────┴──────────────┴─────────┤
│                  Rust Backend                    │
│  ┌──────────┐ ┌──────────┐ ┌─────────────────┐ │
│  │ Hotkey   │ │ Platform │ │ LLM Orchestrator│ │
│  │ Manager  │ │ Bridge   │ │                 │ │
│  └────┬─────┘ └────┬─────┘ └────────┬────────┘ │
│       │             │                │          │
│  ┌────┴─────────────┴────┐   ┌──────┴────────┐ │
│  │   Platform Abstraction │   │  Storage      │ │
│  │   Layer (PAL)          │   │  (SQLite)     │ │
│  │  ┌─────────────────┐  │   └───────────────┘ │
│  │  │ macOS:          │  │                      │
│  │  │ apple-vision +  │  │                      │
│  │  │ core-graphics + │  │                      │
│  │  │ Accessibility   │  │                      │
│  │  ├─────────────────┤  │                      │
│  │  │ Windows: UIAuto │  │                      │
│  │  │ + WinOCR (v0.3) │  │                      │
│  │  └─────────────────┘  │                      │
│  └────────────────────────┘                      │
└──────────────────────────────────────────────────┘
```

---

## 核心模块职责

### 1. Hotkey Manager
- 注册全局快捷键（默认 `Cmd + .`，用户可改）
- 触发后启动完整推理链路
- 非 Idle 状态下忽略重复触发
- 使用 Tauri 2.0 的 global-shortcut plugin

### 2. Platform Abstraction Layer (PAL)
- `trait PlatformProvider`：统一接口，各平台各自实现
- 职责：截屏、OCR、读取当前前台应用名、读取 focused 输入框内容（Accessibility）
- 截屏策略：优先通过 AX API 定位输入框坐标，截取输入框上方聊天区域；AX 不可用时降级截整个活跃窗口
- MVP 仅实现 `MacOSProvider`，使用 `apple-vision` + `core-graphics` crate

### 3. LLM Orchestrator
- 模式判断：输入框有内容 → 补全模式，AX 读取失败或为空 → 回复模式（降级策略）
- 组装 prompt（system prompt 含引导语 + 当前 OCR 文本 + 草稿）
- Streaming 调用 LLM，逐字返回推荐文本
- 统一抽象 `trait LlmBackend`，MVP 实现 `OpenAiBackend`
- 语言风格由 LLM 从上下文自动推断

### 4. Storage (SQLite)
- 存储每次 OCR 识别的原始文本 + 元数据
- MVP 阶段只存不用于实时推理（为 v0.2 语义检索积累数据）
- 定期后台整理任务（LLM 批量结构化，类似 LLM Wiki 理念）— v0.2
- 30 天自动清理

### 5. Tray & Status
- 菜单栏图标，显示当前状态
- 下拉菜单：设置、历史、退出

### 6. Floating Window
- 触发后弹出，**不抢焦点**（overlay 模式）
- 定位在输入框附近（依赖 AX 坐标，降级时居中）
- Streaming 逐字显示推荐内容
- 用户直接在原应用 `Cmd+V` 粘贴（内容已在剪贴板）
- 5 秒超时自动 fade out
- 全局 Esc 提前关闭 / 再次 `Cmd+.` 关闭旧浮窗
- 错误时显示可操作的错误信息

### 浮窗窗口属性（待 spike 验证）
```
decorations: false    // 无标题栏
always_on_top: true   // 置顶
skip_taskbar: true    // 不在 Dock 显示
focused: false        // 不抢焦点
transparent: true     // 背景透明
```

> ⚠️ Tauri 2.0 的不抢焦点 overlay 窗口能力需首周 spike 验证。若不支持，退回"浮窗抢焦点 → Enter 确认 → 焦点回原应用"方案。

---

## 数据模型 (SQLite)

```sql
-- 对话记录（OCR 原始文本）
CREATE TABLE conversations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    app_name    TEXT NOT NULL,           -- "微信", "钉钉", "Terminal"
    raw_text    TEXT NOT NULL,           -- OCR 原始识别文本
    draft_text  TEXT,                    -- 用户输入框草稿（如有）
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- 定期整理后的结构化记录（v0.2）
CREATE TABLE structured_messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER REFERENCES conversations(id),
    sender          TEXT,               -- 发送者名称
    content         TEXT,               -- 消息内容
    timestamp_hint  TEXT,               -- OCR 中识别到的时间
    created_at      DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- AI 推荐记录（用于学习用户偏好）
CREATE TABLE recommendations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER REFERENCES conversations(id),
    mode            TEXT NOT NULL,       -- "reply" | "complete"
    recommended     TEXT NOT NULL,       -- AI 推荐内容
    accepted        BOOLEAN DEFAULT 0,  -- 用户是否采纳
    created_at      DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- 用户配置
CREATE TABLE config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

自动清理策略：
```sql
DELETE FROM conversations WHERE created_at < datetime('now', '-30 days');
DELETE FROM structured_messages WHERE conversation_id NOT IN (SELECT id FROM conversations);
DELETE FROM recommendations WHERE conversation_id NOT IN (SELECT id FROM conversations);
```

---

## 核心 Rust 接口定义

```rust
// ===== Platform Abstraction =====
#[async_trait]
pub trait PlatformProvider: Send + Sync {
    /// 截取聊天区域截图（优先精确截取输入框上方区域，降级截整个窗口）
    async fn capture_chat_area(&self) -> Result<Screenshot>;

    /// OCR 识别截图中的文字
    async fn ocr(&self, screenshot: &Screenshot) -> Result<String>;

    /// 获取当前前台应用显示名
    fn frontmost_app_name(&self) -> Result<String>;

    /// 读取当前 focused 输入框的文本内容（失败返回 None，降级为 Reply 模式）
    fn read_input_field(&self) -> Result<Option<String>>;

    /// 获取输入框的屏幕坐标（用于浮窗定位）
    fn input_field_position(&self) -> Result<Option<ScreenRect>>;

    /// 写入系统剪贴板
    fn set_clipboard(&self, text: &str) -> Result<()>;
}

// ===== LLM Abstraction =====
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Streaming 生成，通过 channel 逐步返回文本片段
    async fn generate_stream(
        &self,
        request: LlmRequest,
        tx: mpsc::Sender<String>,
    ) -> Result<()>;
}

pub struct LlmRequest {
    pub system_prompt: String,           // 含引导语：帮助 LLM 理解 OCR 文本结构
    pub current_context: String,         // 当前 OCR 文本（唯一上下文来源）
    pub draft: Option<String>,           // 用户草稿（补全模式）
    pub mode: Mode,                      // Reply | Complete
}

pub enum Mode {
    Reply,    // 生成完整回复
    Complete, // 补全用户已输入的内容
}

// ===== Orchestrator =====
pub struct Orchestrator {
    platform: Box<dyn PlatformProvider>,
    llm: Box<dyn LlmBackend>,
    storage: Storage,
    state: AtomicState,  // 状态机，非 Idle 时忽略触发
}

impl Orchestrator {
    /// 主触发流程
    pub async fn trigger(&self, tx: mpsc::Sender<String>) -> Result<Recommendation> {
        // 非 Idle 状态忽略
        if !self.state.is_idle() {
            return Err(Error::Busy);
        }

        self.state.set(State::Capturing);

        // 1. 获取前台应用
        let app = self.platform.frontmost_app_name()?;

        // 2. 截屏 + OCR
        let screenshot = self.platform.capture_chat_area().await?;
        self.state.set(State::Recognizing);
        let ocr_text = self.platform.ocr(&screenshot).await?;

        // 3. 读取输入框判断模式（失败降级为 Reply）
        let draft = self.platform.read_input_field().unwrap_or(None);
        let mode = if draft.as_ref().is_some_and(|d| !d.is_empty()) {
            Mode::Complete
        } else {
            Mode::Reply
        };

        // 4. 组装请求并 streaming 调用 LLM
        self.state.set(State::Generating);
        let request = LlmRequest {
            system_prompt: self.build_system_prompt(),
            current_context: ocr_text.clone(),
            draft: draft.clone(),
            mode,
        };
        self.llm.generate_stream(request, tx).await?;

        // 5. 存储 + 写入剪贴板
        self.storage.save_conversation(&app, &ocr_text, &draft).await?;
        self.platform.set_clipboard(&self.collected_response())?;

        self.state.set(State::Ready);
        Ok(Recommendation { text: self.collected_response(), mode })
    }
}
```

---

## 状态机

```
[Idle] ──快捷键触发──→ [Capturing] ──截屏完成──→ [Recognizing]
         (非Idle忽略)                                  │
                                                  OCR完成
                                                       ↓
[Error] ←──任意阶段失败──  [Generating] ←── 组装prompt + streaming
   │                           │
   └──3秒后自动恢复──→ [Idle]   │ streaming 完成
                               ↓
                          [Ready] ──超时/Esc/再次触发──→ [Idle]
```

菜单栏图标状态映射：
- **Idle**: 灰色图标
- **Capturing / Recognizing / Generating**: 蓝色图标（旋转动画）
- **Ready**: 绿色图标
- **Error**: 红色图标（3秒后自动恢复为 Idle）+ 浮窗显示错误信息

---

## 用户交互流程

```
用户在聊天应用中 → Cmd+. → 截屏+OCR → LLM streaming →
浮窗浮现（不抢焦点，逐字显示）→ 用户直接 Cmd+V 粘贴 → 浮窗 5 秒后自动消失
                                → 或 Esc 关闭浮窗（不满意）
                                → 或再按 Cmd+. 重新触发
```

---

## MVP 项目文件结构

```
compleo/
├── src-tauri/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs              # Tauri 入口
│   │   ├── orchestrator.rs      # 核心调度逻辑
│   │   ├── hotkey.rs            # 全局快捷键注册
│   │   ├── storage.rs           # SQLite 操作
│   │   ├── state.rs             # 状态机定义
│   │   ├── llm/
│   │   │   ├── mod.rs           # LlmBackend trait
│   │   │   └── openai.rs       # OpenAI 实现（streaming）
│   │   ├── platform/
│   │   │   ├── mod.rs           # PlatformProvider trait
│   │   │   └── macos.rs        # macOS 实现 (apple-vision + core-graphics + Accessibility)
│   │   ├── tray.rs              # 菜单栏图标 + 状态
│   │   └── commands.rs          # Tauri IPC commands
│   └── tauri.conf.json
├── src/                          # React 前端
│   ├── App.tsx
│   ├── main.tsx
│   ├── windows/
│   │   ├── FloatingWindow.tsx   # 推荐结果浮窗（streaming 显示）
│   │   └── Settings.tsx         # 设置面板
│   ├── components/
│   │   └── StatusIndicator.tsx
│   └── hooks/
│       └── useTauriEvent.ts     # 监听后端事件
├── package.json
├── tsconfig.json
└── README.md
```

---

## MVP 功能范围

| 功能 | MVP (v0.1) | 后续版本 |
|------|:-----------:|:--------:|
| macOS 菜单栏常驻 | v0.1 | |
| 全局快捷键触发 (`Cmd + .`) | v0.1 | |
| 截屏 + OCR (`apple-vision` + `core-graphics`) | v0.1 | |
| 精确截取聊天区域 | v0.1 | |
| 回复模式 | v0.1 | |
| 补全模式（best-effort，AX 失败降级为 Reply） | v0.1 | |
| LLM Streaming 调用 (OpenAI) | v0.1 | |
| 模型可配置 | v0.1 | |
| 推荐结果 → 剪贴板 + 不抢焦点浮窗 | v0.1 | Ghost text (v0.2) |
| 错误浮窗（可操作信息） | v0.1 | |
| SQLite 历史记录（只存不用于推理） | v0.1 | |
| 设置面板 (API key / 模型 / 快捷键 / System Prompt) | v0.1 | |
| 状态指示 (图标颜色变化) | v0.1 | |
| 首次启动权限引导 | v0.1 | |
| 历史上下文用于 LLM 推理 | | v0.2 |
| 向量语义检索 | | v0.2 |
| 多 LLM 后端切换 UI | | v0.2 |
| 后台定期结构化整理 | | v0.2 |
| 浮窗"换一个/编辑"交互 | | v0.2 |
| Windows 支持 | | v0.3+ |

---

## Dogfood MVP（自用快速验证，4 步）

自用阶段可跳过设置面板、权限引导、状态图标，用 config 文件 / 环境变量代替：

1. **Tauri 脚手架** — 菜单栏常驻 + 全局快捷键 + overlay 浮窗 spike
2. **macOS 截屏 + OCR** — `apple-vision` + `core-graphics`，精确截取聊天区域
3. **LLM Streaming 调用** — OpenAI 对接 + prompt 模板（含 OCR 引导语）
4. **剪贴板 + 浮窗** — 不抢焦点 overlay，streaming 逐字显示，5 秒消失

做到这里即可开始 dogfood。后续按需补充 SQLite、设置面板、状态图标。

---

## 待验证风险（Spike）

| 风险 | 验证方式 | 时机 |
|------|----------|------|
| Tauri 2.0 不抢焦点 overlay 窗口 | 最小 Tauri 项目测试窗口属性组合 | 开发第 1 天 |
| 微信/钉钉 Accessibility API 支持 | 用 Accessibility Inspector 检查输入框 AXValue | 开发第 1 天 |
| `apple-vision` crate 中文 OCR 质量 | 截取微信聊天窗口实测 | 开发第 2 天 |

---

## 隐私与安全

- 所有数据存储在用户本地 (`~/Library/Application Support/Compleo/`)
- **OCR 文本会发送给 LLM 提供商**（OpenAI 等）以生成回复，这是核心功能所必需的
- 除 LLM API 调用外，不向任何第三方服务上传数据
- API Key 存储在本地 SQLite 或 macOS Keychain
- 30 天自动清理历史数据
