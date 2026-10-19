# QuotaPin 工程说明

面向维护者。内容：技术选型理由、代码结构、数据流、轮询与触发逻辑、通知状态机、托盘状态设计、UI 规格、渲染管线、构建与测试。

---

## 1. 设计取舍

需求里写死了三条硬指标：**体积小、内存 < 10 MB、CPU 低**。这直接排除了主流方案：

| 方案 | 单文件体积 | 空闲内存 | 结论 |
| --- | --- | --- | --- |
| Electron / Tauri(WebView2) | 3 MB / 2 MB + WebView2 运行时 | 80–200 MB | 内存差一个数量级，WebView2 还需预装 |
| .NET WinForms (AOT) | 12–20 MB | 25–40 MB | x64 AOT 下限压不到 10 MB |
| C++ + Win32 | 0.2 MB | 3 MB | 可行，但构建链更重、迭代慢 |
| **Rust + Win32（本项目）** | **0.67 MB** | **1–4.5 MB** | 采用 |

因此：

- **只用 Win32**，不引入任何 UI 框架。依赖只有 `windows-sys`（FFI 声明，不含逻辑）和 `serde_json`（解析 JSON）。
- **界面全部自绘**：一个约 570 行的软件光栅器（`canvas.rs`）+ GDI 只负责文字栅格化。理由见 §7——GDI 的 `DrawText` 写不进 alpha 通道，而分层窗口必须要 alpha；与其叠三层折衷，不如自己控像素。
- **图标运行时生成**：托盘图标＝两根额度条，颜色随余量走，所以必须能在运行时换色；用同一套 `draw_logo()` 既画托盘图标（`CreateIconIndirect`），也画 `assets/icon.ico`（`--icon` 命令），两者永不漂移。
- **通知用托盘气泡**（`Shell_NotifyIcon` + `NIF_INFO`）：Win10/11 会把它渲染成标准 Toast 并进入通知中心，但不需要注册 AUMID、不需要快捷方式、不需要 WinRT——省掉一整套打包机制。

---

## 2. 代码结构

```
quotapin/
├─ Cargo.toml            # 单 bin，release: opt-level=z + lto + panic=abort + strip
├─ build.rs              # 调 Windows SDK 的 rc.exe 编资源；找不到就跳过（不失败）
├─ assets/
│  ├─ app.rc             # 图标 + VERSIONINFO
│  ├─ icon.ico           # 由 `QuotaPin.exe --icon assets/icon.ico` 生成，已提交
│  ├─ logo.svg           # 主 logo（圆角深底 + 双柱）
│  └─ logo-mark.svg      # 透明双柱标（托盘图标的几何来源）
├─ scripts/
│  ├─ build.ps1          # 构建 + 测试 + 打 zip
│  ├─ bmp2png.py         # 设计走查：--render 产出的 BMP → PNG
│  └─ grab.py            # 设计走查：抓运行中窗口的像素（无需可见桌面）
├─ src/
│  ├─ main.rs            # 入口、单实例、DPI awareness、CLI
│  ├─ app.rs             # 窗口过程、托盘/小窗/面板的编排、菜单、安装
│  ├─ poller.rs          # 后台线程：轮询节奏、通知判定
│  ├─ api.rs             # auth.json 凭证 + WinHTTP GET + JSON 解析
│  ├─ config.rs          # state.json 读写
│  ├─ canvas.rs          # 软件光栅器 + 字体 + 分层窗口 surface
│  ├─ ui.rs              # 主题、logo、面板与小窗的绘制、命中测试
│  ├─ fmt.rs             # 百分比 / 时长 / 本地时间格式化
│  ├─ sys.rs             # Win32 薄封装（窗口、进程、注册表、时间、内存）
│  ├─ tray.rs            # 托盘图标、提示条、右键菜单、气泡通知
│  └─ dev.rs             # 设计期命令：--render / --icon
└─ docs/images/          # README 用图（由 --render 产出）
```

