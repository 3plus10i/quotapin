# QuotaPin 工程说明

面向维护者：技术选型理由、代码结构、数据流、轮询与触发逻辑、通知状态机、托盘状态设计、UI 规格、渲染管线、构建与测试。

---

## 1. 项目速览

**一句话**：一个 419 KB 的单文件 Windows 程序，常驻托盘，显示 Codex 的 5 小时 / 7 天额度余量、重置时间与重置机会，并在关键时刻发通知。

**硬指标与达成手段**

| 指标 | 目标 | 达成 | 靠什么 |
| --- | --- | --- | --- |
| 单文件体积 | 尽可能小 | **419 KB** | Rust + `opt-level="z"` + LTO + `panic=abort` + `strip`；零 UI 框架、零 TLS 库（用系统 WinHTTP） |
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
| 托盘图标（16px 沙漏） | 一直在 | 产品标 + 5h 余量的粗档位（满 / 半 / 见底），见 §7 |
| 面板（300dp 卡片） | 单击托盘 / 单击小窗 | 两行完整数据 + 底部重置机会；行可点击切换「剩余 / 已用」 |
| 小窗（158×50 卡片） | Codex/ChatGPT 在前台，或用户勾了「小窗常驻」 | 两条迷你进度条；可拖动、位置记忆、单击弹面板 |

**轮询与触发**：单 worker 线程，250 ms 心跳。取数间隔 `codex.exe`/`chatgpt.exe` 在跑 180 s、没跑 1800 s、失败 60 s 重试 3 次。前台探测约 1 Hz，只在状态翻转时 `PostMessage`。线程间只走 `Mutex<Shared>` + `PostMessage`，UI 线程从不碰网络。

**通知状态机**：每个窗口记一个四元组 `Mark { bucket, used, reset_at, at }` 到 `state.json`。

- `bucket = clamp(ceil(remaining/20), 0..5)`；档位**下降** → 「额度剩余 n%」；降到 0 → 「额度已用尽」+ 警告图标。
- **重置判定不看档位**，看 `reset_at` 是否**比时间走得更快**（前移量 > 经过时间 + 30 s），或 `used` 断崖式回落 20 个点以上——理由见 §6.2。
- 储备重置机会：到期前 3 天内、当天 10:00 提醒一次，按 `id` 去重，一生成只响一次。
- 启动后第一条成功取数发一次「QuotaPin 已启动 + 当前余量」（§6.6）。
- 首次观测一律静默记录，所以启动不会补发历史通知。

**空转的 5h 窗口不显示倒计时**：余量 100 % 且截止时间在一整个窗口之外时，面板写「重置 已就绪，等待开始使用」，因为那种窗口还没启用、时间戳每拍都在平移（§8.1）。

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
| `ui.rs` | 722 | 主题、托盘图标封装、面板 / 小窗绘制、命中测试、全部度量常量 |
| `app.rs` | 744 | 三个窗口过程、显隐与定位、菜单、提示条与图标换档 |
| `canvas.rs` | 768 | 软件光栅器、文字就地绘制、位图缩放、分层窗口 surface |
| `poller.rs` | 602 | 轮询节奏、通知状态机、`Snapshot → View` 投影 |
| `api.rs` | 494 | 凭证读取、WinHTTP、JSON → `Snapshot` |
| `sys.rs` | 419 | Win32 薄封装（窗口、进程、注册表、时间、DPR、内存） |
| `tray.rs` | 200 | 托盘图标、提示条、右键菜单、气泡通知（含 Toast 专用的大图标句柄） |
| `dev.rs` | 156 | 设计期命令：`--render`（离线出图走查） |
| `config.rs` | 209 | `state.json` 读写 + `Mark` 类型 |
| `mark.rs` | 143 | 产品标注解：三档位图怎么取、5h 余量到档位的阈值 |
| `fmt.rs` | 113 | 百分比 / 时长 / 本地时间格式化 |
| `main.rs` | 54 | 入口、单实例互斥、DPI、CLI |

**建议的阅读顺序**：`poller.rs`（数据怎么变成 View）→ `ui.rs` 的 `paint_popover`（View 怎么变成像素）→ `app.rs` 的 `run()` 与三个 `wndproc`（它们怎么被串起来）→ `canvas.rs`（像素怎么算的）→ `api.rs`（数据从哪来）。

**调试期本事**：`QuotaPin.exe --render <dir>` 用假数据把面板（亮/暗 × 5 种状态）、小窗、三档 logo、各尺寸托盘图标渲染成 BMP，`scripts/bmp2png.py` 转 PNG。视觉改动能离线成图、可 diff、可进文档，不必真机上反复点。

