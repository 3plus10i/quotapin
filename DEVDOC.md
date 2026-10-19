# QuotaPin 工程说明

面向维护者：技术选型理由、代码结构、数据流、轮询与触发逻辑、通知状态机、托盘状态设计、UI 规格、渲染管线、构建与测试。

---

## 1. 项目速览

**一句话**：一个 610 KB 的单文件 Windows 程序，常驻托盘，显示 Codex 的 5 小时 / 7 天额度余量、重置时间与重置机会，并在关键时刻发通知。

**硬指标与达成手段**

| 指标 | 目标 | 达成 | 靠什么 |
| --- | --- | --- | --- |
| 单文件体积 | 尽可能小 | **610 KB** | Rust + `opt-level="z"` + LTO + `panic=abort` + `strip`；零 UI 框架、零 TLS 库（用系统 WinHTTP） |
| 空闲内存 | < 10 MB | **1–2.5 MB** | 无 WebView / 无 .NET / 无 GC；空闲时 `SetProcessWorkingSetSize` 主动归还工作集 |
| CPU | 低 | **0.08%** | 一个 250 ms 心跳线程（睡醒多数拍什么都不做）+ 每 30 s 一次重绘 |
| 部署 | 免安装 | **双击即用** | 单 exe，不写 Program Files，只留一份 `state.json`；开机自启靠 HKCU Run 键，菜单里开关 |

**技术栈**：Rust 2021 + `windows-sys`（纯 FFI 声明）+ `serde_json`。没有任何 UI 框架、没有 WebView、没有 WinRT。界面是一个约 600 行的自绘软件光栅器（`canvas.rs`）画出来的，GDI 只负责把字栅格化到临时 DIB 上。

**数据链**（只读，两个 GET）

```
%CODEX_HOME%\auth.json  →  tokens.access_token / account_id
        ↓  Authorization + ChatGPT-Account-Id 头
GET chatgpt.com/backend-api/wham/usage                → 5h/7d 的 used%、reset_at、可用重置机会数
GET chatgpt.com/backend-api/wham/rate-limit-reset-credits → 每个机会的 id / expires_at
        ↓  serde_json → Snapshot
        ↓  build_view → View（UI 只认 View）
托盘图标 + 面板 + 小窗
```

不写文件、不改凭证、不上报数据；取数字段与 Codex 客户端自己用的一致。

**三个界面，一份数据**

| 界面 | 何时出现 | 承载什么 |
| --- | --- | --- |
| 托盘图标（16px 沙漏） | 一直在 | 上半 = 5h 档位色，下半 = 7d 档位色，两半独立变色；整枚变灰 = 数据过期或请求失败 |
| 面板（300dp 卡片） | 单击托盘 / 单击小窗 | 两行完整数据 + 底部重置机会；行可点击切换「剩余 / 已用」 |
| 小窗（158×50 卡片） | Codex/ChatGPT 在前台，或用户勾了「小窗常驻」 | 两条迷你进度条；可拖动、位置记忆、单击弹面板 |

**轮询与触发**：单 worker 线程，250 ms 心跳。取数间隔 `codex.exe`/`chatgpt.exe` 在跑 180 s、没跑 1800 s、失败 60 s 重试 3 次。前台探测约 1 Hz，只在状态翻转时 `PostMessage`。线程间只走 `Mutex<Shared>` + `PostMessage`，UI 线程从不碰网络。

**通知状态机**：每个窗口记一个三元组 `Mark { bucket, used, reset_at }` 到 `state.json`。

- `bucket = clamp(ceil(remaining/20), 0..5)`；档位**下降** → 「额度剩余 n%」；降到 0 → 「额度已用尽」+ 警告图标。
- **重置判定不看档位**，看 `reset_at` 前移（或 `used` 断崖式回落 20 个点以上）——这是关键修正，理由见 §5.2。
- 储备重置机会：到期前 3 天内、当天 10:00 提醒一次，按 `id` 去重，一生成只响一次。
- 首次观测一律静默记录，所以启动不会补发历史通知。

**渲染管线**（三步）

```
Canvas（Vec<u32>，预乘 BGRA）
  → 圆角矩形 / 圆环 / 阴影 / 进度条 / 三角形：全用 SDF 或 3×3 超采样求覆盖率
  → 文字：目标像素拷进临时 DIB → GDI 用最终颜色就地压字 → 逐通道 lerp 回写（DrawTextW 写不进 alpha 通道）
Surface（CreateDIBSection）→ UpdateLayeredWindow(ULW_ALPHA) → WS_EX_LAYERED 窗口
```

