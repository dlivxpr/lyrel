# kero／diri 调研与 lyrel 现状核验

- 日期：2026-11-11
- 范围：只读源码核对、现有测试与方案约束；不提出终端解析 Markdown／SVG 的改造。
- lyrel 当前 revision：`b84db3aec9d57e46accf73324260d88154514667`，workspace `0.3.1`。
- 上游 checkout：
  - kero：`09eb1d42dfe24c8ffc07600024e152e096532519`；远端 HEAD 同日仍相同。
  - mygo：`90a1f06eb5c9e2c869c52d8d00cf0fa00f7c7962`。
  - omp：`1379127c078f978c9908059d31f6dc6c16efbe53`；远端 HEAD 已到 `eda28eff…`，但 `terminal-capabilities.ts`、`kitty-graphics.ts`、`svg-figure.ts`、`svg-source.ts` 无相关差异。
  - diri：`0b23e72080ca56b3cda795acb3ffa153eb6f75c8`；远端 HEAD 已到 `45239963…`，但 scrollback 状态机无差异，悬浮框文件只有无关反馈文案／测试变化。

## 1. 总结论

两份原调研的核心机制均真实：

1. kero 截图中的 SVG 图片由 **omp** 识别 fenced SVG、修补半截 XML、异步栅格化为 PNG，再经 Kitty graphics protocol 输出；kero／mygo 负责终端协议解析、状态镜像和位图绘制，不负责理解模型 Markdown。
2. diri 的稳定阅读由 **绝对行锚点 + 离开 live 时的整屏快照 + 已读历史行缓存**共同实现，不只是滚动偏移修正。截图中的 `↓ N 行 · 回到实时输出` 悬浮框也真实存在，`N` 是视口距 live 边缘的行数，不是未读消息数。

但对 lyrel 最关键的补充是：**lyrel 已经有 omp 所需的 Kitty 图片主链路。当前缺口是 omp 默认不认识 `TERM_PROGRAM=lyrel`，因此不会自动发出 Kitty／Unicode placeholder；不是 lyrel 缺少 PNG 解码、placement 或 GPU 图片绘制。**

## 2. kero 调研核验

### 2.1 直接成立的结论

| 结论 | 一手证据 |
|---|---|
| kerod 持有权威 emulator，PTY 输出直接 `Feed`，客户端显示 Mirror 帧 | `/tmp/kero/internal/daemon/session.go:120-135,200-225,772-790`；`/tmp/kero/app.go:644-681` |
| kero 用 4ms 合帧，同步输出未结束时 50ms 重试 | `/tmp/kero/internal/daemon/session.go:648-680` |
| kero 自称 Ghostty，同时保留 `LC_TERMINAL=kero` | `/tmp/kero/internal/daemon/daemon.go:505-542` |
| mygo 经 libghostty-vt 读取 image generation、placement，并注册 Go PNG decoder | `/tmp/mygo/plugins/terminal/internal/vt/images.go:50-227` |
| mygo 按 z 分层、按 generation 更新 bitmap，支持 U+10EEEE 虚拟放置 | `/tmp/mygo/plugins/terminal/images.go:11-155`；`placeholders.go:9-176` |
| Mirror 将像素重新编码为 `f=32,o=z`、base64 4096 字符分块；placement 单独发送 | `/tmp/mygo/plugins/terminal/mirror_images.go:32-158` |
| omp 的 SVG 流式修订为 200ms 单飞，修补部分 XML，最终立即重栅格化 | `/tmp/omp-src/packages/tui/src/chat/svg-figure.ts:24-30,308-388`；`svg-source.ts:31-134` |
| 每次修订使用新 key／image id；旧 id 只标记 release，在后续帧统一 purge | `svg-figure.ts:375-388`；`components/image.ts:286-305,480-525` |
| omp 默认 TUI 帧率上限 30fps，Kitty 分块为 4096 字符 | `frame-rate.ts:1-15`；`terminal-capabilities.ts:899-930` |
| kero 默认图像缓存 320 MiB | `/tmp/mygo/plugins/terminal/terminal.go:646-649` |

因此，“逐笔出现”的视觉效果来自 omp 连续替换完整 PNG 快照，不是终端显示半个 PNG 文件。`SvgFigure` 的 `MAX_EDGE_PX=4096`、`MAX_VIEWPORT_SHARE=0.8` 也解释了截图的大图布局，而非 kero 自有聊天组件。