约 4000 行 Rust（含测试），其中 UI/窗口编排占一半：

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `app.rs` | 760 | 三个窗口过程、显隐与定位、菜单、安装 |
| `ui.rs` | 657 | 主题、logo、面板/小窗绘制、命中测试 |
| `canvas.rs` | 572 | 光栅器、字体、分层窗口 surface |
| `api.rs` | 494 | 凭证、WinHTTP、JSON → Snapshot |
| `poller.rs` | 469 | 轮询节奏、通知状态机 |
| `sys.rs` | 419 | Win32 薄封装 |
| `dev.rs` | 211 | `--render` / `--icon` |
| `tray.rs` | 177 | 托盘图标/提示/菜单/气泡 |
| `config.rs` | 118 | state.json |
| `fmt.rs` | 113 | 百分比/时长/本地时间 |
| `main.rs` | 61 | 入口、单实例、DPI、CLI |

模块依赖是单向的，没有循环：

```
main ─→ app ─→ ui ─→ canvas
        │      └──→ fmt
        ├──→ poller ─→ api ─→ sys
        ├──→ tray ─────────→ sys
        ├──→ config
        └──→ sys
```

---

## 3. 数据流

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

  3. 判定      process_notifications(&snap, &mut cfg)  → Vec<Toast>     （见 §5）
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
               ├─ refresh_tray()   图标双柱颜色 + 提示条文案
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

## 4. 轮询与触发逻辑

worker 是一个 250 ms 心跳的循环，所有定时都是"心跳计数 + 时间戳比较"，不用 `SetTimer`：

```
tick % 4  == 0  →  前台探测（≈1Hz）
now >= next     →  取数（间隔由 §4.1 决定）
tick % 240 == 0 →  刷新 View.now（让「x 分钟前」「x 小时后」不僵死）
```

### 4.1 间隔决策

| 条件 | 间隔 |
| --- | --- |
| 取数成功，`codex.exe`/`chatgpt.exe` 有进程 | `INTERVAL_ACTIVE` = 180 s |
| 取数成功，无相关进程 | `INTERVAL_IDLE` = 1800 s |
| 取数失败且连续失败 ≤ 3 次 | `RETRY_AFTER_ERROR` = 60 s |
| 取数失败且连续失败 > 3 次 | 回到 180 / 1800 |

进程检测走 `CreateToolhelp32Snapshot` + `PROCESSENTRY32W.szExeFile` 全表扫描（约 200 项，微秒级），比逐个 `OpenProcess` 稳。默认名单 `codex.exe`、`chatgpt.exe`、`codex desktop.exe`，可由 `state.json` 的 `extra_procs` 扩展。

### 4.2 前台探测

```
GetForegroundWindow → GetWindowThreadProcessId → OpenProcess(QUERY_LIMITED)
→ QueryFullProcessImageNameW → basename.lowercase → 命中名单？
```

只在**状态翻转**时 `PostMessage(WM_APP+3)`，所以静止时没有任何 UI 唤醒。小窗的显隐没有额外延迟（即时响应），因为 alt-tab 回来的第一帧就该看到它。

### 4.3 手动刷新与打开面板

- 菜单「立即刷新」→ `Shared.force = true` → 下一拍（≤250 ms）立即取数。
- 打开面板**不**强制刷新：面板显示的是最新快照 + 「更新 x 分钟前」，避免每次点击都打一次接口。要最新数据，点「立即刷新」，或等 3 分钟的自然节奏。

### 4.4 错误分类

| HTTP | 界面文案 | 处理 |
| --- | --- | --- |
| 401 | `HTTP 401：登录已失效，请重新登录 Codex` | 60 s 重试 ×3，之后按常规间隔（重新登录后自愈） |
| 403 | `HTTP 403：当前账号无权访问该接口` | 同上 |
| 404 | `HTTP 404：接口路径可能已变化` | 同上 |
| 其他 | `请求失败：HTTP {code}` | 同上 |
| 本地 | `找不到 Codex 凭证 / auth.json 解析失败 / 发送请求失败（errno）` | 同上 |

---

## 5. 通知状态机

### 5.1 20% 节点

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

状态迁移（每个窗口独立，键为 `row.label`，如 `"5 小时"`）：