所有度量写成 dp 常量，统一乘 `GetDpiForWindow/96`，并开 `PER_MONITOR_AWARE_V2`，150% 缩放下字仍清晰、面板尺寸正确。

**持久化**：`%APPDATA%\QuotaPin\state.json`（原子写：`.tmp` + `rename`）。存小窗位置、常驻开关、显示口径、通知去重用的 `marks`、已提醒的机会 `id`、当天到期检查标记。

**代码地图**（约 4200 行 Rust / 11 个模块，依赖单向无环）

```
main ─→ app ─→ ui ─→ canvas
        │      └──→ fmt
        ├──→ poller ─→ api ─→ sys
        ├──→ tray ─────────→ sys
        ├──→ config
        └──→ sys
```

| 文件 | 行 | 职责 |
| --- | --- | --- |
| `app.rs` | 726 | 三个窗口过程、显隐与定位、菜单、托盘状态刷新 |
| `ui.rs` | 751 | 主题、沙漏标、面板 / 小窗绘制、命中测试、全部度量常量 |
| `canvas.rs` | 697 | 软件光栅器、文字就地绘制、分层窗口 surface |
| `poller.rs` | 515 | 轮询节奏、通知状态机、`Snapshot → View` 投影 |
| `api.rs` | 494 | 凭证读取、WinHTTP、JSON → `Snapshot` |
| `sys.rs` | 419 | Win32 薄封装（窗口、进程、注册表、时间、DPR、内存） |
| `dev.rs` | 284 | 设计期命令：`--render` / `--icon` / `--svg` |
| `tray.rs` | 177 | 托盘图标、提示条、右键菜单、气泡通知 |
| `config.rs` | 147 | `state.json` 读写 + `Mark` 类型 |
| `fmt.rs` | 113 | 百分比 / 时长 / 本地时间格式化 |
| `main.rs` | 72 | 入口、单实例互斥、DPI、CLI |

**建议的阅读顺序**：`poller.rs`（数据怎么变成 View）→ `ui.rs` 的 `paint_popover`（View 怎么变成像素）→ `app.rs` 的 `run()` 与三个 `wndproc`（它们怎么被串起来）→ `canvas.rs`（像素怎么算的）→ `api.rs`（数据从哪来）。

**调试期本事**：`QuotaPin.exe --render <dir>` 用假数据把面板（亮/暗 × 5 种状态）、小窗、logo 渲染成 BMP，`scripts/bmp2png.py` 转 PNG。视觉改动能离线成图、可 diff、可进文档，不必真机上反复点。

---

## 2. 设计取舍

需求里写死了三条硬指标：**体积小、内存 < 10 MB、CPU 低**。这直接排除了主流方案：

| 方案 | 单文件体积 | 空闲内存 | 结论 |
| --- | --- | --- | --- |
| Electron / Tauri(WebView2) | 3 MB / 2 MB + WebView2 运行时 | 80–200 MB | 内存差一个数量级，WebView2 还需预装 |
| .NET WinForms (AOT) | 12–20 MB | 25–40 MB | x64 AOT 下限压不到 10 MB |
| C++ + Win32 | 0.2 MB | 3 MB | 可行，但构建链更重、迭代慢 |
| **Rust + Win32（本项目）** | **0.61 MB** | **1–4.5 MB** | 采用 |

因此：

- **只用 Win32**，不引入任何 UI 框架。依赖只有 `windows-sys`（FFI 声明，不含逻辑）和 `serde_json`（解析 JSON）。
- **界面全部自绘**：一个约 700 行的软件光栅器（`canvas.rs`）+ GDI 只负责把字栅格化。理由见 §9——`DrawText` 写不进 alpha 通道，而分层窗口必须要 alpha；所以文字走「拷进临时 DIB → GDI 就地压字 → 混合回写」，其余形状自己控像素。
- **图标运行时生成**：托盘图标＝一个分成两半的沙漏，颜色随两档余量走，所以必须能在运行时换色；用同一套 `draw_mark()` 既画托盘图标（`CreateIconIndirect`），也画 `assets/icon.ico`（`--icon` 命令），两者永不漂移。
- **通知用托盘气泡**（`Shell_NotifyIcon` + `NIF_INFO`）：Win10/11 会把它渲染成标准 Toast 并进入通知中心，但不需要注册 AUMID、不需要快捷方式、不需要 WinRT——省掉一整套打包机制。
- **不做安装模式**：单文件就是交付形态。开机自启只是 HKCU Run 下的一个键，右键菜单里勾一下就写，取消就删，比"安装"更透明也更好卸载。