---

## 2. 设计取舍

需求里写死了三条硬指标：**体积小、内存 < 10 MB、CPU 低**。这直接排除了主流方案：

| 方案 | 单文件体积 | 空闲内存 | 结论 |
| --- | --- | --- | --- |
| Electron / Tauri(WebView2) | 3 MB / 2 MB + WebView2 运行时 | 80–200 MB | 内存差一个数量级，WebView2 还需预装 |
| .NET WinForms (AOT) | 12–20 MB | 25–40 MB | x64 AOT 下限压不到 10 MB |
| C++ + Win32 | 0.2 MB | 3 MB | 可行，但构建链更重、迭代慢 |
| **Rust + Win32（本项目）** | **0.42 MB** | **1–4.5 MB** | 采用 |

因此：

- **只用 Win32**，不引入任何 UI 框架。依赖只有 `windows-sys`（FFI 声明，不含逻辑）和 `serde_json`（解析 JSON）。
- **界面全部自绘**：一个约 700 行的软件光栅器（`canvas.rs`）+ GDI 只负责把字栅格化。理由见 §9——`DrawText` 写不进 alpha 通道，而分层窗口必须要 alpha；所以文字走「拷进临时 DIB → GDI 就地压字 → 混合回写」，其余形状自己控像素。
- **图标是位图，不是画出来的**：产品标由 `scripts/make_logo.py` 从一张横向三格的源图烘焙成 `assets/mark.bin`（三档 × 16/20/24/32/48，全部预乘）与 `assets/icon.ico`。运行时只是一次带面积滤波的缩放 + `CreateIconIndirect`，不引 WIC、不引 GDI+，托盘图标与 exe 图标同源。
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
│  ├─ icon.ico           # 16~256 多尺寸，128/256 用 PNG 压缩；由 make_logo.py 生成
│  ├─ mark.bin           # 三档沙漏 × 16/20/24/32/48，预乘位图，编译期 include_bytes! 进 exe
│  └─ logo.png           # 256px 透明标，README / 文档配图用（不进发布包）
├─ scripts/
│  ├─ build.ps1          # 构建 + 测试 + 打 zip
│  ├─ make_logo.py       # 源图（横向三格）→ mark.bin / icon.ico / logo.png（用 ffmpeg 解码 PNG）
│  ├─ bmp2png.py         # 设计走查：--render 产出的 BMP → PNG
│  ├─ crop.py            # 设计走查：裁一块并最近邻放大，逐像素看
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
               ├─ refresh_tray()   提示条文案
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

每个窗口在 `state.json.marks` 里记一个四元组，键为 `row.label`（如 `"5 小时"`）：

```rust
Mark { bucket: i32, used: f64, reset_at: i64, at: i64 }
```

`at` 是这一拍的时间。**没有它就无法区分"截止时间跳了"和"截止时间在随时间平移"**，见 §6.2。

每拍按下面的顺序判定，**先判重置，再判档位**：

| 条件 | 行为 |
| --- | --- |
| 没有历史记录 | **静默**记录。首次启动不补发历史通知 |
| `reset_at` 前移量 − 经过时间 > 30 s，或 `used` 回落 ≥ 20 个点 | 发「额度已重置」 |
| 否则 `bucket` 下降 | 发 `剩余 {bucket×20}%`；`bucket == 0` 改发「额度已用尽」+ 警告图标 |
| 其余 | 无 |
| 无论如何 | 把本次的 `Mark` 写回 |

去重靠这份 `marks`，所以重启、崩溃、休眠都不会重发。

### 6.2 为什么重置判定不能只看档位

档位是比较"剩余量落在哪个 20% 区间"，它对重置**不敏感**：一个周期里如果只用了 3%，剩余 97% → 重置后剩余 100%，两次都落在 `bucket = 5`，档位从未变化，`b > prev` 永远不成立，重置通知就永远发不出来。

所以重置必须由**窗口自身的身份**判定，也就是接口给的 `reset_at`：窗口翻篇后它必然前移（滚动窗口 = 上一周期起点 + 窗口长度），与用掉多少额度无关。实际实现取两个信号：

```rust
const RESET_SLACK: i64 = 30;   // ε

fn rolled_over(prev: &Mark, used: f64, reset_at: i64, now: i64) -> bool {
    if reset_at > 0 && prev.reset_at > 0 && prev.at > 0 {
        let elapsed = (now - prev.at).max(0);      // 这一拍离上一拍过了多久
        if reset_at - prev.reset_at - elapsed > RESET_SLACK { return true; }
    }
    used + 20.0 <= prev.used                        // 兜底：reset_at 没动但用量断崖式回落
}
```