| 上次 | 本次 | 行为 |
| --- | --- | --- |
| 无 | 任意 | **静默**记录。首次启动不补发历史通知 |
| 同档 | 同档 | 无 |
| 高档 | 低档 | 发通知：`剩余 {bucket×20}%`；`bucket == 0` 改发「额度已用尽」+ 警告图标 |
| 低档 | 高档 | 视为**窗口重置**（用量回落），发「额度已重置」，并把档位更新到新值 |

去重靠 `state.json.buckets`，所以重启、崩溃、休眠都不会重发。

### 5.2 储备重置机会到期

poller 每拍检查，条件是三个都成立才触发：

1. 本地时间 `hour == 10`（10:00–10:59 之间任意一拍）
2. `state.json.expiry_day != 今天`（当天只跑一次；跑过就写今天，无论有没有提醒）
3. 存在 `status == "available"` 且 `0 < expires_at - now <= 3 天` 且不在 `warned_credits` 里的机会

一次提醒覆盖所有符合条件的：标题 `N 次重置机会即将到期`，正文「最早 MM-DD hh:mm 到期」，若只有一条则改用接口返回的 `title`（如 `Full reset (Weekly + 5 hr)`）。触发后把这些 `id` 写进 `warned_credits`——**每个机会一生只提醒一次**。

### 5.3 通道与文案

`Shell_NotifyIconW(NIM_MODIFY, NIF_INFO)`，Win10/11 渲染为 Toast 并留存在通知中心。

| kind | 图标 | 用在哪 |
| --- | --- | --- |
| 0 | 信息 | 剩余 80/60/40% |
| 1 | 警告 | 剩余 20% / 用尽 / 机会到期 |
| 2 | 应用图标 | 额度已重置 |

文案模板（`·` 分隔，不换行）：

```
剩余节点   {窗口}额度剩余 {n}%     /  已用 {x}% · 重置 {hh:mm}（{2 小时 46 分后}）
用尽       {窗口}额度已用尽        /  重置 {hh:mm}（{...}）
重置       {窗口}额度已重置        /  已用 {x}% · 下次重置 {hh:mm}
机会到期   {n} 次重置机会即将到期  /  最早 {MM-DD hh:mm} 到期 · 共 {n} 次可用
```

一轮最多发 3 条（`drain_toasts` 里截断），避免异常情况下刷屏。

---

## 6. 托盘状态设计

托盘的每一像素都在传信息，没有纯装饰：

| 元素 | 含义 |
| --- | --- |
| 左柱颜色 | 5 小时窗口的余量档（绿/琥珀/红/灰） |
| 右柱颜色 | 7 天窗口的余量档 |
| 整图标变灰 | 最近一次请求失败，或数据超过 10 分钟未更新 |
| 提示条 | `QuotaPin · 5h 80% · 7d 92% · 重置机会 3 次`；失败时追加 `· 更新失败` |
| 左键 | 开合面板 |
| 右键 | 菜单（勾选态即当前配置） |

档位颜色表（同时用于 UI 进度条）：

| 剩余 | 常量 | 值 |
| --- | --- | --- |
| ≥ 40 | `ok` | `#2FA85B` |
| 20–40 | `warn` | `#C08A0E` |
| < 20 | `crit` | `#E04B4B` |
| 未知/失败 | `neutral` | 亮色主题 `#B6BCC6`，暗色 `#555C67` |

图标按 `GetSystemMetrics(SM_CXSMICON)`（通常 16 或 20）渲染，`CreateDIBSection` + `CreateIconIndirect` 直接产出带 alpha 的 HICON；只在档位变化时重建（`icon_key` 比较），避免 GDI 抖动。

**面板贴边定位**：`Shell_NotifyIconGetRect` 拿图标矩形。注意它返回 `S_FALSE` 表示图标被收在「^」溢出区里——此时矩形依旧可用（指向溢出按钮旁），所以 `hr >= 0` 就采用，只有失败才退回光标位置。随后按工作区（`MonitorFromPoint` + `GetMonitorInfoW.rcWork`）夹取，任务栏在上方时自动翻到图标下方。

---

## 7. UI 规格

### 7.1 面板（popover）

宽 300dp，高度随行数自适应。