---

## 3. 代码结构

```
quotapin/
├─ Cargo.toml            # 单 bin，release: opt-level=z + lto + panic=abort + strip
├─ build.rs              # 调 Windows SDK 的 rc.exe 编资源；找不到就跳过（不失败）
├─ assets/
│  ├─ app.rc             # 图标 + VERSIONINFO
│  ├─ icon.ico           # 由 `QuotaPin.exe --icon assets/icon.ico` 生成，已提交
│  ├─ logo.svg           # 主 logo（圆角深底 + 沙漏），由 --svg --tile 生成
│  └─ logo-mark.svg      # 透明沙漏标，由 --svg 生成（托盘图标同源几何）
├─ scripts/
│  ├─ build.ps1          # 构建 + 测试 + 打 zip
│  ├─ bmp2png.py         # 设计走查：--render 产出的 BMP → PNG
│  └─ grab.py            # 设计走查：抓运行中窗口的像素（无需可见桌面）
├─ src/                  # 模块职责见 §1 代码地图
└─ docs/images/          # README 用图（由 --render 产出）
```

---

## 4. 数据流

```
 p o l l e r   t h r e a d          每 250ms 心跳，到点才干活
 ─────────────────────────────────────────────────────────────
  0. 读凭证    %CODEX_HOME%\auth.json  → access_token / account_id
                （未设置 CODEX_HOME 时 = %USERPROFILE%\.codex\auth.json）

  1. 取数      WinHTTP GET  https://chatgpt.com/backend-api/wham/usage
               WinHTTP GET  https://chatgpt.com/backend-api/wham/rate-limit-reset-credits
               请求头：Authorization / OpenAI-Beta: codex-1 / originator: Codex Desktop
                       OAI-Product-Sku: CODEX / ChatGPT-Account-Id

  2. 解析      serde_json::Value → Snapshot { rows, credits, credits_available, ok, error }

  3. 判定      process_notifications(&snap, &mut cfg)  → Vec<Toast>     （见 §6）
               process_credit_expiry(&snap, &mut cfg)  → Vec<Toast>

  4. 投影      build_view(&snap, &cfg) → View   （UI 只吃 View，不认 Snapshot）

  5. 交接      lock(Shared) { view = ..; toasts += .. }
               PostMessage(msg_hwnd, WM_APP+1)

 ───────────────────────── 跨线程只走 Mutex + PostMessage ─────────
                     │
                     ▼
 u i   t h r e a d                  GetMessage 循环，从不碰网络
 ─────────────────────────────────────────────────────────────
  WM_APP+1   on_data()
               ├─ refresh_tray()   沙漏两半的颜色 + 提示条文案
               ├─ drain_toasts()   Shell_NotifyIcon(NIF_INFO) × N（最多 3 条）
               └─ 面板可见 → paint_popover()；小窗可见 → paint_mini()
                                 → Canvas → Surface → UpdateLayeredWindow

  WM_APP+3   前台进程变化 → show_mini(fg || pinned)
  WM_TIMER   30s          → 重画面板（刷新「x 分钟前」）+ 归还工作集
```

三个关键约束：

1. **UI 线程不碰网络**，worker 线程不碰 HWND（只 `PostMessage`），所以没有锁顺序问题。
2. **View 是唯一的渲染输入**：`ui::paint_*` 从不读全局状态，只吃 `&View`，因此 `--render` 能用假数据离线出图，走查视觉不需要起窗口。
3. **失败也产出 View**：请求失败时保留上一次成功的数据（`last_ok`），并把 `err/err_at` 填进去，界面因此能画成「灰掉的旧数据 + 失败原因」，而不是空白。

---

## 5. 轮询与触发逻辑

worker 是一个 250 ms 心跳的循环，所有定时都是"心跳计数 + 时间戳比较"，不用 `SetTimer`：

```
tick % 4  == 0  →  前台探测（≈1Hz）
now >= next     →  取数（间隔由 §5.1 决定）
tick % 60 == 0  →  刷新 View.now（让「x 分钟前」「x 小时后」不僵死）
```

### 5.1 间隔决策