**为什么必须减掉经过时间（v0.1.1 修的 bug）**：5h 窗口在**完全没用**的时候不会锚定，接口每次返回的都是 `reset_at = 现在 + 5 小时`（实测：`used_percent = 0`，`reset_at` 恰好是请求时刻 + 18000 s）。也就是说截止时间每一拍都**平移**一个轮询间隔：

| 时刻 | `reset_at` | 与上一拍相比 |
| --- | --- | --- |
| 09:15 | 14:15 | — |
| 09:18 | 14:18 | +180 s |
| 09:21 | 14:21 | +180 s |

老代码写的是 `reset_at > prev.reset_at + 120`，于是**每一拍都判定为"重置"**——3 分钟一条「5 小时额度已重置」，正文永远是"已用 0%"，重置时间一路往后爬。日志里那次刷屏就是这么来的。

平移的位移等于经过时间，真翻篇的位移等于"旧窗口剩余 + 经过时间"（旧窗口到期后新窗口再加满一个周期），两者差着小时级。所以判据从"移动了多少"改成"**比时间走得更快多少**"。

**ε 只能卡在轮询间隔之间**：判据写成 `Δreset − Δt > ε`，ε 就是那个"多出来的位移"。它必须

- **< 最快的轮询间隔**（当前 60 s，即出错重试）。否则一次轮询里的服务端抖动就能凑够 ε；而平移的位移恒等于 Δt、余量为 0，所以只要 ε 小于最短间隔，平移永远不可能被读成跳变。
- **远小于一个窗口长度**（5h）：最慢的间隔是 `INTERVAL_IDLE = 1800 s`，占窗口的 1/10，真翻篇的位移是"旧窗口剩余 + Δt"，量级差着小时，不会和抖动混淆。

取 30 s：比最短间隔 60 s 小一半，比扫描误差（±几秒）大一个数量级。两条边界由单测 `slack_fits_between_polls_and_window` 锁住——改 `RETRY_AFTER_ERROR` / `INTERVAL_IDLE` 时会立刻失败，不用等人肉回忆这条推理。

- `prev.at == 0` 表示这份 `marks` 来自没有 `at` 的旧版本：这一拍跳过时间戳信号（只靠用量回落），并把 `at` 补上，下一拍起恢复正常。
- 兜底分支处理 `reset_at` 缺失或不变、但用量明显回落的极端情况（例如用掉一次储备重置机会）。
- 副作用是**一次重置只报一次**：翻篇后 `marks` 立刻更新成新的 `reset_at`，下一拍不再满足条件。

单测锁了四条：`reset_is_noticed_even_when_usage_never_reached_20_percent`（3% → 1% 的轻量周期，档位全程为 5，仍必须报重置）、`a_sliding_deadline_is_not_a_reset`（空转窗口连转 8 拍必须一条都不发，随后真翻篇仍要发）、`usage_dropping_means_the_window_rolled_over`（用量断崖兜底）、`slack_fits_between_polls_and_window`（ε 与轮询间隔的大小关系）。

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
| 2 | 应用图标（大尺寸，`NIIF_USER \| NIIF_LARGE_ICON`） | 额度已重置 |

**Toast 的图标不走托盘那一份**：托盘图标按 `SM_CXSMICON` 取尺寸，只有 16 / 20 / 24 px，而 Toast 左侧是按 32 dp 画的，把那张小位图拉伸上去就是"糊"的来源。所以 `Tray` 里另存一个 `balloon` 句柄：开机时按 `max(48, 32 dp)` 从 `mark.bin` 生成一次（48 是烘焙好的最大尺寸，不会再被拉伸），并配 `NIIF_LARGE_ICON` 让 shell 按大图标绘制。它只在 kind == 2 时被用到，而 kind == 2 恒为"刚翻篇"（满杯，variant 0），所以不需要跟着托盘换档。

文案模板（`·` 分隔，不换行；`fmt::pct` 自带 `%`，格式串里不要再写一次）：

```
剩余节点   {窗口}额度剩余 {n}%     /  已用 {x}% · 重置 {hh:mm}（{2 小时 46 分后}）
用尽       {窗口}额度已用尽        /  重置 {hh:mm}（{...}）
重置       {窗口}额度已重置        /  已用 {x}% · 下次重置 {hh:mm}
机会到期   {n} 次重置机会即将到期  /  最早 {MM-DD hh:mm} 到期 · 共 {n} 次可用
启动       QuotaPin 已启动        /  5h {x}% · 7d {y}% · 重置机会 {n} 次
```