```
   ● Codex 余量                    更新 刚刚

   5 小时                             80%
   ██████████████████░░░░░░░░░░░░░
   重置 15:46 · 3 小时 52 分后

   7 天                               92%
   ██████████████████████████░░░░░
   重置 10-26 09:03 · 6 天 21 小时后

   ────────────────────────────────────
   重置机会 3 次      最近到期 10-23 02:13
```

| 区块 | 度量 | 字体 / 颜色 |
| --- | --- | --- |
| 卡片 | 300dp 宽，圆角 12，左右内边距 14，窗口四周另留 13dp 阴影边距 | 底 `#FFFFFF` / `#1B1E23`，1px 描边 |
| 头部 | y=13，高 18；左侧状态点直径 6.4dp | 标题 12dp/600；右侧状态 10.5dp/400 灰 |
| 行 | 首行 y=41，行高 50，行间距 10 | 标签 11.5dp/400 灰；数值 21dp/600 |
| 进度条 | y = 行首+26，高 5，圆角 2.5，宽 = 300 − 28 | 轨道 `#E8EAEE` / `#2E333A`，填充按 `severity()` |
| 重置行 | y = 行首+35，高 15 | 10.5dp/400，`#9AA1AC` / `#6E7683` |
| 分隔线 | 1px，距上下各 8dp | `#EDEFF2` / `#2B2F36` |
| 底部 | 高 30 | 左「重置机会」10.5dp/400 + 次数 11.5dp/600；右侧 10.5dp/400 |
| 阴影 | SDF 距离场三次方衰减，spread 13dp | 亮 `rgba(20,24,32,0.17)`，暗 `rgba(0,0,0,0.35)` |

高度公式：`41 + n×50 + (n−1)×10 + 8 + 1 + 30 + 12`，两行 = 202dp（窗口 202 + 上下阴影 26 = 228dp）。

### 7.2 小窗（mini）

卡片 158×50，圆角 11，底色 92–93% 不透明。

```
   5h   ██████████░░░░░░░░          80%
   7d   ██████████████████░░        92%
```

- 行 y 坐标：13 / 30；进度条几何：`x ∈ [34, 112]`，高 3dp，圆角 1.5；数值右对齐到 `147`
- 字体：标签 9dp/700 灰，数值 10.5dp/600（低于 40% 才染色）
- 默认落点：工作区右下角内缩 14dp；拖动后位置写入 `state.json.mini_pos`，按所在显示器工作区夹取
- 拖动判定：位移 > 3px 才算拖动，否则算点击

### 7.3 交互与信息承载

| 元素 | 静止 | hover | click |
| --- | --- | --- | --- |
| 面板行 | 无底色 | `hover` 底色圆角块（亮 `#F4F5F7` / 暗 `#24282E`） | 切换「剩余/已用」，数字与条同时切换，标签补 `· 已用` |
| 面板底部 | 灰字 | 同上底色 | 同「切换」，方便误触时也有反馈 |
| 小窗 | 92% 底 + 1px 边 | 底 +2~4% 提亮，边 +15% 提亮 | 拖动 / 弹面板 |
| 托盘图标 | 双柱颜色 | 系统 tooltip | 左键开合、右键菜单 |
| 整块面板 | — | — | 点击外部失焦即收起（`WM_ACTIVATE` = `WA_INACTIVE`） |

`clamped` 细节：点击托盘收起面板时会先触发失焦收起，再收到 `WM_LBUTTONUP`；用 `last_pop_hide` 时间戳在 260 ms 内吃掉这次"重复打开"，否则会出现"点一下关不掉"。

---

## 8. 渲染管线

```
Canvas（Vec<u32>，预乘 BGRA）
  ├─ round_rect / ring_round / shadow  → 圆角矩形 SDF，coverage = clamp(0.5-d, 0, 1)
  ├─ bar / circle                      → 组合上面的原语
  └─ text                              → GDI 白字黑底 DIB → 亮度当 alpha → 手工合成
        ↓
  Surface（CreateDIBSection，窗口大小的持久 DIB）
        ↓  memcpy + UpdateLayeredWindow(ULW_ALPHA, AC_SRC_OVER/AC_SRC_ALPHA)
  分层窗口 (WS_EX_LAYERED)，按像素 alpha 合成
```

三个实现要点：