### 2.2 需要修正或限定的表述

- `1524×838×4 = 5,108,448 bytes`，约 **4.87 MiB**（或 5.11 MB），原文“约 5.1 MiB”单位不准确；不影响架构结论。
- mygo 的 PNG 上限为单边 10000、RGBA 400 MiB；原文写“400MB”，宜明确为 MiB。
- “只有回到底部才释放”若用于 diri 全部生命周期过于绝对：普通阅读是在回 live 时释放，find／宽度变化等还有自己的清理或重定位路径。
- 原文证据边界是诚实的：没有真实测量远程带宽、延迟和端到端流式帧率，不能把这些数值当成运行时验收结果。

## 3. lyrel 相对 kero／omp 到底缺什么

| 链路层 | kero＋omp | lyrel 当前实现 | 真实缺口 |
|---|---|---|---|
| fenced SVG 识别、半截 XML 修补、200ms 重栅格化 | omp `SvgFigure` | lyrel 不解析普通文本中的 SVG，且正式文档明确不主动加载 SVG 字符串／路径 | **不缺终端能力；应复用 omp，不能在 VT 层重复实现** |
| omp 终端能力发现 | kero 注入 `TERM_PROGRAM=ghostty`，omp 选择 Kitty＋placeholder | lyrel 注入真实身份 `TERM_PROGRAM=lyrel`；omp 的 `TerminalId`、`KNOWN_TERMINALS`、`fromProgram` 和 placeholder allowlist 均无 `lyrel` | **首要缺口：omp 静态注册与 placeholder allowlist** |
| 自动启用版本边界 | kero 声称其内嵌 libghostty 版本 | lyrel 在 `0.2.1` 首次同时接入资源／placement／Unicode placeholder | 注册时按 `TERM_PROGRAM_VERSION >= 0.2.1` 启用，避免新版 omp 错开旧 lyrel |
| APC 分块、base64、RGB／RGBA、PNG、zlib、`a=q` 应答 | libghostty-vt＋Go PNG decoder | daemon `GraphicsSniffer` 外提协议，`png`／`miniz_oxide` 校验，支持查询与 quiet 应答 | 已具备 |
| `a=t`／`a=T`／`a=p`、image／placement id、裁剪、偏移、z 层 | libghostty-vt＋mygo | `GraphicsState`、`Placement`、`TerminalElement`／`ImageStore` | 已具备 |
| `U=1`＋U+10EEEE、diacritics、网格切片 | libghostty-vt＋mygo | daemon 与桌面共享 `placeholder_row`／`placeholder_prototype`，Rio 提供网格语义 | 已具备 |
| 存活会话的图片恢复 | Mirror 重新编码 RGBA | daemon 持有资源／placement，带外二进制帧传输，像素按资源版本发送，base64 不进入 replay ring | 已具备，且不需要照搬 Mirror 重编码 |
| 实际 omp 图片／SVG | kero 自动发协议 | 已有 released omp 生产消费者重放，包含 assistant、toolResult、原生 SVG | 协议可用；尚需 **live streaming** 专项验收 |
| Sixel／iTerm2 | kero 这条绘制链路只有 Kitty | lyrel 同样以 Kitty 为图片路径 | 相对 kero 不缺 |

关键源码：

- lyrel 协议外提与格式：`crates/lyrel-core/src/core/kitty_graphics.rs:1-70,604-705,1292-1430`。
- daemon 权威资源／placement：`crates/lyrel-core/src/core/graphics_state.rs:1-110,343-360`。
- Unicode placeholder：`crates/lyrel-core/src/core/graphics_placeholder.rs:1-106`。
- 桌面图片提交与资源 lease：`src/terminal/images.rs:1-55,1064-1160`。
- 文字与图片同一 snapshot：`src/terminal/element/snapshot.rs:225-375`。
- lyrel 的真实身份：`crates/lyrel-core/src/daemon/pane/launch.rs:390,467-500`。
- omp 无 `lyrel` 注册：`/tmp/omp-src/packages/tui/src/terminal-capabilities.ts:52-77,590-705`；`/tmp/omp-src/packages/tui/src/kitty-graphics.ts:76-94`。

不建议把 lyrel 改成 `TERM_PROGRAM=ghostty`：lyrel 不是 libghostty 终端，这会额外触发 omp 的 Ghostty 首帧延迟等特判。短期显式配置可以继续使用：