一轮最多发 3 条（`drain_toasts` 里截断），避免异常情况下刷屏。

### 6.5 静默运行

右键菜单里勾「静默运行」（`state.json.muted`）后**一条通知都不发**，托盘、提示条、面板、小窗照常工作。

- 抑制点在 `App::drain_toasts`，不在 poller：状态机与 `marks` 仍然每轮推进，所以静默期间该记的档位、该认的重置都记着，取消静默后不会补发一串旧通知。
- 队列**照常清空**（取出来直接丢弃）。静默的语义是"现在别告诉我"，不是"先攒着以后一起告诉我"。
- 不影响取数：静默只关通知通道，3 分钟 / 30 分钟的轮询节奏不变，托盘图标该换档还是换。

### 6.6 启动通知

启动后**第一条成功的取数**发一条 `QuotaPin 已启动`（`kind = 0`），正文是两个窗口的当前余量加储备机会次数：

```
QuotaPin 已启动        5h 80% · 7d 92% · 重置机会 2 次
```

- 挂在"第一次 `snap.ok`"而不是进程启动瞬间：否则离线启动会先弹一条空数据，等数据到了再弹一条，重复打扰。第一拍就失败时不发，失败重试成功后补发一次。
- 只发一次（`worker` 里的 `greeted` 标志），与 `marks` 的去重无关——它不是状态变化，是"我起来了"。
- 同样走 `drain_toasts`，所以勾了「静默运行」就不发（§6.5）。
- 与「首次观察静默」（§6.1）不冲突：那条规则管的是**档位/重置**通知，启动通知是独立的一次性事件。

这样设计的好处是 `poller` 不需要知道静默开关——它只负责判断"该发什么"，发不发是界面层的事，测试也就不用把静默塞进状态机。

---

## 7. 托盘状态设计

托盘图标就是产品标（蓝色沙漏），但它**跟着 5 小时余量换内容**：满沙漏 / 半空 / 见底。早前试过按档位染色，16px 下糊成一团，也不像同一个产品；试过在角落加告警圆点，直径占掉图标三分之一，只好放弃。现在的做法换的是**沙子位置**而不是颜色——图形始终是同一个沙漏，肉眼在 16px 下依然分得清上半个球是满的还是空的。

| 5h 剩余 | 托盘图标 | 源图 |
| --- | --- | --- |
| > 80% | 满沙漏（`variant 0`，**默认 logo**，也是 exe / 窗口图标） | 图 1（约 90%） |
| 20–80% | 半空（`variant 1`） | 图 2（约 50%） |
| ≤ 20% | 见底（`variant 2`） | 图 3（约 20%） |

分界点定在 80 / 20，因为三张源图分别画在 ~90 / ~50 / ~20，落在这两个位置上图形依旧"说得准"；再往细里切就需要更多张图，而 16px 也承载不了。

判定只看 **5h** 那一行（按 `limit_secs ≤ 6h` 找行，不按下标，接口以后多给几条额度也不会错位）。7d 变化太慢，用它换图标只会让人以为程序卡住了。

信息由这几处承载，各司其职：

| 元素 | 含义 |
| --- | --- |
| 托盘图标 | 产品标识 + 5h 余量的粗粒度读数（满 / 半 / 见底） |
| 提示条 | `QuotaPin · 5h 80% · 7d 92% · 重置机会 3 次`；失败时追加 `· 更新失败` |
| 左键 | 开合面板（面板里才用颜色表达档位） |
| 右键 | 菜单（勾选态即当前配置）：立即刷新 / 小窗常驻 / 静默运行 / 开机自启 / 退出 |
| 通知气泡 | 20% 节点、重置、储备重置机会到期；勾了「静默运行」就一条不发（见 §6.5） |

档位颜色表（5 小时进度条、百分比、状态点用；7 天条另有固定青色，见 §8.1）：

| 剩余 | 常量 | 值 |
| --- | --- | --- |
| ≥ 40 | `ok` | `#2FA85B` |
| 20–40 | `warn` | `#C08A0E` |
| < 20 | `crit` | `#E04B4B` |
| 未知/失败 | `neutral` | 亮色主题 `#99A1AF`，暗色 `#737C8A` |
| 7 天条（恒定） | `day` | 亮 `#1097B4`，暗 `#3FBED6` |