1. **预乘 alpha**：所有混合都是 `dst = src·a + dst·(1-a)`，边界正确、可叠加，`UpdateLayeredWindow` 也正好要预乘数据。
2. **文字 alpha**：GDI 的 `DrawTextW` 只写 RGB、不写 alpha，直接画到 ARGB 位图上会得到透明字。做法是把字画成"白字黑底"的临时 DIB，取 `max(r,g,b)` 当覆盖率，再用这个覆盖率把目标色合成进 Canvas。字体用 `ANTIALIASED_QUALITY`（灰度抗锯齿）而不是 ClearType，避免彩色子像素污染覆盖率。一次绘制约 10 段文字，临时 DIB 即用即删，代价可忽略。
3. **DPI**：`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，所有度量都是 dp × `scale`（`GetDpiForWindow / 96`），字号按缩放后像素值创建。

命中测试用同一套 dp 常量（`ui::hit_test`），所以视觉与点击区域不会错位；动过布局常量只需改一处。

---

## 9. 状态文件

`%APPDATA%\QuotaPin\state.json`（原子写：先写 `.tmp` 再 `rename`）：

| 字段 | 类型 | 用途 |
| --- | --- | --- |
| `mini_pos` | `[x, y]` | 小窗左上角屏幕坐标；`null` = 用默认右下角 |
| `pinned` | bool | 「小窗常驻」 |
| `show_used` | bool | 数字显示口径（剩余 / 已用），点击面板行切换 |
| `buckets` | `{窗口名: 0..5}` | 通知去重：上次提醒到的档位 |
| `warned_credits` | `[id]` | 已提醒过的重置机会 ID |
| `expiry_day` | `"YYYY-MM-DD"` | 到期检查当天是否已执行 |
| `extra_procs` | `[exe]` | 追加的前台进程名（小写） |

写入时机：用户改了设置（立即）、每次取数成功（`c.save()`，几百字节，SSD 上无感）。

---

## 10. 构建与发布

```powershell
cargo build --release                       # 673 KB
cargo test --release                        # 12 tests
pwsh -NoProfile -File scripts/build.ps1     # + dist/QuotaPin-0.1.0-win64.zip
```

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
QuotaPin.exe --icon <path>    # 重新生成 assets/icon.ico（改了 draw_logo 后必须跑）
python scripts/bmp2png.py <dir>       # BMP → PNG（走查用，纯 stdlib）
python scripts/grab.py <out.bmp>      # 抓运行中窗口的像素，不需要可见桌面
```

`--render` 的意义：视觉改动能离线成图、可 diff、可进文档，不必在真机上反复点。

---

## 11. 测试

`cargo test --release`，12 项，覆盖的都是"会静默出错"的纯逻辑：

| 模块 | 覆盖点 |
| --- | --- |
| `poller` | 档位边界（100/80/79.9/60/40/20/0）、首次静默、跨档只响一次、用尽告警、用量回落判定为重置、短标签映射 |
| `api` | ISO8601 → unix（含闰日）、窗口秒数 → 中文标签、remaining 夹取 |
| `fmt` | 百分比取整规则、时长单位选择、倒计时边界 |

不测的部分：Win32 调用、绘制像素。前者靠运行时实测（见 README 资源占用表），后者靠 `--render` 出图人工走查。

---

## 12. 已知限制

- **只支持 Windows**：WinHTTP + Shell_NotifyIcon + Win32 窗口，没有跨平台抽象。
- **只认 ChatGPT 账号登录**：API key 账号没有这套额度接口。
- **内部接口无契约**：`/backend-api/wham/*` 不是公开 API，改动时表现为 404，界面会显示原因但不会自动降级。
- **小窗判定依赖进程名**：改名版需要手工配置 `extra_procs`；网页版 Codex 不会触发小窗（只看进程，不看标签页）。
- **全屏应用不避让**：小窗是 `WS_EX_TOPMOST`，全屏游戏/视频时会盖在上面（可用「小窗常驻」关闭它）。
- **不处理多账号切换**：以 `auth.json` 当前内容为准，一个进程一份数据。

可以考虑的后续：`additional_rate_limits` 的多行展示做完整（现在只取 primary window）、面板内一键打开 Codex、把 256px 图标改成 PNG 压缩以再省 270 KB。