| 条件 | 间隔 |
| --- | --- |
| 取数成功，`codex.exe`/`chatgpt.exe` 有进程 | `INTERVAL_ACTIVE` = 180 s |
| 取数成功，无相关进程 | `INTERVAL_IDLE` = 1800 s |
| 取数失败且连续失败 ≤ 3 次 | `RETRY_AFTER_ERROR` = 60 s |
| 取数失败且连续失败 > 3 次 | 回到 180 / 1800 |

进程检测走 `CreateToolhelp32Snapshot` + `PROCESSENTRY32W.szExeFile` 全表扫描（约 200 项，微秒级），比逐个 `OpenProcess` 稳。默认名单 `codex.exe`、`chatgpt.exe`、`codex desktop.exe`，可由 `state.json` 的 `extra_procs` 扩展。

### 5.2 前台探测

```
GetForegroundWindow → GetWindowThreadProcessId → OpenProcess(QUERY_LIMITED)
→ QueryFullProcessImageNameW → basename.lowercase → 命中名单？
```

只在**状态翻转**时 `PostMessage(WM_APP+3)`，所以静止时没有任何 UI 唤醒。小窗的显隐没有额外延迟（即时响应），因为 alt-tab 回来的第一帧就该看到它。

### 5.3 手动刷新与打开面板

- 菜单「立即刷新」→ `Shared.force = true` → 下一拍（≤250 ms）立即取数。
- 打开面板**不**强制刷新：面板显示的是最新快照 + 「更新 x 分钟前」，避免每次点击都打一次接口。要最新数据，点「立即刷新」，或等 3 分钟的自然节奏。

### 5.4 错误分类

| HTTP | 界面文案 | 处理 |
| --- | --- | --- |
| 401 | `HTTP 401：登录已失效，请重新登录 Codex` | 60 s 重试 ×3，之后按常规间隔（重新登录后自愈） |
| 403 | `HTTP 403：当前账号无权访问该接口` | 同上 |
| 404 | `HTTP 404：接口路径可能已变化` | 同上 |
| 其他 | `请求失败：HTTP {code}` | 同上 |
| 本地 | `找不到 Codex 凭证 / auth.json 解析失败 / 发送请求失败（errno）` | 同上 |

---

## 6. 通知状态机

### 6.1 20% 节点

定义剩余量档位：

```
bucket(remaining) = clamp(ceil(remaining / 20), 0..5)      剩余 100 → 5
                                                            剩余  80 → 4
                                                            剩余  60 → 3
                                                            剩余  40 → 2
                                                            剩余  20 → 1
                                                            剩余   0 → 0
```

`ceil` 而不是 `floor`，是因为「跌破 80%」这一刻的读数通常是 79.x，`ceil` 会把它归到 80 这一档，正好对应"跌破整 20% 节点"的语义。

每个窗口在 `state.json.marks` 里记一个三元组，键为 `row.label`（如 `"5 小时"`）：

```rust
Mark { bucket: i32, used: f64, reset_at: i64 }
```

每拍按下面的顺序判定，**先判重置，再判档位**：

| 条件 | 行为 |
| --- | --- |
| 没有历史记录 | **静默**记录。首次启动不补发历史通知 |
| `reset_at` 前移 > 120 s，或 `used` 回落 ≥ 20 个点 | 发「额度已重置」 |
| 否则 `bucket` 下降 | 发 `剩余 {bucket×20}%`；`bucket == 0` 改发「额度已用尽」+ 警告图标 |
| 其余 | 无 |
| 无论如何 | 把本次的 `Mark` 写回 |

去重靠这份 `marks`，所以重启、崩溃、休眠都不会重发。

### 6.2 为什么重置判定不能只看档位

档位是比较"剩余量落在哪个 20% 区间"，它对重置**不敏感**：一个周期里如果只用了 3%，剩余 97% → 重置后剩余 100%，两次都落在 `bucket = 5`，档位从未变化，`b > prev` 永远不成立，重置通知就永远发不出来。

所以重置必须由**窗口自身的身份**判定，也就是接口给的 `reset_at`：窗口翻篇后它必然前移（滚动窗口 = 上一周期起点 + 窗口长度），与用掉多少额度无关。实际实现取两个信号：

```rust
fn rolled_over(prev: &Mark, used: f64, reset_at: i64) -> bool {
    if reset_at > 0 && prev.reset_at > 0 && reset_at > prev.reset_at + 120 { return true; }
    used + 20.0 <= prev.used   // 兜底：reset_at 没动但用量断崖式回落
}
```