图标按 `GetSystemMetrics(SM_CXSMICON)`（通常 16 或 20，150% 缩放下 24）取尺寸，从 `mark.bin` 里挑**该档位**最接近的一版做面积滤波缩放，再 `CreateDIBSection` + `CreateIconIndirect` 产出带 alpha 的 HICON。换档由 `icon_key` 守卫：同一档位不会反复重建，`set_icon` 先 `NIM_MODIFY` 再 `DestroyIcon` 旧句柄，所以进程里稳定只有两个图标句柄——托盘那份随档位换，Toast 那份（`balloon`，48 px，见 §6.4）建一次就不动——不会累积。退出时 `remove` 把两个都销毁。

`mark.bin` 的容器是 `"QP2" | 档位数 | 尺寸数 | 尺寸表 | 位图…`：位图按档位分块，块内尺寸升序，每块 `size² × 4` 预乘 BGRA，最大的一版（48）兼任"未烘焙尺寸"的兜底（`--render` 的 128px 走查图就是这么来的）。`make_logo.py` 按源图宽高比切格（2172×724 → 三格 724），三格统一裁成同一个裁切框，所以换档时图标不会忽大忽小。`icon.ico` / `logo.png` 只取 `variant 0`。

**面板贴边定位**：`Shell_NotifyIconGetRect` 拿图标矩形。注意它返回 `S_FALSE` 表示图标被收在「^」溢出区里——此时矩形依旧可用（指向溢出按钮旁），所以 `hr >= 0` 就采用，只有失败才退回光标位置。随后按工作区（`MonitorFromPoint` + `GetMonitorInfoW.rcWork`）夹取，任务栏在上方时自动翻到图标下方。

---

## 8. UI 规格

### 8.1 面板（popover）

宽 300dp，高度随行数自适应。

```
   ● Codex 余量                    更新 刚刚

   5 小时                             80%
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━        ← 4dp，绿/琥珀/红
   重置 15:46 · 3 小时 52 分后

   7 天                               92%
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━        ← 4dp，恒定青色
   重置 10-26 09:03 · 6 天 21 小时后

   ────────────────────────────────────
   重置机会 3 次      最近到期 10-23 02:13
```