```text
PI_FORCE_IMAGE_PROTOCOL=kitty PI_KITTY_PLACEHOLDERS=1 omp
```

长期零配置方案应是 omp 增加 `lyrel` 能力注册和版本门控，保留 `PI_FORCE_IMAGE_PROTOCOL=off`、`PI_NO_KITTY_PLACEHOLDERS=1`、`terminal.showImages: false` 等显式关闭项。

资源上限是差异而不是“协议缺失”：lyrel 当前单图最大边长 8192、编码／解码各 32 MiB；kero／mygo 对应 10000、400 MiB。omp SVG 自身最长边 4096，因此该差异不阻塞本次目标。

## 4. diri 调研核验

### 4.1 稳定阅读机制

直接核对成立：

- `ScrollbackViewport` 保存 `anchor`、`live_start_row`、`held_live`、`held_live_start`：`diri-term/src/scrollback.rs:149-196`。
- 离开 live 即 clone 整屏：`scrollback.rs:774-782`；入口 `element.rs:173-186`。
- `row_source` 在阅读态优先查 `held_live`，再查 live／history cache：`scrollback.rs:710-722`。
- 读态历史行 `or_insert`，不覆盖已取回内容：`scrollback.rs:1198-1205`。
- live 起点前进时以 `view_offset = live_start_row - anchor` 派生距离：`scrollback.rs:1214-1221`。
- `PERF.md:2317-2334` 明确说明绝对锚点不能保护仍可变的 live grid，并记录 160×50 capture／release 微基准；这不是 GUI 延迟证明。

### 4.2 截图悬浮框

实现位于 `diri-app/src/terminal_pane.rs:5538-5575`：

- `view_offset > 0` 时显示，底部居中； exited 时抬高到底部 52，否则 18。
- 文案为 `terminal.return_to_live`；中文是 `{lines} 行 · 回到实时输出`，英文是 `{lines} lines · Return to live`。
- `lines` 直接来自 `resident.element.view_offset()`，会随后台 live 起点前进而增大。
- 点击调用 `scroll_to_live`，释放阅读内容；若 find 暂停则刷新。
- 它是自定义 clickable `div`，不是 lyrel 必须逐字复刻的控件 API。

### 4.3 原 lyrel 修复建议需要补齐的不变量

原调研的根因判断正确：lyrel 当前 `snapshot.rs` 以 `Line(row - display_offset)` 读取可变 live grid，`held` 只在同步输出／等待尺寸等路径复用旧帧，不能冻结普通阅读时的活屏重绘。Rio `0.5.29` 的追加补偿真实存在，但 `clear_history` 会把 `display_offset` 归零。

方案不能停在“复制一屏”：

1. **每次用户滚动都要更新绝对 `top_row`**，不是只在第一次离开 live 时记录。
2. **阅读态不能只依赖 Rio `display_offset`**；`CSI 3J` 会清零它。应以 `top_row` 为真相，以当前 `lines_evicted + history_size` 派生距 live 行数。
3. **长生命周期内容不能保存已解析主题色**。应保存字符、ANSI style、marks／hyperlink 等语义，主题变化时重新解析可见行。
4. **文字与图片必须同一绝对坐标系**。direct placement 的 `anchor_row` 和 placeholder 所在行都映射到 `top_row`；仅冻结文字会让图片继续变化或消失。
5. **图片 lease 按 retained rows 持有**，不能长期钉住 image store 中全部历史 direct placements。
6. **选择、复制、链接、搜索、光标／IME 必须读取同一阅读呈现**；后台 live 的搜索匹配不能直接盖到旧文字上。
7. **不需要 diri 的 daemon 行协议，也不需要 IPC 版本变更**。lyrel 的 Rio 镜像已有 `lines_evicted + history_size` 与图片 `anchor_row`，这是 GUI presentation 层改造。

## 5. 本次实际验证与边界

已执行：

```text
LYREL_TEST_OMP=/opt/homebrew/bin/omp \
  target/debug/deps/lyrel-3faf302f9a9eeb59 \
  --ignored --exact \
  terminal::view::gpui_tests::graphics_consumer_released_omp_native_resume_pictures \
  --nocapture --test-threads=1
```