- 120 s 容差是为了吸收服务端时间戳的抖动，同时也避免把「重置时间被向上微调几分钟」误判成翻篇。
- 兜底分支处理 `reset_at` 缺失或不变、但用量明显回落的极端情况（例如用掉一次储备重置机会）。
- 副作用是**一次重置只报一次**：翻篇后 `marks` 立刻更新成新的 `reset_at`，下一拍不再满足条件。
- 用掉储备重置机会导致用量归零、但 `reset_at` 没变时，兜底分支同样会报「已重置」——语义上没错。

单测 `reset_is_noticed_even_when_usage_never_reached_20_percent` 就锁这条：3% → 1% 的轻量周期，档位全程为 5，仍必须报重置。

### 6.3 储备重置机会到期

poller 每拍检查，条件是三个都成立才触发：

1. 本地时间 `hour == 10`（10:00–10:59 之间任意一拍）
2. `state.json.expiry_day != 今天`（当天只跑一次；跑过就写今天，无论有没有提醒）
3. 存在 `status == "available"` 且 `0 < expires_at - now <= 3 天` 且不在 `warned_credits` 里的机会

一次提醒覆盖所有符合条件的：标题 `N 次重置机会即将到期`，正文「最早 MM-DD hh:mm 到期」，若只有一条则改用接口返回的 `title`（如 `Full reset (Weekly + 5 hr)`）。触发后把这些 `id` 写进 `warned_credits`——**每个机会一生只提醒一次**。

### 6.4 通道与文案

`Shell_NotifyIconW(NIM_MODIFY, NIF_INFO)`，Win10/11 渲染为 Toast 并留存在通知中心。

| kind | 图标 | 用在哪 |
| --- | --- | --- |
| 0 | 信息 | 剩余 80/60/40% |
| 1 | 警告 | 剩余 20% / 用尽 / 机会到期 |
| 2 | 应用图标 | 额度已重置 |

文案模板（`·` 分隔，不换行；`fmt::pct` 自带 `%`，格式串里不要再写一次）：

```
剩余节点   {窗口}额度剩余 {n}%     /  已用 {x}% · 重置 {hh:mm}（{2 小时 46 分后}）
用尽       {窗口}额度已用尽        /  重置 {hh:mm}（{...}）
重置       {窗口}额度已重置        /  已用 {x}% · 下次重置 {hh:mm}
机会到期   {n} 次重置机会即将到期  /  最早 {MM-DD hh:mm} 到期 · 共 {n} 次可用
```

一轮最多发 3 条（`drain_toasts` 里截断），避免异常情况下刷屏。

---

## 7. 托盘状态设计

托盘的每一像素都在传信息，没有纯装饰：

| 元素 | 含义 |
| --- | --- |
| 沙漏上半颜色 | 5 小时窗口的余量档（绿/琥珀/红/灰） |
| 沙漏下半颜色 | 7 天窗口的余量档 |
| 两半同时变灰 | 最近一次请求失败，或数据超过 10 分钟未更新 |
| 提示条 | `QuotaPin · 5h 80% · 7d 92% · 重置机会 3 次`；失败时追加 `· 更新失败` |
| 左键 | 开合面板 |
| 右键 | 菜单（勾选态即当前配置） |

为什么是沙漏：一个 16px 的图标只能承载"一个形状"，而这里要同时表达两个独立数字。沙漏天生分成上下两半（上半 = 5 小时，下半 = 7 天），中间收腰让两半互不干扰，形状本身还暗合"额度在流逝"的意思。上半吃 5h 的颜色，下半吃 7d 的颜色，两半独立变色。

几何上刻意避开"两根柱子 + 两个三角形"的做法：每一半由 `glass_outline()` 采样成一个闭合多边形——边缘是圆角、侧壁略微内凹、收到腰部留一小段直颈——所以没有尖角、没有直线拼接，缩到 16px 也不会退化成"蝴蝶结"。`--icon` 与 `--svg` 吃的是同一份点，exe 图标、托盘图标、`logo.svg` 三者永不漂移。

档位颜色表（同时用于 UI 进度条）：

| 剩余 | 常量 | 值 |
| --- | --- | --- |
| ≥ 40 | `ok` | `#2FA85B` |
| 20–40 | `warn` | `#C08A0E` |
| < 20 | `crit` | `#E04B4B` |
| 未知/失败 | `neutral` | 亮色主题 `#B6BCC6`，暗色 `#555C67` |