| 区块 | 度量 | 字体 / 颜色 |
| --- | --- | --- |
| 卡片 | 300dp 宽，圆角 12，左右内边距 14，窗口四周另留 13dp 阴影边距 | 底 `#FFFFFF` / `#1B1E23`，1px 描边 |
| 头部 | y=13，高 18；左侧状态点直径 6.4dp | 标题 13dp/600；右侧状态 12dp/400 灰 |
| 行 | 首行 y=41，行高 56，行间距 10 | 标签 12.5dp/400 灰；重置说明 12dp/400；数值 21dp/**Light**（字号拉开层级，字重不加粗） |
| 进度条 | y = 行首 + 25，宽 = 300 − 28，高 **4dp**，两端全圆角 | 轨道 `#CFD5DD` / `#3A414B`（要比卡片底暗一档，空槽才看得见）；填充见下 |
| 重置行 | y = 行首 + 39（`ROW_H − 17`，两行文字共用同一基线节奏） | 10.5dp/400，`#9AA1AC` / `#6E7683` |
| 分隔线 | 1px，距上下各 8dp | `#EDEFF2` / `#2B2F36` |
| 底部 | 高 30 | 左「重置机会」12dp/400 + 次数 12.5dp/600；右侧 12dp/400 |
| 阴影 | SDF 距离场三次方衰减，spread 13dp | 亮 `rgba(20,24,32,0.17)`，暗 `rgba(0,0,0,0.35)` |

高度公式：`41 + n×56 + (n−1)×10 + 8 + 1 + 30 + 12`，两行 = 214dp（窗口 214 + 上下阴影 26 = 240dp）。

**空转的 5 小时窗口改说人话**。余量 100 % 且截止时间正好在一整个窗口之外时，重置行不写 `重置 13:01 · 2 小时 46 分后`，而写 `重置 已就绪，等待开始使用`（`ui::READY_LINE`）。理由是那句倒计时在骗人：这种窗口还没被启用，接口每次都用"现在 + 5h"回答它（§6.2），所以时间戳一直在滑、倒计时永远从 5 小时开始。判据在 `ui::not_started()`：只看 5 小时那一行（`limit_secs == 5h`）、余量 ≥ 99.5（正好是大数字四舍五入显示 100 % 的门槛）、且 `reset_at − now ≥ 5h − 300s`。300 s 的余量是给"截止时间只在取数时平移、而 `now` 每次重绘都在走"这个错位留的。真正用起来之后截止时间固定，几分钟内就会自动换回时间戳。7 天条不受影响。

**两条一样粗**。早前让 7 天走 8dp 粗条，本意是"不读标签也能分清"；实际效果是粗条被读成"更重要/更紧急"，而它只是周期不同，反而要花注意力去纠正。现在统一 `BAR_H = 4dp`。

**两条不同色**。区分改由颜色承担：5 小时条按 `severity()` 走绿 / 琥珀 / 红，7 天条固定**青色**（亮 `#1097B4`，暗 `#3FBED6`，`bar_fill()` 里按 `limit_secs ≥ 86400` 判定）。

为什么 7 天不也用红黄绿：那样两条同色，粗细分不出、颜色也分不出，只能回头读标签；而"7 天还剩多少"慢到一周才走完一格，用它的颜色当告警通道本来就没什么信息量。7 天的档位由**右侧数字**表达（低于 40% 自动染色），条只负责"这是哪个窗口"。

**同一档只换内容，不换含义**：`bar_fill()` 里失败/过期（`failing || stale()`）优先返回灰色，所以无论哪条，取不到数时都退回中性灰，不会假装自己还有档位。

### 8.2 小窗（mini）

卡片 158×50，圆角 11，底色**不透明**：亮 `#FBFCFD`，暗 `#1E2228`。

```
   5h   ━━━━━━━━━━━━━━━━            80%     ← 3dp，绿/琥珀/红
   7d   ━━━━━━━━━━━━━━━━━━━━━━      92%     ← 3dp，恒定青色
```

- 行 y 坐标：11 / 30；进度条以行内文字中线为轴居中，几何 `x ∈ [34, 112]`，两端全圆角；数值右对齐到 `147`
- 厚度与面板同源：统一 3dp（`MINI_BAR_H`，比面板细 1dp，因为卡片只有 50dp 高）
- 配色与面板同源：5h 按 `severity()`，7d 青色，取数失败统一灰（都走 `bar_fill()`）
- 字体：标签 10dp/Semibold 灰，数值 13dp/400（低于 40% 才染色）
- 默认落点：工作区右下角内缩 14dp；拖动后位置写入 `state.json.mini_pos`，按所在显示器工作区夹取
- 拖动判定：位移 > 3px 才算拖动，否则算点击
- **底色不透明**：原来用 92–93% alpha，为的是"透一点桌面显得轻"；代价是在深色壁纸上合成出来的底色正好落在灰色槽的明度区间，空槽整段消失。半透明换来的观感提升不值这个信息损失，直接不透明。

### 8.3 交互与信息承载

| 元素 | 静止 | hover | click |
| --- | --- | --- | --- |
| 面板行 | 无底色 | `hover` 底色圆角块（亮 `#F4F5F7` / 暗 `#24282E`） | 切换「剩余/已用」，数字与条同时切换，标签补 `· 已用` |
| 面板底部 | 灰字 | 同上底色 | 同「切换」，方便误触时也有反馈 |
| 小窗 | 不透明底 + 1px 边 | 底 +2~4% 提亮，边 +15% 提亮 | 拖动 / 弹面板 |
| 托盘图标 | 满 / 半 / 见底三档沙漏 | 系统 tooltip | 左键开合、右键菜单 |
| 整块面板 | — | — | 点击外部失焦即收起（`WM_ACTIVATE` = `WA_INACTIVE`） |

`clamped` 细节：点击托盘收起面板时会先触发失焦收起，再收到 `WM_LBUTTONUP`；用 `last_pop_hide` 时间戳在 260 ms 内吃掉这次"重复打开"，否则会出现"点一下关不掉"。

---

## 9. 渲染管线

```
Canvas（Vec<u32>，预乘 BGRA）
  ├─ round_rect / ring_round / shadow  → 圆角矩形 SDF，coverage = clamp(0.5-d, 0, 1)
  ├─ image                             → 位图面积滤波缩放（三档产品标：16/20/24/32/48 已烘焙，其余尺寸落到最大一版）
  ├─ bar / circle                      → 组合上面的原语
  └─ text                              → 目标像素拷进临时 DIB → GDI 就地压字 → 逐通道混合回写
        ↓
  Surface（CreateDIBSection，窗口大小的持久 DIB）
        ↓  memcpy + UpdateLayeredWindow(ULW_ALPHA, AC_SRC_OVER/AC_SRC_ALPHA)
  分层窗口 (WS_EX_LAYERED)，按像素 alpha 合成
```

五个实现要点：

1. **预乘 alpha**：所有混合都是 `dst = src·a + dst·(1-a)`，边界正确、可叠加，`UpdateLayeredWindow` 也正好要预乘数据。
2. **文字的渲染路径**：`DrawTextW` 只写 RGB、不写 alpha，直接画到 ARGB 位图上会得到透明字。做法是**让 GDI 直接往目标像素上画**：把文字框内的 Canvas 像素（反预乘成直通 alpha）拷进临时 DIB，用最终颜色 `SetTextColor` 画上去，解出覆盖率后回写。回写必须在**直通 alpha 空间**混合再重新预乘 —— 小窗卡片当时还是 `a=238` 的半透明底，把直通结果当预乘值写回去，每个文字块后面就留下一个亮白矩形（现在小窗改成不透明了，但这条对任何半透明表面都成立）。
3. **要 ClearType 的形状，不要它的颜色**：`lfQuality` 用 `CLEARTYPE_QUALITY`，因为它拿到的是**轮廓**覆盖率，比灰度 AA 饱满三成左右（实测同一行文字的平均墨量 9.9 → 13.0），而这个项目的"字重太轻"就来自灰度 AA。但它的三条通道覆盖率不同，细笔画上就是橙蓝色边；所以回写时**取变化最大的那个通道反解出单一覆盖率**（`cov = (drawn−back)/(ink−back)`），再用这一份覆盖率重绘三个通道 —— 形状和字重是 ClearType 的，颜色是自己的，没有彩边。
4. **中文必须显式指定字面**：Segoe UI 没有 CJK，靠 GDI 的字体链接会落到宋体，而宋体带点阵字模，12–13px 时 GDI 直接拿去当位图用（不抗锯齿）——"余"的撇捺会断成孤立的点，看起来"不像字"。所以 `Font` 同时持有 `latin` / `cjk` 两个 HFONT，按串里有没有非 ASCII 切换；`measure()` 必须走同一套选择，否则量出来的是另一种字宽，页脚"重置机会 N 次"会错位。
5. **DPI**：`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，所有度量都是 dp × `scale`（`GetDpiForWindow / 96`），字号按缩放后像素值创建。字体族必须按名字选（`Segoe UI Light` / `Semibold`、`Microsoft YaHei UI Light`），因为 GDI 的 `lfWeight` 只在族内匹配，300/600 会静默退化成 Regular/Bold。

命中测试用同一套 dp 常量（`ui::hit_test`），所以视觉与点击区域不会错位；动过布局常量只需改一处。

---

## 10. 状态文件

`%APPDATA%\QuotaPin\state.json`（原子写：先写 `.tmp` 再 `rename`）：

| 字段 | 类型 | 用途 |
| --- | --- | --- |
| `mini_pos` | `[x, y]` | 小窗左上角屏幕坐标；`null` = 用默认右下角 |
| `pinned` | bool | 「小窗常驻」 |
| `muted` | bool | 「静默运行」：不发任何通知（见 §6.5）；旧文件没有这个键时按 `false` 处理 |
| `show_used` | bool | 数字显示口径（剩余 / 已用），点击面板行切换 |
| `marks` | `{窗口名: {bucket, used, reset_at, at}}` | 通知去重 + 重置判定的依据（见 §6.1） |
| `warned_credits` | `[id]` | 已提醒过的重置机会 ID |
| `expiry_day` | `"YYYY-MM-DD"` | 到期检查当天是否已执行 |
| `extra_procs` | `[exe]` | 追加的前台进程名（小写） |

写入时机：用户改了设置（立即）、每次取数成功（`c.save()`，几百字节，SSD 上无感）。

`marks` 由旧版本的 `buckets` 演进而来：单存档位无法判断"低用量周期内的重置"，于是把一个整数换成三元组。旧文件里的 `buckets` 会被忽略，表现为"首次观测静默"，不会误报。

---

## 11. 构建与发布

```powershell
cargo build --release                       # 419 KB
cargo test --release                        # 14 tests
pwsh -NoProfile -File scripts/build.ps1     # + dist/QuotaPin-0.1.0-win64.zip
```

发布包内容：`QuotaPin.exe` + `README.md`。没有安装器、没有 `install.cmd`——解压双击就是全部流程。logo 只在仓库里给文档用，不随包发。

体积来源与手段：

| 手段 | 效果 |
| --- | --- |
| `opt-level = "z"` + `lto = true` + `codegen-units = 1` | 代码段压到 ~250 KB |
| `panic = "abort"` + `strip = true` | 去掉 unwind 表与符号 |
| 零 UI 框架、零 TLS 库（用系统 WinHTTP） | 省掉 2–3 MB |
| 图标以 DIB 形式内嵌 | 8 个尺寸共 94 KB（128/256 用 PNG 压缩），三档托盘标另占 53 KB |

`build.rs` 会自己找 `rc.exe`：`QUOTAPIN_RC` 环境变量 → `PATH` → `C:\Program Files (x86)\Windows Kits\10\bin\*\x64\rc.exe`（取最高版本）。找不到时打一条 warning 继续编译，只是 exe 没有图标资源。

### 开发期命令

```powershell
QuotaPin.exe --render <dir>   # 用假数据把面板(亮/暗 × 6 种状态：normal/warn/low/error/used/ready)、小窗、三档 logo、各尺寸托盘标渲染成 BMP
python scripts/make_logo.py <源图.png> <ffmpeg.exe>
                              # 源图是横向 N 格的沙漏（宽 = N × 高），重新生成
                              # assets/mark.bin / icon.ico / logo.png（换了标才要跑）
python scripts/bmp2png.py <dir>       # BMP → PNG（走查用，纯 stdlib）
python scripts/grab.py <out.bmp>      # 抓运行中窗口的像素，不需要可见桌面
python scripts/crop.py in.png out.png x y w h 6   # 裁一块并最近邻放大，逐像素看走查
```

`--render` 的意义：视觉改动能离线成图、可 diff、可进文档，不必在真机上反复点。

换了标之后：跑一次 `scripts/make_logo.py` → `cargo build --release`，因为 `mark.bin` 是 `include_bytes!`、`icon.ico` 是 `build.rs` 在编译期打包的，不重编不会进 exe。

---

## 12. 测试

`cargo test --release`，25 项，覆盖的都是"会静默出错"的纯逻辑：

| 模块 | 覆盖点 |
| --- | --- |
| `poller` | 档位边界（100/80/79.9/60/40/20/0）、首次静默、跨档只响一次、用尽告警、用量回落判定为重置、**低用量周期的重置仍被 `reset_at` 捕获**、**空转窗口的平移截止时间不算重置**、**ε 与轮询间隔的大小关系**、**启动通知的正文**、平稳轮询不打扰、短标签映射 |
| `mark` | 80/20 换档边界、`mark.bin` 容器完整（魔数 / 档位数 / 尺寸表 / 总长度）、每档每尺寸都能取到正确字节数 |
| `ui` | 三档 × 16/20/24/32/48 都能真的产出 HICON（空句柄会让托盘悄悄沿用旧图）；7 天条恒为青色、不随档位漂移，取数失败时两条一起转灰；**空转 5h 窗口的"已就绪"判据**（含 300 s 余量、99.5 % 门槛、7 天行不受影响） |
| `config` | `muted` 存得住、旧文件缺这个键时按 `false` 读；`marks` / `warned_credits` / `mini_pos` 往返不丢 |
| `api` | ISO8601 → unix（含闰日）、窗口秒数 → 中文标签、remaining 夹取 |
| `fmt` | 百分比取整规则、时长单位选择、倒计时边界 |

不测的部分：绘制像素。前者靠运行时实测（见 §11 与 README 的资源占用表），后者靠 `--render` 出图人工走查。

---

## 13. 已知限制

- **只支持 Windows**：WinHTTP + Shell_NotifyIcon + Win32 窗口，没有跨平台抽象。
- **只认 ChatGPT 账号登录**：API key 账号没有这套额度接口。
- **内部接口无契约**：`/backend-api/wham/*` 不是公开 API，改动时表现为 404，界面会显示原因但不会自动降级。
- **小窗判定依赖进程名**：改名版需要手工配置 `extra_procs`；网页版 Codex 不会触发小窗（只看进程，不看标签页）。
- **全屏应用不避让**：小窗是 `WS_EX_TOPMOST`，全屏游戏/视频时会盖在上面（可用「小窗常驻」关闭它）。
- **托盘图标只有三档**：满 / 半 / 见底（见 §7），只回答"5h 大概还剩多少"；精确读数、7d 状态、取数失败都在提示条和面板里，图标不再细分——16px 放不下更多。
- **换档不区分失败态**：取数失败时沿用最后一次成功的档位，只在提示条追加「更新失败」；否则图标会随着一次次重试跳来跳去。
- **不处理多账号切换**：以 `auth.json` 当前内容为准，一个进程一份数据。
- **空转窗口的截止时间是会动的**：5h 窗口在完全没用的时候，接口每次返回 `reset_at = 现在 + 5h`（见 §6.2），所以面板上的「重置 hh:mm」在不用的时候会一直往后滑，倒计时永远从 5 小时开始——这是服务端的语义，不是本程序的显示错误。真正开始消耗后它才固定下来。

可以考虑的后续：`additional_rate_limits` 的多行展示做完整（现在只取 primary window）、面板内一键打开 Codex、给三个档位做各自的 16px 手工像素稿（现在是面积滤波 + 调色板吸附）。