结果：`1 passed`，耗时 4.53s；`omp/18.18.6`，`requests=[]`，消息 `5→5`，`modelUsage=0`，`zeroInference=true`。该生产消费者用例实际断言 assistant 图片、toolResult 图片和原生 SVG 栅格图三类身份。

首次通过 `cargo test` 启动同一用例时，隔离 daemon 在 30 秒内未就绪并返回 `NotFound`，未进入图片断言；随后单独启动同一 fixture daemon，3 秒内生成 `control.sock` 与 `daemon.sock`，再直接运行测试二进制通过。首次结果记为 harness 启动失败，不记为产品协议失败或通过。

未执行／不能据此宣称：

- 没有执行 omp fenced SVG 的实时 token 流与 200ms 修订的端到端测试；现有通过项是静态保存会话重放。
- 没有真实 GUI 截图或远程链路带宽测量。当前进程缺 macOS Accessibility 与 Screen Recording 权限，未改系统权限，未启动或终止用户的 diri／kero 进程。
- 没有运行 diri 基准或 lyrel GUI 真实 agent 阅读回归；稳定阅读根因来自源码、Rio 行为和 diri 一手记录。

## 6. 五种 coding agent 的原生图片能力补充矩阵

本节回答“能否把 kero 截图中的 omp 效果外推给 lyrel 支持的所有 agent”。核对对象为本机发布版、对应上游发布源码与官方配置文档；“没有 fenced SVG 流式”指未找到相应识别、部分 XML 修补和重栅格化路径，不指 agent 永远不能增加该功能。

| agent／核对版本 | 当前原生图片内容 | 协议与公开选择路径 | fenced SVG 流式 | 对 lyrel 的结论 |
|---|---|---|---|---|
| omp `18.18.6`；调研源码 `1379127c…` | assistant 原生图片、toolResult 图片、fenced SVG 栅格图 | `PI_FORCE_IMAGE_PROTOCOL=kitty`、`PI_KITTY_PLACEHOLDERS=1`；保留 `off/none` 与 `terminal.showImages` 关闭项 | **有**：`SvgFigure` 200ms 单飞、部分 XML 修补、最终立即栅格化 | 唯一可直接复现截图完整目标的现有路径；短期显式启用，长期上游注册 `lyrel` 与版本门控 |
| pi `1.5.0`；真实消费者已通过的是 `1.4.0` 静态重放 | 保存的 toolResult 图片；PNG 直接显示，其他支持格式由 pi 转码 | `PI_IMAGE_PROTOCOL=kitty`；`terminal.images`、`terminal.showImages`，具体设置优先于环境 | **没有**：`1.5.0` 发布物无 `SvgFigure`／`closePartialSvg`／`rasterizeSvg` | 可复用现有 Kitty direct placement；不能把 toolResult 支持写成 assistant SVG 支持 |
| OpenCode `2.2.7`；真实消费者已通过的是 `2.2.0` 静态重放 | 用户附件与 tool-result 的图片卡片；每组内联前三张，其余 `+N more` | `session.image_preview` 默认关闭，需用户显式开启；OpenTUI `protocol=auto`，另有 `OPENTUI_IMAGE_PROTOCOL=kitty` 与 `OPENTUI_GRAPHICS=false` | **没有**：`packages/tui` 无 SVG fence 栅格化路径 | 现有 OpenTUI 查询与 direct placement 已能在 lyrel 工作；不应注入 `true` 覆盖用户明确的 `false` |
| Kimi Code `2.5.2` | `ImageThumbnail` 只由 `UserMessageComponent` 使用，显示用户粘贴图片；未见 toolResult／assistant 图片消费者 | `@moonshot-ai/pi-tui` 实现 `PI_IMAGE_PROTOCOL=kitty|iterm2|none`；官方 env-vars 文档未列此项，不能当稳定公开承诺 | **没有**：发布 bundle 与当前官方源码均无 `SvgFigure` 等路径 | 环境变量最多选择已有缩略图的协议，不能创造 agent 输出图片能力；目标若包含 agent 图片／SVG，必须改 Kimi 上游 |
| Hermes 本机 `0.22.4`；最新 stable `0.23.1` | 本机版只见 pet 的 Kitty 路径；`0.23.1` 已有用户附件及白名单工具本地图片的 `image.render`／`InlineImage` | `0.23.1` 使用 `display.inline_images=auto|kitty|iterm|unicode|off`；auto 不认识 `lyrel` 时回退 half-blocks，显式 `kitty` 才选择 placeholder | **没有**：`0.23.1` 与当前 main 均无 fenced SVG 流式栅格化 | 需按版本验收；不能把本机旧版或 main 的能力混写成所有 Hermes 已支持，也不能把 pet 当作聊天图片 |