图标按 `GetSystemMetrics(SM_CXSMICON)`（通常 16 或 20）渲染，`CreateDIBSection` + `CreateIconIndirect` 直接产出带 alpha 的 HICON；只在档位变化时重建（`icon_key` 比较），旧 HICON 在 `NIM_MODIFY` 之后立刻 `DestroyIcon`，避免 GDI 句柄累积。

**面板贴边定位**：`Shell_NotifyIconGetRect` 拿图标矩形。注意它返回 `S_FALSE` 表示图标被收在「^」溢出区里——此时矩形依旧可用（指向溢出按钮旁），所以 `hr >= 0` 就采用，只有失败才退回光标位置。随后按工作区（`MonitorFromPoint` + `GetMonitorInfoW.rcWork`）夹取，任务栏在上方时自动翻到图标下方。

---

## 8. UI 规格

### 8.1 面板（popover）

宽 300dp，高度随行数自适应。

```
   ● Codex 余量                    更新 刚刚

   5 小时                             80%
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━        ← 4dp 细条
   重置 15:46 · 3 小时 52 分后

   7 天                               92%
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━        ← 8dp 粗条（2 倍）
   重置 10-26 09:03 · 6 天 21 小时后

   ────────────────────────────────────
   重置机会 3 次      最近到期 10-23 02:13
```