一手来源补充：

- pi：本机 `@earendil-works/pi-tui/1.5.0/dist/terminal-image.js` 的 `PI_IMAGE_PROTOCOL`、`a=T` direct Kitty 编码；[官方 terminal setup](https://github.com/earendil-works/pi/blob/v1.5.0/packages/coding-agent/docs/terminal-setup.md) 与 [environment variables](https://github.com/earendil-works/pi/blob/v1.5.0/packages/coding-agent/docs/environment-variables.md)。本机 `pi-coding-agent` bundle 的 `ToolExecutionComponent` 只从 tool result 收集图片。
- OpenCode：[`v2.2.7` SessionImages / ToolImages](https://github.com/anomalyco/opencode/blob/v2.2.7/packages/tui/src/routes/session/index.tsx)、[`DialogImagePreview`](https://github.com/anomalyco/opencode/blob/v2.2.7/packages/tui/src/component/dialog-image-preview.tsx)、[公开 CLI 设置](https://opencode.ai/docs/cli/config)；[`OpenTUI 0.8.1` 图片协议](https://github.com/anomalyco/opentui/blob/v0.8.1/packages/web/src/content/docs/components/image.mdx) 与 [环境变量](https://github.com/anomalyco/opentui/blob/v0.8.1/packages/web/src/content/docs/reference/env-vars.mdx)。
- Kimi Code：本机 `2.5.2` bundle 唯一 `new Image(...)` 位于 `ImageThumbnail`；[官方当前 `image-thumbnail.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/apps/kimi-code/src/tui/components/media/image-thumbnail.ts) 明确是 pasted image，[官方 `user-message.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/apps/kimi-code/src/tui/components/messages/user-message.ts) 是其消费者；`2.5.2`  changelog 提交 `e2770e37bd19` 的对应文件与本机实现一致。官方 [interaction](https://moonshotai.github.io/kimi-code/en/guides/interaction) 只承诺粘贴媒体，[env-vars](https://moonshotai.github.io/kimi-code/en/configuration/env-vars) 未列 `PI_IMAGE_PROTOCOL`。
- Hermes：本机 `0.22.4` 的 `agent/pet/render.py` 与 `ui-tui` `usePet`／`PetKitty`；[`v0.23.1` `agent/terminal_image.py`](https://github.com/NousResearch/hermes-agent/blob/v0.23.1/agent/terminal_image.py) 的 `render_inline_image`、`tool_image_path` 与 `display.inline_images` 模式，[`InlineImage`](https://github.com/NousResearch/hermes-agent/blob/v0.23.1/ui-tui/src/components/inlineImage.tsx) 的 `image.render` 与 placeholder／half-block 呈现，以及 [v0.23.1 release](https://github.com/NousResearch/hermes-agent/releases/tag/v0.23.1)。

### 环境变量能解决什么

只能解决“已有 renderer 选择哪种终端协议”，不能解决“agent 是否把某类内容交给 renderer”。因此：

1. **协议兼容**由 lyrel 的 Kitty 实现和各 agent 的能力发现／显式覆盖负责；继续保持 `TERM_PROGRAM=lyrel`，不冒充 Ghostty。
2. **内容语义**必须由 agent 自己提供：fenced SVG、流式修订、toolResult、附件身份、重挂载与删除都由其 TUI 决定。lyrel 不解析 PTY 文本中的 Markdown／SVG，也不按 agent 名伪造图片。
3. **短期**可使用各 agent 的公开配置做显式 opt-in；**长期零配置**应向上游提交 `lyrel` 能力注册或检测修复，并按最低支持版本门控。Kimi 的未文档化环境覆盖只能作为版本锁定的过渡路径，不能替代上游公开契约。
4. 若产品要求五种 agent 都具有 omp 的 fenced SVG 流式效果，工作范围应明确包含 pi、OpenCode、Kimi Code、Hermes 四个上游项目的设计、PR／fork、版本与回归验收；这不是 lyrel 单仓库终端改造。