| 区块 | 度量 | 字体 / 颜色 |
| --- | --- | --- |
| 卡片 | 300dp 宽，圆角 12，左右内边距 14，窗口四周另留 13dp 阴影边距 | 底 `#FFFFFF` / `#1B1E23`，1px 描边 |
| 头部 | y=13，高 18；左侧状态点直径 6.4dp | 标题 12.5dp/600；右侧状态 11.5dp/400 灰 |
| 行 | 首行 y=41，行高 56，行间距 10 | 标签 12dp/400 灰；重置说明 11.5dp/400；数值 21dp/**Light**（字号拉开层级，字重不加粗） |
| 进度条 | y = 行首 + 25，宽 = 300 − 28，两端全圆角 | 轨道 `#E8EAEE` / `#2E333A`，填充按 `severity()` |
| 进度条厚度 | 5 小时 = **4dp**，7 天 = **8dp**（`bar_height(limit_secs)`：`≥ 86400 s` 走粗条） | 一粗一细，两行不用读标签也能区分 |
| 重置行 | y = 行首 + 39（`ROW_H − 17`，两行文字共用同一基线节奏） | 10.5dp/400，`#9AA1AC` / `#6E7683` |
| 分隔线 | 1px，距上下各 8dp | `#EDEFF2` / `#2B2F36` |
| 底部 | 高 30 | 左「重置机会」11.5dp/400 + 次数 12dp/600；右侧 11.5dp/400 |
| 阴影 | SDF 距离场三次方衰减，spread 13dp | 亮 `rgba(20,24,32,0.17)`，暗 `rgba(0,0,0,0.35)` |

高度公式：`41 + n×56 + (n−1)×10 + 8 + 1 + 30 + 12`，两行 = 214dp（窗口 214 + 上下阴影 26 = 240dp）。

粗细条是**唯一**用来区分两个窗口的视觉手段：不靠分隔线、不靠图标、不靠额外文案。`bar_height()` 由 `RowView.limit_secs` 决定，所以将来接口多加一个日窗口时它会自动变成粗条，不需要改绘制代码。

### 8.2 小窗（mini）

卡片 158×50，圆角 11，底色 92–93% 不透明。

```
   5h   ━━━━━━━━━━━━━━━━            80%     ← 3dp
   7d   ━━━━━━━━━━━━━━━━━━━━━━      92%     ← 6dp
```

- 行 y 坐标：11 / 30；进度条以行内文字中线为轴居中，几何 `x ∈ [34, 112]`，两端全圆角；数值右对齐到 `147`
- 厚度与面板同源：`mini_bar_height()` 给出 3dp / 6dp（同样是 2 倍关系）
- 字体：标签 10dp/Semibold 灰，数值 13dp/400（低于 40% 才染色）
- 默认落点：工作区右下角内缩 14dp；拖动后位置写入 `state.json.mini_pos`，按所在显示器工作区夹取
- 拖动判定：位移 > 3px 才算拖动，否则算点击

### 8.3 交互与信息承载

| 元素 | 静止 | hover | click |
| --- | --- | --- | --- |
| 面板行 | 无底色 | `hover` 底色圆角块（亮 `#F4F5F7` / 暗 `#24282E`） | 切换「剩余/已用」，数字与条同时切换，标签补 `· 已用` |
| 面板底部 | 灰字 | 同上底色 | 同「切换」，方便误触时也有反馈 |
| 小窗 | 92% 底 + 1px 边 | 底 +2~4% 提亮，边 +15% 提亮 | 拖动 / 弹面板 |
| 托盘图标 | 双半颜色 | 系统 tooltip | 左键开合、右键菜单 |
| 整块面板 | — | — | 点击外部失焦即收起（`WM_ACTIVATE` = `WA_INACTIVE`） |

`clamped` 细节：点击托盘收起面板时会先触发失焦收起，再收到 `WM_LBUTTONUP`；用 `last_pop_hide` 时间戳在 260 ms 内吃掉这次"重复打开"，否则会出现"点一下关不掉"。

---

## 9. 渲染管线

```
Canvas（Vec<u32>，预乘 BGRA）
  ├─ round_rect / ring_round / shadow  → 圆角矩形 SDF，coverage = clamp(0.5-d, 0, 1)
  ├─ polygon                           → 3×3 超采样的奇偶规则扫描线填充（沙漏两半的轮廓）
  ├─ bar / circle                      → 组合上面的原语
  └─ text                              → 目标像素拷进临时 DIB → GDI 就地压字 → 逐通道混合回写
        ↓
  Surface（CreateDIBSection，窗口大小的持久 DIB）
        ↓  memcpy + UpdateLayeredWindow(ULW_ALPHA, AC_SRC_OVER/AC_SRC_ALPHA)
  分层窗口 (WS_EX_LAYERED)，按像素 alpha 合成
```

三个实现要点：

1. **预乘 alpha**：所有混合都是 `dst = src·a + dst·(1-a)`，边界正确、可叠加，`UpdateLayeredWindow` 也正好要预乘数据。
2. **文字的渲染路径**：GDI 的 `DrawTextW` 只写 RGB、不写 alpha，直接画到 ARGB 位图上会得到透明字；而"白字黑底取亮度当蒙版"会丢掉 hinting 之外的一切，小字看起来像放大过头的位图。现在的做法是**让 GDI 直接往目标像素上画**：把文字框内的 Canvas 像素（反预乘成直通 alpha）拷进临时 DIB，用最终颜色 `SetTextColor` 画上去，再按 `c.a` 做逐通道 lerp 写回。逐通道而非统一 alpha，是为了让半透明文字也保留 GDI 的次像素覆盖。前提是文字底下必须不透明（两个界面都先铺卡片），这也是回写能是一次简单 lerp 的原因。
3. **为什么不用 ClearType**：子像素抗锯齿的前提是"背景已知、像素不被二次缩放"。这里表面由 DWM 按显示缩放合成，11–12px 的汉字只有 1px 笔画，彩色边缘读起来是噪点而不是锐度。改动时做过对照（同一份 UI 只切 `lfQuality` 渲两遍，用 `scripts/crop.py` 放大 6–8 倍比 21dp 数字、面板正文、小窗标签）：灰度抗锯齿在每个字号上都更均匀。所以字体质量是 `ANTIALIASED_QUALITY`。
4. **DPI**：`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，所有度量都是 dp × `scale`（`GetDpiForWindow / 96`），字号按缩放后像素值创建。字体族必须按名字选（`Segoe UI Light` / `Semibold`），因为 GDI 的 `lfWeight` 只在族内匹配，300/600 会退化成 Regular/Bold。

命中测试用同一套 dp 常量（`ui::hit_test`），所以视觉与点击区域不会错位；动过布局常量只需改一处。

---

## 10. 状态文件

`%APPDATA%\QuotaPin\state.json`（原子写：先写 `.tmp` 再 `rename`）：

| 字段 | 类型 | 用途 |
| --- | --- | --- |
| `mini_pos` | `[x, y]` | 小窗左上角屏幕坐标；`null` = 用默认右下角 |
| `pinned` | bool | 「小窗常驻」 |
| `show_used` | bool | 数字显示口径（剩余 / 已用），点击面板行切换 |
| `marks` | `{窗口名: {bucket, used, reset_at}}` | 通知去重 + 重置判定的依据（见 §6.1） |
| `warned_credits` | `[id]` | 已提醒过的重置机会 ID |
| `expiry_day` | `"YYYY-MM-DD"` | 到期检查当天是否已执行 |
| `extra_procs` | `[exe]` | 追加的前台进程名（小写） |

写入时机：用户改了设置（立即）、每次取数成功（`c.save()`，几百字节，SSD 上无感）。

`marks` 由旧版本的 `buckets` 演进而来：单存档位无法判断"低用量周期内的重置"，于是把一个整数换成三元组。旧文件里的 `buckets` 会被忽略，表现为"首次观测静默"，不会误报。

---

## 11. 构建与发布

```powershell
cargo build --release                       # 610 KB
cargo test --release                        # 14 tests
pwsh -NoProfile -File scripts/build.ps1     # + dist/QuotaPin-0.1.0-win64.zip
```

发布包内容：`QuotaPin.exe` + `README.md` + `logo.svg`。没有安装器、没有 `install.cmd`——解压双击就是全部流程。

体积来源与手段：

| 手段 | 效果 |
| --- | --- |
| `opt-level = "z"` + `lto = true` + `codegen-units = 1` | 代码段压到 ~250 KB |
| `panic = "abort"` + `strip = true` | 去掉 unwind 表与符号 |
| 零 UI 框架、零 TLS 库（用系统 WinHTTP） | 省掉 2–3 MB |
| 图标以 DIB 形式内嵌 | 8 个尺寸共 313 KB（其中 256px 占 270 KB；若要更小可只保留 16/32/48/256） |

`build.rs` 会自己找 `rc.exe`：`QUOTAPIN_RC` 环境变量 → `PATH` → `C:\Program Files (x86)\Windows Kits\10\bin\*\x64\rc.exe`（取最高版本）。找不到时打一条 warning 继续编译，只是 exe 没有图标资源。

### 开发期命令

```powershell
QuotaPin.exe --render <dir>   # 用假数据把面板(亮/暗 × 5 种状态)、小窗、logo 渲染成 BMP
QuotaPin.exe --icon <path>    # 重新生成 assets/icon.ico（改了 draw_mark 后必须跑）
QuotaPin.exe --svg <path> [--tile]  # 重新生成 assets/logo.svg / logo-mark.svg（同一份几何）
python scripts/bmp2png.py <dir>       # BMP → PNG（走查用，纯 stdlib）
python scripts/grab.py <out.bmp>      # 抓运行中窗口的像素，不需要可见桌面
python scripts/crop.py in.png out.png x y w h 6   # 裁一块并最近邻放大，逐像素看走查
```

`--render` 的意义：视觉改动能离线成图、可 diff、可进文档，不必在真机上反复点。

改了 `draw_mark()` 之后要按顺序跑 `--icon assets/icon.ico` → `cargo build --release`，因为 exe 的图标资源是在编译期由 `build.rs` 打包进去的。

---

## 12. 测试

`cargo test --release`，14 项，覆盖的都是"会静默出错"的纯逻辑：

| 模块 | 覆盖点 |
| --- | --- |
| `poller` | 档位边界（100/80/79.9/60/40/20/0）、首次静默、跨档只响一次、用尽告警、用量回落判定为重置、**低用量周期的重置仍被 `reset_at` 捕获**、平稳轮询不打扰、短标签映射 |
| `api` | ISO8601 → unix（含闰日）、窗口秒数 → 中文标签、remaining 夹取 |
| `fmt` | 百分比取整规则、时长单位选择、倒计时边界 |

不测的部分：Win32 调用、绘制像素。前者靠运行时实测（见 §11 与 README 的资源占用表），后者靠 `--render` 出图人工走查。

---

## 13. 已知限制

- **只支持 Windows**：WinHTTP + Shell_NotifyIcon + Win32 窗口，没有跨平台抽象。
- **只认 ChatGPT 账号登录**：API key 账号没有这套额度接口。
- **内部接口无契约**：`/backend-api/wham/*` 不是公开 API，改动时表现为 404，界面会显示原因但不会自动降级。
- **小窗判定依赖进程名**：改名版需要手工配置 `extra_procs`；网页版 Codex 不会触发小窗（只看进程，不看标签页）。
- **全屏应用不避让**：小窗是 `WS_EX_TOPMOST`，全屏游戏/视频时会盖在上面（可用「小窗常驻」关闭它）。
- **不处理多账号切换**：以 `auth.json` 当前内容为准，一个进程一份数据。
- **重置判定的时间容差**：服务端把 `reset_at` 微调超过 120 s（例如后台重算了窗口起点）会被当成一次重置；实际很少见，代价是多一条通知而不是漏一条。

可以考虑的后续：`additional_rate_limits` 的多行展示做完整（现在只取 primary window）、面板内一键打开 Codex、把 256px 图标改成 PNG 压缩以再省 270 KB。
