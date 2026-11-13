<!-- 本文件源自 l0ng-ai/tty7，lyrel 已修改（含本声明）；原有版权与许可保留。 -->
# 更新日志

本文件仅记录 lyrel 的当前变更，不继承衍生来源的发行版本。lyrel 从 `0.1.0` 独立编号。

格式参考 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，版本遵循 [Semantic Versioning](https://semver.org/spec/v2.0.0.html)。

## [0.3.4] - 2026-11-13

### 依赖升级

- #259：`rio-vt`／`rio-graphics`／`rio-unicode` 原子升级到正式 registry 的精确 `=0.5.30`，同步锁定传递 `rio-grapheme-width 0.5.30`；适配 `glyph_register` 新增的 §8.5 `GlyphLayout` 参数，注册解析默认沿用协议默认布局，本票不启用 Glyph Protocol 能力。保留关闭 VT 默认 features、原 PTY／daemon 和无 GPUI 的共享核心；完整构建、回归、原生与远端验收结果按候选实际执行记录，未执行项不记为通过。

### 新增

- #268：daemon 在 8 MiB 文字回放环之外保留 pane 会话内已接受的 Glyph glossary，并在 ring／graphics／颜色回放之后，向显式发送 `SubscribeGlyphState` 的消费者发送有序有界 `GlyphState` 全量投影（generation + 插入序 `glyf` 注册）。旧 client 或未 opt-in 消费者保持连接，且不会收到 `GlyphQuery`／`GlyphState` 未知 kind；controller 未 opt-in 时 `q` 保持无应答。`c`／RIS 在流位置更新 generation，replacement 保留 FIFO 顺序；消费者原子替换 registry 并忽略更旧 generation，旧 reader 的迟到结果由 link generation 退休。Unix 同会话 handoff 显式携带该 frame，disk scrollback 剥离完整 glyph APC，daemon 死亡后的新 shell 以空 glossary 开始；不增加磁盘 glyph cache、第二 parser 或自动重启。聚合 wire／decoded 预算不足时明确拒绝而非静默丢失，保留 Colors59/61、Progress71、Sixel 协商与 `s` 的后续 gate。
- #267：GUI Glyph 消费路径实现 pinned v1.12 §8.5 布局：pad → size → align，声明 extent（`aw`／`lh`／`ymin`）或未声明时的解码点 bbox 与 `contain` em 上限、五种 size、水平／垂直 align（含 baseline 与 descender）、分数 padding 与退化归零、未扣 padding 的 render span 裁剪；`width=2` 仅扩像素 span，末列、非空或已有邻格自行绘制时回退一列，保持 logical width、光标推进、换行、选择与复制。前景、单元格几何、主字体 baseline、span、注册 revision 与 display scale 每帧参与派生，字体／显示缩放不要求应用重注册。保留普通字体、Powerline／boxdraw、Kitty 与 native 路径；本切片仍不回复 `s`。真实 PTY→daemon→production consumer 与确定性边界结果见本票交付证据；Windows 原生像素／显示缩放、WSL、真实 SSH 与 macOS 未在本票宣称通过。
- #275：静态 Sixel 在 #274 的 daemon authority 上补齐发现、恢复与拒绝语义。pane 协议新增可忽略的 `static-sixel` feature：附着客户端只在本次连接实际协商到它时才在 DA1 附加属性 `4`，旧 daemon／未更新远端 server 不会继承本机 GUI 的声明；`XTSMGRAPHICS` 由持有像素几何与准入上限的 daemon 在附着与脱离两种状态回答（`Pi=1` 当前／最大寄存器 `1024`，`Pi=2 Pa=1` 当前文本区域像素并按每边 4,096 截断，`Pa=2/3/4` 以状态 3 拒绝因为准入同时受单图 32 MiB RGBA 面积预算约束，ReGIS 与未知 item 以状态 1 拒绝），镜像保持沉默；`XTGETTCAP` 白名单不变，`sixel` 仍不是 terminfo 能力名。Sixel DCS 在进入文字环之前提取，发送方停止、8 MiB replay 被淘汰后，全新 consumer 挂回同一存活 pane 仍从有序 Graphics 帧恢复像素与 placement 且发送方计数器不变；超限、非法颜色分量与被取消的截断体不产生 placement、不移动光标、不替换已接受资源；resize 后图片继续与实际文字行共享几何。真实 img2sixel 1.10.5 输出与生产消费者证据按交付记录，Windows 原生像素、WSL headless、真实 SSH 与 macOS 不由 headless 断言替代。
- #269：Glyph 注册资源身份随呈现快照捕获——`RenderCell` 与 `ReadingCell` 保存 `Arc<GlyphView>`；未完成的同步呈现帧、grid 竞争复用帧和阅读行保留其显示时的版本，live 单元格在下一次成功帧采用当前注册。replace／clear 不再改写正在阅读的画面，返回 live 后采用新版本或普通字体 fallback；RIS 仍按既有破坏性阅读重置丢弃阅读呈现并清空 live registry。不新增 registry／cache 或第二绘制管线，普通 pane 不付逐单元格查找，outline 由最后一个 live／捕获 owner 释放。验证以真实 PTY→daemon→生产消费者为主，原生与前台边界另行记录。

- #274：静态 Sixel 使用正式 Rio parser，经原 daemon／PTY 有序准备、资源预算与 placement 生命周期进入终端；自然尺寸、默认光标／滚动、DECSDM、右侧光标模式及拒收后的文字顺序不套用 Kitty 语义，图片位于文字之下，镜像不再绕过 daemon 解码。保留 Kitty、同步呈现和阅读态；与 #265 的 DCS discovery framer 按各自模式共存，不放宽发现请求预算。Windows 独立 target 的 Sixel／真实 PTY 7 项、Kitty 15 项、XTGETTCAP 4 项、同步输出 1 项、生产消费者 2 项通过（源码 `4d788ed5`），实际官方 img2sixel 原生入口已准备；未宣称 Windows 原生像素、WSL headless、SSH 或 macOS 通过。不扩展文件／URL／动画，不提前宣称 #275 的完整恢复／安全／发现兼容。
- #271：单终端标签页显示 `OSC 9;4` 的运行中、未定量、暂停及错误进度，仅显示发送方提供的百分比；daemon 在回放环之外保存有界当前状态，支持存活 pane 重挂载及 Unix 同会话 handoff，不从旧文字给新 Shell 复活进度。命令结束清理运行／暂停、错误保留到 clear 或下一命令；独立语义事件不重复驱动 agent 生命周期，隐藏输出保持绘制门控。实际 pane endpoint 升至 v18、control 保持 v16，不兼容时明确拒绝并要求更新，不自动重启服务。本票增加真实 PTY→daemon→生产 tab 消费者及原生场景入口；各平台实际验证以交付证据为准，不宣称未运行的原生／SSH 验收通过。
- #272：标签页进度汇总其全部终端叶子，包括未聚焦、隐藏和停放的窗格；错误优先，仅单个有效任务提供百分比时显示数值，多任务只显示诚实状态而不做平均。保留智能体等待／完成／未读语义，不增加重复 busy 或完成通知；关闭、移动、停放／恢复、焦点、重连及休眠／唤醒重建按当前成员更新，瞬时进度不写入 durable 布局或 agent payload。新增真实多 PTY→daemon→生产 tab 消费者、确定性成员边界测试和原生多窗格场景；Windows 实窗、WSL 与真实 SSH 仍按父验收边界分别记录，不宣称 macOS 通过。
- #262：终端内容区支持 OSC 22 基本指针 set/reset（含空名称释放），按实际 GPUI／平台形状映射并安全回退；保留工作台菜单／分屏拖动、本地文字选择和链接操作优先级，命令结束、提示符、reset、断连与链接退休清理旧覆盖。不增加指针堆栈／查询或终端身份伪装；新增真实 PTY／daemon／生产交互消费者、确定性 reader 边界回归和可复现原生场景。测试平台的绘制策略与操作断言不替代原生指针、macOS 或真实 SSH 验收。
- #261：终端新增 pane 会话内 `OSC 4/10/11/12` 索引色与默认前景／背景／光标 set/query/reset 覆盖层，绘制、查询与 prompt 背景共用当前主题 + 覆盖后的有效调色板；命令退出保留、主题切换只更新未覆盖默认、复位露出当前主题，其他 pane／界面／配置不受影响。daemon 在回放环之外保留固定 259 槽的有界解码状态，经有序流投影给已协商消费者；ring 裁剪、窗口重附着与同会话 handoff 可恢复，daemon 死亡后的新 shell 从空状态开始。新增 `pane-colors` feature 与客户端 opt-in，新 daemon 绝不向旧客户端发送新帧；缺少该能力的后端在图形窗格附着前收到明确的更新错误，不静默丢弃颜色、不停止旧 daemon、不自动重启。同步输出与阅读态沿用现有 retained-frame 规则；新增真实 PTY／daemon／生产终端消费者、handoff 与恢复路径回归及原生颜色场景入口。Windows 原生、WSL 与真实 SSH 组合验收由集成阶段补齐，不以 headless 结果替代。

### 修复

- #268：`GlyphState` 的共享解码器拒绝 `upm=0`、`aw=0` 与 `lh=0`，与注册请求的值域一致，避免恢复投影向消费者引入从未能被接受的布局参数。
- #275：Sixel 原生恢复脚本改用 9 MiB 无滚动的 SGR reset 输出淘汰原传输字节，避免可见长行折行先把图片滚出保留历史，混淆正常资源退役与恢复失败；图片生产进程仍在淘汰与重新附着前退出。

- #263：协商的 `?1005` UTF-8 鼠标坐标编码实际生效——此前终端已应答 1005 已设置，编码器却只查 SGR 并在列／行 ≥223 处静默丢弃报告。现在按生效编码写出（X10 上限保持 223，UTF-8 上限 2015，SGR 不变），按下／松开、修饰键与滚轮在宽窗格送达应用；`?1005`／`?1006` 切换与 DECRQM 查询按模拟器生效状态回答，1015／1016 仍如实回答不支持；daemon 模式折叠按模拟器规则将鼠标上报级别与 1005／1006 编码作为互斥组折叠，重连回放只恢复最终生效状态。Shift 本地选择覆盖与未协商 legacy 行为不变。真实 PTY→daemon→生产消费者断言、原生宽窗格／SSH 场景与未验证项见工单交付记录。
- #264：将 Kitty 事件类型协商接入真实 GPUI press／repeat／release 路由，CSI-u、方向键和功能键使用真实 held／keyup 信息；只为实际写入 PTY 的按下配对释放，并在焦点、窗口失活、切屏、模式／命令边界、退出和重连时退役旧配对。有效模式栈、查询与 replay 统一过滤框架缺失的 alternate-key 位，保留 legacy 输入及 ConPTY 已合成的 Enter／Ctrl+J 事件对；IME 预编辑不取得配对，支持关联文本的纯文本提交使用 key code `0`。补充生产输入、真实 PTY／daemon 和精确字节消费者；Windows 实窗、系统中文 IME、真实 SSH 与 macOS 原生验收分别记录，不能由 headless 事件测试代替。
- #265：复用正式 Rio 的 XTGETTCAP 解码与 response callback，按解码名字仅广告已支持的 256 色／真彩色；未知、未验证、剪贴板、Glyph／Sixel／iTerm2、Rio 产品身份和固定几何均负答。daemon 在附着／脱离／重连中保持单一实时应答者，桌面和历史 replay 不重答；实际 pane endpoint 的 `xtgettcap-owner` feature 缺失时明确要求更新，不自动重启服务，不改变产品级 `TERM`／`TERM_PROGRAM`。原生及真实客户端验收按实际执行记录，不将 headless 协议通过等同于跨平台完整验收。

- #260：终端 `OSC 4/10/11/12` 查询与生产绘制统一使用当前主题的有效 ANSI、默认前景／背景和光标颜色，保留固定扩展色位、亮色可读性、同步输出提交与历史重放副作用抑制；不启用程序驱动动态改色。Windows 独立 target 的真实 PTY → daemon → 生产终端颜色消费者完成有效 RED／GREEN，既有查询重放消费者与四项调色板边界测试通过，均无忽略项；新增 `scripts/terminal-color-query-scene.mjs` 对照入口。原生像素、WSL／真实 SSH 与 macOS 未在本票分支验收，不以返回值或 headless 绘制断言替代。
- 修复 Markdown 图片后台加载错误的本地化：worker 保留错误类别，回到 UI 线程后才翻译，避免中文界面混入英文 SVG／容量／重定向提示；资源消费者显式覆盖中文界面，保留外部引用拒绝和禁止重定向断言。
- 修复智能体输出时上滚再下滑出现已消失输入栏的阅读态残影；Markdown 所见即所得编辑器支持 Ctrl/Cmd+F 查找与替换，宽表按页面宽度折行且不再溢出，Markdown 与代码编辑器提供常显水平滚动条；代码编辑器状态栏移入文档面板底部、与全局状态栏并存；原生 SSH 窗格通过远端进程表显示当前 shell 进程树；应用 Toast 与连接提示统一到右下角、全局状态栏上方。

## [0.3.3] - 2026-11-12

### 修复

- 修复非最大化窗口的 Markdown 铺满／停靠切换与终端指针命中；移除重复表格工具，保持自动换行代码块背景连续，渲染表格单元格中的行内语法，并为文档提供可拖动滚动条。Markdown 状态栏回到编辑器底部；原生 HTML 与图片直接呈现，智能体侧栏改用高清品牌资源。空会话不再被陈旧 shell 可执行路径覆盖智能体名称，恢复会话会读取原生历史标题；omp 通过命令级只读配置叠加将内联图片上限从 8 调整为 20，不改写用户全局配置。
- 修复 PowerShell 在 Unix 上使用 Windows `;` 拼接 `PI_CONFIG_FILES`，改用 `[IO.Path]::PathSeparator`；Windows PowerShell 5.1、pwsh 7 与 Unix 主机共用同一脚本。
- 稳定 macOS 跨 daemon 图形预算消费者的视口上报时序，避免历史图片在新 offer 到达前仍被旧视口保护而触发预算拒绝。

## [0.3.2] - 2026-11-11

### 新增

- #250：为 zsh、Bash、fish、PowerShell 的直接 agent 启动增加命令级 Kitty 环境与下一提示符恢复，本地 Nushell 使用轻量 wrappers；快捷启动、明确映射、待连接窗格、恢复／分叉保留 agent 意图和公开终端 scope，Hermes 专属 marker 尊重显式空值。保留协议禁用和用户 hooks，不引入 PATH shim；补充真实 shell／原生子进程／pending consumers 与无集成时的显式回退。实施阶段未运行构建、格式化、测试、原生窗口或远端验收。
- #249：新增不强制图片协议的 pi 1.4.0 scoped-discovery 消费者，保留显式关闭／设置优先级和零新消息重放；OpenCode 2.2.0 预览 on／off／inline false 检查完整 global／inline 配置不变。补充 pinned Kimi 用户粘贴缩略图和 Hermes 0.23.1 `image.render`／`InlineImage` 原生组件到真实 PTY／生产终端的消费者，比较完整图片、放置、卸载／重挂载与替换；不增加 renderer、不改用户配置。同步各 agent 能力矩阵与 OpenCode opt-in 文档；本分票未执行构建、测试、原生窗口或远端验收。
- #236／#245：Markdown 完成唯一默认 Zorite-derived 源码路线的切换，移除旧 Markdown/MDX-rich 投影与专属实现契约；caret-local 标记显露、普通单击链接、MDX source-only 和 shared Document/history 成为当前产品规则。primary/secondary 安装同一组真实 Host providers，rename/save-as 只更新身份与源码策略，不丢弃历史。正式文档、原 14 项 native 消费者逐项迁移及 pinned MIT 来源/字体通知统一收敛；Windows ZIP/Inno 与 macOS app/ZIP 增加完整根 LICENSE 字节守卫。完整宽表的远端末行、真实图片/公式联合 hard-break 与长代码诊断尾部消费者补齐；源码和测试交付不是最终 native、实窗、SSH、平台、包或用户接受通过。

- #236／#239：为 Zorite-derived Markdown 的每个 presentation 接入原生表格尺寸/样式选择、可键盘操作的行列/对齐/resize/fit 命令与 Kit 语法高亮/语言/复制控件。表格和代码均修改同一共享文档/history；代码语言修改保留 fence metadata，表格 Unicode 命中与 IME 使用绘制一致的 cell 几何。补充完整宽表与 owning Host 保存重开消费者；集中回归和实际参考窗口对照尚待执行，不以源码接线声明验收通过。
- #175／#176：新增智能体关注导航，优先定位等待最久的窗格，再浏览未读完成回合，支持跨标签页／窗口、随处搜索与自定义快捷键（Windows Ctrl+Shift+U／macOS Cmd+Shift+U）。daemon 记录连续状态时长，重复同状态 hook 不重置计时；托盘、工作区切换器与 CLI 使用同一语义，`lyrel agents` 新增 FOR 列，旧 daemon 的未知时长显示 `-`。
- #175／#177：GitHub 面板将同一仓库的堆叠拉取请求分组并绘制连接线，刷新和重排后按条目身份保持键盘选择；不将无关 fork 或身份不明的分支合并，保留既有链接。
- #175／#178：未连接窗口的终端按既有外观提示回答主题查询，无有效提示时沿用浅色默认；附着客户端继续负责自己的查询，重放与显式 handoff 不重复回答历史查询。
- #175／#179：macOS 新增 Cmd+Shift+W“关闭窗口”快捷键，复用既有未保存文档确认与会话关闭路径；Cmd+W 和 Windows 默认键位不变。
- #236／#241：导入固定 MIT `ratex-gpui 0.4.1` 结构公式内核并适配 Kit/Fast；原生行内/块级渲染、二维分式/根式/定界符、补全、上下文菜单与原位提交/取消接入共享 Document。提交只修改公式正文及显式对齐标记，取消与未修改提交保留原始定界符、混合行尾和 attached spelling；过期 seat/render 与有损结构解析拒绝应用并显示原因。新增生产工作台与完整 400034 B 高原子/fragment 原生消费者；构建、运行和固定参考实窗比较由集成验收执行，未宣称通过。
- #236／#244：为 Zorite-derived 原生 Markdown 配置随缩放变化的正文留白、列表／引用节奏与提示块图标，主题切换不重置模式和历史；保留紧凑标题／提示块 disclosure。新增行首 slash／键盘插入菜单与真实颜色草稿、取消、应用和清除入口，引用续行／退出、格式和双视图修改均使用同一文档历史。代码交付不代表原生同源文对照、完整长行／affinity 或系统 IME 已复验。
- #236／#242：Zorite-derived Markdown 的所有 presentation 接入真正 native Mermaid Host，复用正式 Merman `0.9.2`、安全 SVG 管线与 Kit/Fast renderer；当前 source 编辑、Undo/Redo 与 live/source 共用 Document，提供 loading、可见诊断、错误恢复、主题字号和异步几何/scroll 锚定。派生结果按文档、块 owner、request、revision 与 source/style lease 隔离；移除或修改块不接受旧结果。新增生产工作台 native 消费者及精确 Host 保存重开覆盖；集中检查与固定参考实窗对照由集成验收执行，未在本分票中宣称通过。
- #247／#251：终端新增稳定阅读态：离开实时输出后以绝对行身份保留一屏语义文字，按首次读取内容冻结，后台输出、光标定位、擦除与重画不改写当前阅读；底部提供 `↓ N 行 · 回到实时输出` 按钮，支持点击、Tab／Enter、直接输入或粘贴返回。文字、直接 Kitty placements 与 Unicode placeholders 共用 `ReadingView`，选择／复制、链接、搜索、光标／IME 读取同一语义来源；resize reflow 保持逻辑阅读位置，清屏／reset／重连／隐藏窗格按独立失效世代释放阅读行与图片 lease。新增真实 PTY／daemon／生产绘制与 GPUI 交互消费者；#255 已在隔离 Windows 生产窗口和 headless consumer 矩阵运行，macOS／Linux 原生窗口与真实远端 SSH 仍未验收。
- #248：为快捷启动的 omp 增加命令级 Kitty 图形兼容 scope 与 streaming SVG 原生证据；显式终端身份优先，不继承宿主版本，不修改用户全局 agent 配置或普通 shell 身份。保留协议禁用、显式关闭与零新消息重放；父 shell 身份改从 daemon 只读 `Observe` 回放独立核验，不把可被 TUI 覆盖的 VT 呈现当作追加式 PTY transcript。

### 修复

- 修复非最大化窗口中 Markdown 铺满／停靠切换和终端指针命中；移除重复表格工具，保持自动换行代码块背景连续，渲染表格单元格中的行内语法，并为文档提供可拖动滚动条。Markdown 状态栏回到编辑器底部；原生 HTML 与图片直接呈现，智能体侧栏改用高清品牌资源。空会话不再被陈旧 shell 可执行路径覆盖智能体名称，恢复会话会读取原生历史标题；omp 通过命令级只读配置叠加将内联图片上限从 8 调整为 20，不改写用户全局配置。

- #252：真实字形／主题／光标消费者从零光栅边界的 `NoopTextSystem` 迁入 `native_terminal` 目标，复用已有 OS 主线程、真实字体 runner 与同一 PTY／daemon 支持；复制、宽字／组合字符、链接、Tab stops、冻结文字及非空 glyph／动态颜色断言全部保留。Windows raw PTY 的长链接夹具先显式开启 DECAWM，并独立核对全部三行后再冻结、滚动与验证 hover／完整目标，不改生产阅读／绘制逻辑或 #249 matrix。未运行构建、测试、lint、格式化；native-font consumer 不替代 #255 实窗／系统 IME 验收。
- #252：生产绘制消费者改从 GPUI 已完成帧的 `terminal-surface` 几何观察固定 pane 边界，不再读取私有 `GridSnapshot::scroll_shift`；保留分数滚动上一行图片的可见薄片与全部真实 paint 断言，不修改字段可见性或生产行为。本修复未运行构建、测试、lint 或格式化，待集成验收。
- #250：修复 Windows PowerShell 5.1 在 hook 之后执行 `Env:` 显式空赋值、再次删除空值的问题：仅对已识别 agent 命令的七个 scope 字段，在旧 Windows PowerShell／pwsh 中用系统 setter 执行空字面量赋值；保持原始历史／OSC、显式 `TERM_PROGRAM`、opt-outs 和提示符恢复。真实 PTY 用例新增显式空／unset 恢复断言，并用生产 scope encoder 配合 `Start-Job`／`Wait-Job` 覆盖 PS5.1，保留 pwsh 7 末尾 `&` 背景启动及恢复断言；旧 `AmpersandNotAllowed` 不算有效启动。本次未执行测试、构建、lint 或格式化，PS5.1／pwsh 7 均待集成验收。
- #250：Bash 提交命令的 history 前缀正则改为局部变量中的 ERE，并在 `=~` 右侧使用未加引号的变量展开，避免 Bash 3.2 在启动阶段把内联正则中的括号误解析为 shell 语法。正则、完整多行命令重建、转义／截断、用户 history／hooks 及命令级 agent scope 均不变，不增加 `eval` 或命令执行；保留全部既有断言。本修复子任务未执行构建、测试、lint 或格式化，Bash 3.2／4.4／5.1／5.2 行为需由集成验收核验。
- #250：修复 Bash 3.2 的直接 agent 命令未收到 Kitty scope：`DEBUG` 临时关闭 history，导致 `[[ -o history ]]` 拒绝已记录的完整命令。仅旧版 `DEBUG` 路径改从 `SHELLOPTS` 读取配置状态，仍要求完整 history 与命令一致；保留 history 关闭、管道／复合命令／解释器排除、显式身份／协议禁用、用户 traps 与 `$_`，不改 Bash 4.4+ 路径。原始运行时诊断已捕获相同的命令文本及被临时关闭的 history；修复后构建、测试、lint、格式化与版本矩阵均未执行。

- #252：修正五项阅读消费者的前置条件与几何观察：文件链接使用真实子进程 cwd，长链接避开 PTY 完成标记行，历史／虚拟图片按 ID 显式删除，分数滚动绘制断言使用固定 pane mask；原子输出生命周期在 daemon／RemoteTerminal 公开边界观察 BSU、隐藏解析与 ESU，避免完整 PTY 往返跨过 fail-safe。保留冻结语义、目标／hover、资源释放和真实 paint 断言，不改生产阅读或图片行为；本修复子任务未运行构建、测试、lint 或格式化。
- #252：稳定阅读的选择／复制、链接、搜索和光标／IME 使用同一语义行；跨视口选择以绝对行保持身份，捕获 Tab stops，主题变化重新解析 ANSI 样式。直接 Kitty 切片与 placeholders 保留资源版本、crop、z 及文本对齐，修正 replay origin 转换；历史缓存仅保留阅读／选择需要的行，不累积全部访问历史。新增真实 PTY／daemon／生产 paint 和 GPUI 交互消费者，修正绘制观察为 committed images；未执行构建、测试、lint、格式化或实窗验收，#255 继续记录本地／SSH、原生像素与系统 IME 证据。
- #249：修复 OpenCode preparation 参数顺序；Kimi worker 显式选择 pinned source 根 `tsconfig.json` 以保留参数 decorators；Hermes 原生夹具改走生产 agent scope encoder 获取 presence-sensitive marker，不手动注入子进程环境。保留完整像素、placement、关闭策略、配置／源码完整性及零推理断言；此轮未执行构建、测试、lint 或格式化，不宣称原失败已运行通过。
- #250：PowerShell 手动启动识别器先解开环境赋值右侧的 `CommandExpressionAst`，再安全读取字面量，修复真实 pwsh 中带前置环境赋值的 agent 命令未收到 `TERM_PROGRAM=kitty`；仍拒绝赋值重定向、管道、复合命令和解释器调用，保留显式空值、scope 编码启动、用户提示符及下一提示符恢复。既有真实 PTY 消费者已覆盖失败路径，未降低断言；本修复子任务未运行构建、格式化或测试。
- #251：冻结 placeholder 消费者区分单元格行原点与方图 aspect-fit 后的居中像素 crop，按独立方图几何同时约束完整行与分数滚动上一行，保留蓝色资源身份、live 擦除／替换后的捕获图片及真实绘制断言，不修改生产图片布局。阅读态的上一行单元格不再遍历实时栅格；构建、测试与格式化由集成验收执行，本修复子任务未运行。
- #248：普通 shell 的 scoped-launch 前置身份标记改从 daemon 的只读 `Observe` 回放中独立检查，并核验前后标记顺序；终端 `scrollback_text()` 是可被原生 TUI 清屏／覆盖的 VT 呈现，不是追加式 PTY transcript。保留退出后生产视图中的父 shell 身份检查、显式策略优先级及全部 streaming 栅格／放置断言，不修改实际输出或生产阅读实现。本修复子任务未执行构建、测试或格式化。
- #248：确定性 omp token-stage 消费者仅在自有 streaming overlay 关闭额外的 `display.smoothStreaming` 文字揭示动画，并用正式 config CLI 核验。避免单个 SSE delta 被再次拆成根标签已完整、首个图元尚未完整的合法透明 SVG 后，误当作红底修订；真实 SSE、SVG 修补／节流、两个非空完整中间栅格、精确最终像素与顺序／无旧放置断言保持不变。未在修复子任务运行验证，不宣称默认 smooth reveal 子前缀验收通过。
- #251：终端阅读态在原生 `Div` 上注册子元素 prepaint 监听后再附加 identity／测试观察包装，修复 `Observed<E>` 不提供 `on_children_prepainted` 的编译错误；保留原有子元素边界测量、遮挡高度更新与返回实时输出按钮布局。编译与行为回归由集成验收执行。
- #251：滚动条把实时边缘的显隐几何与当前历史范围分开保存，避免首次拖动被过期历史容量裁剪而未建立冻结阅读态；保留后台输出不唤醒滚动条及粘贴回到实时输出的行为。阅读行同时保留共享 Kitty 解码器的语义 placeholder runs，完整行与分数滚动的上一行图片切片均来自冻结行，修复既有 omp 消费者在阅读时图片未进入生产绘制列表的问题；新增消费者回归未在修复子任务中执行。
- #236／#243：保存从同一共享文档快照捕获源文、编码/BOM、逐行表示与字节，并绑定原Document／Host／path。另存为写入成功后才采用目标，失败保留原身份和历史；在途新编辑使旧关闭意图退役，后续保存不重启旧关闭。显式去尾空白改为稀疏事务，保留混合行尾并可撤销；扩展名改为MDX时只改变各呈现策略，不替换文档。新增编码往返、失败恢复、第二呈现快照与native消费者，实际local／SSH及平台验证由集成者分别记录。
- #236：修复公开 `EditorTrimTrailingWhitespace` action 同步重入 `LyrelApp` 导致的崩溃；直接调用已有编辑命令，保留共享文档的稀疏修改与 Undo。
- #236／#241：结构公式焦点内的 Cmd/Ctrl+S 先确认有效草稿，再保存共享文档，避免工作台快捷键抢先保存旧正文。
- #236 双轴审查：图片/图表显式回收 GPU atlas，图片预览与 cache 共用释放 lease；资源扫描使用权威 Document snapshot。关闭非 composition owner 的视图不再终结存活输入的 IME；格式转换与后续 typing 分离为 Undo 事务，no-op Trim 不覆盖先前选区。正文菜单完整双语并随语言切换更新，MDX 改回 Markdown 恢复既有双视图链接能力；公式保留缩进/空 fence 边界，纯 caret/selection 移动不重新栅格化，选区绘制复用单帧行索引。新增真实工作台/native 消费者，实际验证由集成者集中记录。
- #236 最终实窗发现并修复折叠标题内的块公式／Mermaid 残留：折叠行的可见性先于资源 widget 布局，同时维护隐藏 fence 的状态。既有真实字体工作台消费者新增加载图形后折叠／展开的相邻标题几何、原文和 clean 状态验证，不缩减图形或改为源码绕过。
- #204 的旧投影、Rope 同步与专属工具栏已由 #236 默认实现替代；旧版本的输入、几何与真实字体结果仍是历史记录，不作为本次导入的通过证明。完整 Unicode/软换行/原子对象/长行/表格/保存/history 合同迁到新工作台消费者，明确退役的 rich-MDX 与 notebook 交互不以兼容内核保留。
- #175／#180：macOS 反向搜索中的 Cmd+Enter 只执行当前选中的历史命令，不再触发窗口全屏；空匹配不运行草稿，其他上下文与非 macOS 默认绑定保持不变。
- #175／#181：Windows 便携更新在解析 ZIP 目录前限制实际条目数与 ZIP64 资源字段，并限制单条目压缩／展开大小；保留最多 4096 项与累计 1 GiB 上限，以及 checksum、EOF CRC、路径和重复项检查，异常包在停止 daemon 或移动原安装前拒绝。
- #175／#182：修复 fish 登录 shell 下远端 server 安装与重启的 POSIX 脚本和嵌套路径引用，保留 shell 拒绝启动的实际状态及诊断，避免将 stdio／pane bridge 当作 daemon 停止。
- #175／#183：zsh metafied 非 ASCII 历史字节在 UTF-8 解码前还原，保留中文、日文、emoji、历史元数据与去重排序，以及 bash／fish 既有行为。
- #175／#184：配置 `ZDOTDIR` 时保留用户 startup 文件与 shell 集成；本地 bash／zsh 反向搜索使用当前窗格已有的自定义 `HISTFILE` seed，合并期间不丢失新输入，不添加跨 pane 历史来源或路径 sidecar。
- #175／#185：原生 SSH 窗格连接就绪后通过自身 SFTP 路由播种远端 shell 历史，保持 Host 与内跳隔离；自动重连不再夺回其他活动客户端持有的工作区，显式 Take Back 保持可用。
- #175／#187：终端鼠标拖动和松开仅由已报告按下的窗格继续上报，避免从侧栏、标题栏或相邻窗格开始的手势产生孤立报告；保留合法越界拖动、点击、选择与滚轮行为。
- #175／#188：在有效 shell 边界清理主屏与备用屏遗留的 Kitty 键盘状态，防止后续程序错误接收 Escape／Ctrl+C 编码；重新附着仍保留当前应用新协商的模式。
- #175／#189：主字体 PUA 图标可借用同背景空白邻格，fallback 图标在两格同为选中状态时也可借位；仍受背景、选择、搜索高亮与邻格内容约束，不改变终端列宽。
- #175／#190：窄窗口的 diff 标题和改动摘要在原区域收缩，不再挤出 Split／Unified、Dock／Fill 和关闭控件，保留 patch 与布局切换状态。
- #175／#191：SSH 主机与智能体长列表可滚动时独占滚轮，仅在事件开始时已到达对应边界后交给外层设置页面；短列表和空结果不陷住外页。
- #175／#193：GitHub 同域安全 API 重定向保留认证，支持仓库重命名或转移；跨域及 HTTPS 降级仍不转发令牌。
- #175／#198：终端 `path:start-end` 引用按窗格 Host 与工作目录解析，并在内置编辑器定位到范围第一行。
- #175 最终审查：空关注队列的信息提示进入应用通知历史，可回看完整内容并复制；终端分别保留左／中／右按钮已报告按下的所有权，不因另一个按钮松开而丢失配对报告；无 `/proc` 的远端 daemon 探测和停止接受任意位置的独立 `--daemon` 参数，继续排除 bridge 及参数／路径中的同名片段。
- #253：终端 resize／reflow 时按逻辑行身份重定位阅读位置：反复窄宽、Unicode／marks、相同 256 字符前缀的最近候选、空行上下文、淘汰后的估计与最老边界、等面积旧画面、等待中的滚动、请求取代及没有后续 PTY 输出的提交均保持当前阅读；不改变 live 输出、选择语义或历史淘汰。新增 `reading_resize_` 生产消费者，覆盖真实 PTY／daemon 与人为分开 `Size`／`Screen` 的有序 pane stream；后者仍使用真实 `RemoteTerminal` 和 rooted `TerminalView`，不是原生窗口或 SSH 证明。
- #254：阅读态生命周期失效与路由加固：`3J`／`RIS`／同批备用屏往返、Snapshot／relink、超过 10,000 行容量压力、过期图片后 resize／同 image ID 新版本、隐藏窗格输出与窗口关闭均按独立世代释放阅读行与图片 lease；滚轮路由始终读取实时终端模式，Shift 保留本地回滚逃生。新增六项受控生产消费者；#255 已运行，macOS／Linux 原生窗口未验收。
- #247 最终回归：多行命令启动作为一次 paste 提交；修复 full-suite 搜索、淘汰图片与 scrollback 回归；退休图形资源时释放未绘制的 GPU page lease；Pi／OMP bridge scope 契约固定到共享终端身份，不继承宿主版本。

### 依赖升级

- GPUI Kit 升级到 `0.9.1`，继续通过官方 `gpui-fast` feature 使用 Fast 后端；同步 `gpui-pre-reqwest-client =0.3.12`，适配原生 foreground ranges、软换行 caret affinity、颜色选择器焦点与 Markdown 消费者 API。新增 `lyrel-markdown-editor`（Zorite-derived GPUI Fast Markdown 编辑器）与 GUI-free `lyrel-text` 共享 codec 两个 workspace 成员；Windows／macOS 默认成员包含 editor，Linux headless 排除桌面包。
- #202：`resvg` 从 `0.48.1` 升级到 `0.49.0`，保留 `text`、`system-fonts`、`raster-images`、`svgz` 与关闭默认 features 的配置；同步锁定传递依赖。
- #203：适配正式 `sysinfo =0.40.1` 的可失败构造与主机名 API，保留不支持平台和底层错误，不返回伪造 CPU／RAM 样本。维持仅 `system` feature、原有有效值检查、每秒共享快照与空闲基线重置；refresh API 仍返回 `()`，不宣称能够检测依赖内部未公开的刷新失败。
- #201：`cargo-minor-patch` 组升级 8 项传递依赖，保留既有锁定与平台特性。

### 验证边界

- #255：在隔离 Windows 生产窗口与 native SSH → 容器 sshd → Linux `lyrel-server` 远程窗口完成桌面端到端验收，覆盖持续输出、静默 resize、反复窄宽、滚动、主题、pill、选择、搜索与全屏 reflow；macOS／Linux 桌面原生窗口、真实中文 IME、触控板惯性与远端带宽／帧率仍未验收。
- #247 原生图形矩阵：omp、pi、OpenCode、Kimi Code 与 Hermes 的图片消费者在真实 PTY／生产终端运行，保留完整像素、placement、卸载／重挂载与关闭策略断言；不替代 macOS／Linux 实窗与系统 IME 验收。

- #236 最终无探针 Windows Release 已完成普通／完整长文／48 列宽表／完整资源文档的输入、精确保存及 100 项 Undo／Redo；最终真实 SSH 验证 BOM／混合行尾保存重开、图片／链接／公式／Mermaid，以及物理键系统中文 IME 的提交／取消与单步 Undo。Windows 全 features workspace 含 35 项无筛选真实字体消费者通过；Linux headless 与两平台 hosted 构建／打包结果另记。负责人明确接受长文完整历史的整应用 private bytes `183.6 → 306.6 MiB` 成本，不宣称严格延迟不劣性；原失败、macOS 实窗／系统 IME 与实际安装未验边界见 `docs/window/side-panel.mdx`。
- #236 macOS 首轮 CI 的颜色选择器消费者为 `34 passed / 1 failed`：测试误用 Windows `Ctrl+A`，未替换十六进制输入。改用既有跨平台 `secondary-a`，保留精确颜色、取消、清除、双视图历史与保存断言；不修改产品快捷键或放宽预期。后续结果见 Markdown 编辑器的最终集成验收记录。
- #175 固定审查 tty7 的 34 个提交，终点为 `8ad8841e4835e4e6c4e6f4d5a0a16541eaa9107f`，采用当前 lyrel 架构的局部语义移植；不恢复 gateway、Linux 桌面、旧 alacritty 引擎、已撤销的 New Tab 拆分或 README 宣传改动，不采用 CI `-D warnings` 与 query-table 重构。
- #175 验收夹具：CLI E2E 忽略既有 `--nocapture` 输出选项但保留用例筛选，避免桌面 CI 零用例退出被误计为通过；Windows DA1 消费者在应用查询前完成真实 ConPTY 启动协商，恢复两种 DA1 的应用实收断言；SCM 消费者在最终绘制后等待当前 refresh epoch 的完成状态，不再将前一帧 loaded 跨异步阶段复用。保留业务断言与原期限，不修改生产回复或第三方代码。
- #175 图形额度消费者在跨 daemon 施压前实际绘制已滚动的旧窗口，并在准入等待期间执行真实帧，而非仅排队 refresh；不把 PTY viewport 屏障当作显示 lease 已退休的证明，保留生产 `1s` 准入边界、单次 SEND_OK、真实像素／历史／关闭释放断言与原测试期限。
- 上述条目记录实现范围，不表示所有原生平台已验收。#192 系统通知中心激活、#194 Unix opener 回收及 #197 更新后 adoption 未移植；按用户调整，macOS 采用实际成功的 CI 为合并门槛，取消隔离 Windows 身份下的 stored-toast 原生验收。未执行的 macOS 快捷键、FSEvents、交通灯、像素、更新／网络及 Windows 通知中心激活仍不计通过，默认 20 路完整套件的既有失败风险也不以窄场景通过抹除。


## [0.3.1] - 2026-11-05

### 依赖升级

- #171：`rio-vt`／`rio-graphics`／`rio-unicode` 原子升级到正式 registry 的精确 `=0.5.29`，同步锁定传递 `rio-grapheme-width 0.5.29`，保留关闭 VT 默认 features、原 PTY／daemon 和无 GPUI 的共享核心。所有消费者迁到立即解析、终端侧同步状态；完整 Fast／Rio 组合由集中检查点重新构建与验收，不沿用 #170 的结果冒称已通过。
- #170：仅通过 `gpui-kit =0.9.0` 的官方 `gpui-fast` feature 切换桌面后端，锁文件采用 Fast `0.1.13` 家族；保留语言／测试支持、HTTP `=0.3.11`、Rio `0.5.28`、单一终端帧所有权与原平台完整委派，不新增直接 engine 依赖、产品 Pre 回退或热路径探针。开发 profile 优化 Fast 链；macOS CI／Release 显式选择并记录实际 SDK 26+，默认 deployment target 与 bundle 继续为 macOS 15。原生行为、终端 retention、材质与同条件 Release 性能由集中检查点实际验收，未执行项不记为通过；macOS 原生结果仍由 #167 提供。
- #170 Windows 检查点已完成完整 Debug 套件（`5155 passed / 55 ignored`）、Release 构建、原生终端／Markdown 输入复制查找／主题字体缩放／窗口恢复／正式 HTTP 检查与 GUI 重开保留 daemon 会话。临时生产 Root／TerminalView retention 诊断未发现静态 pane 每帧重建，保留现有 buffer owner／paint phase；原生透明空白采样与升级前逐通道一致。以上不替代最终 Rio 组合、全部性能门槛或 #167 macOS 实机证据。

### 修复

- 空会话不再保存／重复恢复只有提示符和控制序列的旧屏幕；仍恢复布局并保留真实输出。Windows 默认 cmd 启动不再制造可累积的空恢复画面。
- SSH 的 SFTP 文件栏统一为可展开／收起的目录树，保留当前浏览根、嵌套多选与后台刷新；上传分别提供文件和文件夹选择，目录行可指定上传目标。
- 多选删除确认的路径列表改为独立滚动，操作按钮保持可见；Markdown 表格操作栏按当前表格左上角定位，随切换、滚动与缩放更新。
- 通知历史新增原文完整详情、选择与整条复制，并在界面说明收录范围；应用 Toast 历史与系统通知、终端输出、原始日志明确区分。
- Windows 产品描述统一为 `lyrel`，发布者／公司名为 `dlivxpr`；macOS bundle 同步标明项目作者并保留上游来源，不将元数据当作签名或公证证明。macOS 设置页移除重复的右上角关闭按钮，保留原生交通灯与未保存检查。
- omp 桥优先使用所属 daemon 注入的当前 hook 可执行文件，Windows helper 正确处理继承 ConPTY 的 stdin；保留真实失败诊断。恢复、切换及重命名从正式会话元数据同步非空标题，不再长期停留在 `cmd.exe`。
- 修复主分支 CI `41987369719` 的桌面测试编译挤占执行期限：以同参数 `--no-run` 独立编译全部测试目标，执行仍保留 20 分钟超时、完整用例与原有断言；不因预期 panic 日志修改产品代码。

- #168 审查修复：同步 hold 的等面积 resize 与 ESU／期限早于有序 `Size` 的过渡均保留 committed 行列与命中几何，竞争 fallback 不按新列数解释旧帧；relink 首个 hold 没有旧 snapshot 时按新 surface 呈现空帧，避免残留几何越界。可见复制按 committed 默认／自定义 tab stops 去除 NUL 展开后缀，保留 stop 后间隔、TBC／RIS／resize 增长、内部空行与折行空格；丢弃尾空行／全空选区的多余分隔，Lines 单独保留一个末尾换行。presented 拖选不再争抢实时 reader 锁。真实 Root／TerminalView 回归覆盖过渡、剪贴板及 Unicode／空白边界；held 不复刻上游旧 LeadingSpacer 复制读入前行字符的 quirk，normal 路径保持不变。受控持锁探针完成 red／green 后移出永久测试，原生和完整平台验收仍由集中检查点负责。
- #168／#172：Windows 非默认配置实例在启动、配置重载或语言切换时不再刷新当前账号的全局 Explorer 菜单标签；以 Windows 返回的实际默认 RoamingAppData／lyrel 为边界，默认实例与显式指定同一默认目录的行为保留，不改变注册／卸载。含修复的真实隔离实窗前后，两项真实 HKCU 键及子键的全部值／原生时间戳和用户 PATH 原始值种类／哈希均未变，只读比较、没有恢复注册项。原生 SSH 验收夹具同时明确关闭 PATH 安装／更新并隔离平台、agent 配置与继承端点，不改变生产默认配置。
- #171：在实际终端 snapshot／paint 发布点保留上次 committed 文字、光标、图片及可见交互，避免焦点／主题／光标重绘泄漏半帧。reader 通过现有链路读取期限独立执行 150 ms fail-safe，处理每次 BSU 重启、结束、resize／reset、命令边界、replay／relink 与退出。协议回复、输入模式及 daemon 图片锚点继续实时推进；CLI `capture --plain` 无 GUI 也捕获已收到的未闭合同步输出。补充 Root paint、可见复制、安静流期限、分片和真实 CLI／daemon／PTY 回归；未执行的测试、原生与性能项目保留为集中验收项，macOS 交由 #167。
- #171 组合回归反馈修正共享 Unicode／换行场景：显式设置 DECAWM，使 Windows raw sender 与 Unix 使用相同 autowrap 前提，不改生产解析或屏蔽字符差异；呈现行提取沿用 renderer 与既有测试的 packed NUL→空格约定，保留内部分栏，仅由逻辑范围比较裁掉行尾填充，不改生产字形。可见图片删除改为检查实际 TerminalElement 的存活像素、新 paint 和已提交 display list，不再要求无关祖先 view 的 render 计数增加。隐藏 pane 的无重绘与返回当前像素断言保留，修复后的完整结果仍由集中验收记录。
- #170 首次 Fast 编译发现搜索行的 `IntoElement` derive 仅从 Fast prelude 导入，展开后引用未声明的 `gpui::` 路径；改为显式使用 Kit 根提供的官方 facade-aware derive，保留原搜索行、ListDelegate、选择与无障碍行为，不添加 engine 依赖、alias 或手写 trait shim。修复后的编译与原有行为套件由集中检查点验证。
- #169 检查点修复真实 PTY 图片消费者、共享 retention 及原生 SSH 消费者的启动等待：同时从首个 `Snapshot` 与后续 `Output` 读取原有就绪标记，避免快速 sender 在订阅安装前已输出时误超时。保留真实 daemon／PTY、标记、原期限及后续像素／几何断言，不修改生产协议、不增加重试或跳过；本轮回归结果由集中验收记录。
- OpenCode 原生 API 启动失败保留底层 OS error，便于区分缺失文件、权限和资源错误；Unix shell 夹具改由独立写入进程持有可写描述符，避免并发 fork 继承后触发 Linux `ETXTBSY`。大 JSON 的真实 shell／Host 历史回归与私有文件捕获保持不变，不增加生产重试或跳过测试。
- 修复隐藏 SCM、最后持有者回收状态后，全局暂存、提交与推送等命令静默失效的问题；缓存缺失时通过原 Host 后台读取本次命令所需状态，不恢复常驻缓存或监听，读取失败显示通知，过期连接的结果不能触发操作。保留原确认、网络并发上限及组合命令顺序。
- 终端重挂载与竞争 attach 的真实 PTY 回归改用显式无用户 profile 的 shell 和独立临时目录，先等待 shell 输出的就绪标记，再单次输入命令；完整输出行与隐藏期间已进入回放的数据取代命令文本回显和固定等待。保留屏幕、几何及控制席位断言，不修改生产重挂载逻辑、不增加重试或延长超时。
- 原生 SSH 验收驱动的启动等待同时读取 `Snapshot` 和 `Output` 中的标记，避免 shell 就绪输出已进入首次快照时永久等待后续输出；只调整验收驱动，不修改生产协议或声称重新完成前台 SSH 图形验收。
- 文件树及文件名／内容搜索按视口和有界预渲染区构建行，保留完整逻辑列表、跨屏选择、异步定位与 Host 文件操作；行内创建／重命名输入的键盘事件不再串到树导航，顶部视口在异步搜索结果返回时不再跳向中段。同一查询刷新时按路径及命中行列保持滚动锚点，前方新增命中或文件组不再将视口拉回替换区开头。
- 同一 HostId 替换连接实例时，文件树、文件名／内容搜索、仓库根和 quick-open 重新读取当前连接，旧结果不能覆盖或解除相同路径的新请求；已打开的 quick-open 随成功重连刷新，隐藏消费者不因此主动发起读取，返回 Files 时重新确认当前实例并清除退休结果，同一已就绪索引不重复排序。没有项目目录时 quick-open 显示无根目录，不再永久停留在索引中；Files 不加入 All。复用当前 watcher 的实例身份，不新增历史 Host 缓存，保留其它窗口／Host 的现有所有权。
- 跨 Host 晚到目录读取回归先暂停新 Host 的请求，确认旧回调已到达且旧文件未进入当前树，再放行新列表；避免 watcher 安装后的补读覆盖测试探针的最后落地记录而造成误超时。保留新文件可见／旧文件不可见断言，不延长超时、不改生产 Host 隔离逻辑。
- CLI 会话生命周期压力回归在 Kill 发出后按既有关闭期限等待真实 registry 移除，再验证不能重新附着；工作区删除后同样等待实际关闭。避免跨连接的异步请求被误当作同步完成，不增加生产重试、不改变保留历史或关闭语义。
- Windows pane 列表在一次请求内共享新鲜进程表，将昂贵探测移出 registry 锁；ID／存活维护路径不查询前景标题，不将显示快照用于安全授权。
- 修复 PTY 输出先广播、OSC 标题后落地的发布竞态：只将单次解析出的同批标题／目录提前写入 pane 状态；没有这些元数据的普通输出跳过额外的提前锁／事实应用，空标题重置仍生效。输出广播后才执行机器树通知和昂贵的前台／周期／提示符探测，保留转发提示符抑制、Host 历史作用域及原始字节／分片顺序。无文本 SVG 按需解析字体，不再为拒绝外部图片引用等待系统字体扫描；原生图片回归通过既有文档图片事件等待真实完成，保留未授权零 HTTP 与显式加载单次请求约束。
- 终端持久化直接复制现有 256 KiB 存储后缀，避免先克隆完整 8 MiB 回放；保留字节、几何、标题与标记的一致性，以及 live attachment／Unix handoff 语义。
- 修复终端稳态 render 在提示符所有权同步、输入覆盖层几何或鼠标光标判断中等待 PTY grid 锁的问题；保留默认鼠标上报与提示符编辑，锁竞争时完整绘制上一有效帧，释放后恢复新内容。输入路由仍读取实时状态，不将受控锁竞争验收当作正常负载性能改善。
- SCM 无变化刷新复用状态和文件装饰；编辑器导航按 Rope 分块比较未变化文本，保留 Unicode、多行差异、撤销／重做与外部刷新行为。性能及原生验收按独立工单记录，不将局部归因或自动测试视作完整跨平台通过。
- Markdown 内联标记与 segment 使用有序单向匹配，消除全 marks 重复扫描；连续文字合并为共享完整 marks 的 run，仅在图片／公式原子处分开，避免 Paragraph 重复合并 highlights。保留 Unicode、组合输入跨段标记、链接、换行、图片／公式及局部源码的语义。Windows 无探针正常 Release 已完成完整约 2.5 MiB 大段落的默认 live 打开、源码切换、三轮首尾输入／Undo 与两次模式往返，原文逐字节不变；此前无响应记录保留，不将本次功能验证或 Debug public render 阶段改进记作整体性能通过。
- SCM 的最后一个实时持有者退出时回收工作树状态、文件装饰索引和刷新记录；文件树按当前窗口实际显示的全部根目录持有，稳态借用唯一持有集合比较，不复制／排序全部根目录或无条件触发可变全局观察者，保留先认领后释放及其它窗口／面板／编辑器／GitHub 消费者。重新订阅、实际刷新和 Host 重连使用不复用的读取身份，晚到状态、分支、历史图、详情与监听不能覆盖或解除新请求，旧缓存不能冒充新鲜结果；断连清理由各状态真实所有者执行，不依赖已回收的 epoch。修正测试中的远端 Host authority 与脏文档跨工作区保留断言，不改变生产内容保护；原生和性能验收仍由对应工单独立记录。
- SCM 晚到读取回归显式刷新共享真实目录的本地状态，再核对远端旧状态不能覆盖本地新文件；不依赖平台文件监视时序，也不把合法的本地状态更新误判为跨 Host 污染，保留旧结果不得发布或解除新请求的断言。
- 图片隐私回归的本地 HTTP 夹具先完整读取有界请求头，再发送 404；不再忽略一次 `read` 的结果并在分片请求未收齐时提前关闭。保留显式加载前零请求、加载后单次请求、404 错误、文档内容及光标几何断言，不改变生产网络策略。
- 补充非图形 session/history 的有限不改动结论：既有 Windows／Linux 最后状态所有者、输出队列排空，以及 Windows 原生关闭／收缩已有独立证据；保留可恢复会话，不新增 trim、压缩或容量削减。逻辑所有者释放不等于全部驻留内存归还 OS，不据此宣称无泄漏或整体性能通过。
- 文件树的 SCM 持有只使用既有仓库发现确认的全部显示工作树；普通目录和未发现目录不会持续启动失败的 Git／RPC 监听重试，子目录发现后仍能关联实际显示根，后续 `git init` 沿原发现路径启用。最后持有者退出后清理空闲网络计数器，在途写入在同一存活 Host 隐藏／重新订阅时共用原计数器，保留真实并发上限；真正断连仍清理旧连接映射，旧 guard 不影响新连接，不新增缓存或重试机制。

- omp 的公开 `PI_FORCE_IMAGE_PROTOCOL`、Kitty placeholder opt-in／硬关闭项沿用非敏感启动上下文，在新启动和原生恢复中保持显式图片策略；不覆盖 `terminal.showImages: false`、不伪造终端身份、不修改全局 agent 配置。修复 Windows 快捷启动将字面量环境前缀当命令执行，以及恢复命令将子进程图片策略遗留在父 shell 的路径。
- 补充 released omp 原生 `--resume` 的隔离合成会话与生产消费者用例，覆盖 assistant／toolResult 图片、原生 SVG 栅格图、8 张上游 cap 删除与显式关闭。正式 `18.14.0` 的中央三图几何／关闭策略及独立八图预算回归通过；Windows 原生八图身份、旧图 demotion、滚动及两种关闭策略通过，三轮审计均消息未增、零请求／模型用量。上游 `18.14.0`／`18.14.1` 的 assistant 按 content slot 共享 key，跨消息同 slot 会碰撞；预算夹具使用同一正式消息的不同 slot，不修改代理或生产 renderer，不宣称绕过此上游限制。最终 integration 另以正式 `18.14.1` 通过三项真实 PTY／生产消费者和零推理审计；18.14.1 原生窗口、真实 SSH agent 组合未执行，macOS（#152）延期。
- pi 图片接入（#149）区分 `earendil-works/pi` 与 omp，将公开 `PI_IMAGE_PROTOCOL` 纳入既有 Host／启动／会话 scope 与 Pi hook 报告，恢复时保留显式 `none`，不自动覆盖 `terminal.images=false` 或 `terminal.showImages=false`。正式 `1.4.0` 的真实 PTY／rooted 卡片像素和几何、删除／重挂载／重传及显式关闭优先级回归通过；Windows 原生二十图合成会话两轮重放、历史导航、删除、零新增消息与四种图片策略通过。无新 prompt／推理、不读私人历史；macOS 与真实 SSH 的 agent 组合仍待对应集成验收。
- OpenCode 集成按既有 `agent_launch` 的受检 Host／`XDG_CONFIG_HOME` 作用域定位 CLI 插件与配置，不再误写默认目录；保留显式 `session.image_preview=false`，不全局改设置或伪造终端身份。补充公开 `-s`／`--auto` 启动参数识别。正式 `2.2.0` 的 production consumer 与 Windows 原生首尾六图、卡片几何、分页、滚离返回、tab 卸载／重挂载及显式禁图均通过，公开 export 未变、零模型用量；MacBook Air M5 与真实 SSH 的 agent 组合尚未验收。
- 修复 synchronized update 中 Kitty 命令先于暂存文字执行、导致多图共用旧光标锚点的问题：daemon 权威镜像按流顺序及时应用文本，桌面保留自己的同步呈现。真实 PTY／daemon／rooted consumer 回归先失败后通过；pi、OpenCode 的卡片几何和 Windows 原生图片随后实测通过，omp 的中央三图、完整八图预算与 Windows 原生像素亦已通过；各自正式版本及尚未执行的跨平台组合按下方独立记录区分。
- 收敛 Kitty 审查修复：选中子帧解码后唤醒实际放置、按 `I=` 替换时核验放置环、agent 快捷启动／恢复从目标 Host 的 `ShellInventory` 默认项解析 shell，拒绝时通过持久错误通知提示修改 `agent_launch` 或在新窗格手动启动，并用 Windows 固定身份 handle／`ReOpenFile` 避免临时媒体读取／删除落到替换对象。三个 replay 共用 PNG 编码，Kitty fixture 共用 stdout 背压与 4,096-byte 分块，SSH 上传／回读统一且携带共享模块；新增夹具回归。文档统一当前 pane `17`／control `16` 与 Pi／omp 已有通过记录；第二层 nested SSH 已用真实 Linux 端点验证 indirect 拒绝和 direct 成功。本轮修复后的构建／测试及原生回归由中央统一执行，不沿用旧记录冒称重跑。

### 新增

- Kitty 图片支持 daemon 持有的完整帧追加、停止／手动选帧和同一存活 daemon 的 fresh-client 恢复；同资源全部放置共用选择与像素，帧序列按整体回收并沿用有限源内容／派生／元数据准入。完整帧最初引入 pane `16`，后续播放扩展后当前统一为 pane `17`／control `16`；兼容显式 `f=0` 与无歧义的最终块无 padding Base64。真实 PTY／daemon／rooted consumer 及 Windows Debug 原生窗口已验证三帧同步切换和发送器退出后的 fresh-window 恢复；本条只记录完整帧切片，不据此宣称其他容量／性能或 macOS 门槛通过。
- Kitty 自动播放由 daemon 保存 stopped／loading／running、frame gap、有限／无限循环和剩余间隔；聚合当前连接 viewport 的 direct／relative／Unicode 可见性，全隐藏或断开时暂停，显式命令仍生效。循环复用保留像素，只发布选帧／播放元数据，pane 升级为 `17`，control 保持 `16`。Windows 五项受控时间、一项真实双消费者／fresh-window 回归与原生播放／loading／追加／隐藏返回／发送器停止后恢复已验证；不据此宣称双 pane 1080p、60 fps 或 macOS 通过。
- Kitty `a=f` 增加 partial append／retained frame edit：支持 prior-frame 底图、32-bit RGBA 背景、像素矩形、alpha／overwrite、raw／PNG／zlib，使用锁外有界准备与新不可变内容 key 原子提交，保留全部 placement 及 fresh-consumer 最新版本。Windows 核心事务拒收、stale prepare 竞态、真实生产消费者像素／恢复回归和原生局部追加／编辑／root 编辑与无重传恢复通过；播放中合成／删除、完整容量与真实 native SSH 的后续通过记录分别见下文，约 60 fps 已由后续 120 Hz WGC＋PresentMon 交叉记录验收，macOS（#152）延期。
- Kitty `a=c` 支持 retained source／target 矩形 alpha／replace 与同帧非重叠复制；`d=f/F` 支持首／中／当前／尾／唯一帧删除、稳定 root 提升、连续编号及 placement／gap／loop 保持。复用锁外有界 immutable preparation／commit，旧代次不能在删除／替换后复活；增加真实 PTY／daemon／rooted consumer、受控时间和准备／解码／snapshot 竞态入口，以及独立 `scripts/kitty-frame-mutations.mjs` 原生播放中合成／删除夹具。Windows 核心、真实 rooted consumer 与原生 compose／replace／edit／删除／loading-append 已通过；真实 Linux native SSH consumer 及标准 GUI 的合成、替换、删除和恢复也已通过。隐藏时删除当前帧保留暂停剩余间隔，完整容量／60 fps 由 #146 单独验收，macOS 未验证。

- Kitty 相对放置在既有 daemon Placement／pane transport 中保留 `P/Q` 父身份、signed `H/V` 及八链接有界图；支持 direct／实际 Unicode 单元父锚点、无光标移动、级联生命周期和 fresh consumer 恢复。Windows 五项真实 PTY／daemon／rooted view 回归及 `scripts/kitty-relative.mjs` 原生 geometry／recovery smoke 已通过；真实 native SSH 的 rooted consumer 和标准 GUI relative／fresh 恢复也已通过。完整 DPI／性能、正式 agent 的真实 SSH 组合未完成，macOS（#152）延期。
- Kitty 间接媒体增加 Windows 本地 regular file、eligible 临时文件与 named shared memory；`O/S` 只限制实际图片区域，允许小区域位于大对象中。Windows 固定已验证本地磁盘 handle、拒绝网络／device／未知 reparse，并按对象 handle 删除临时 handoff；Unix 保留文件／POSIX shm 语义，macOS 按偏移附近有界映射，临时文件使用身份核验及 no-clobber 恢复避免删错替换对象。发送方 locality 每次接受及读取前重新证明，嵌套 SSH／WSL 或未知命名空间 fail-closed，直接传输不受影响；不新增图片磁盘缓存。Windows 真实 PTY／rooted consumer 与原生三类媒体红绿像素／安全应答 smoke 已通过，Linux headless 完整 features 与 native SSH 三类 Linux 媒体回归通过；第二层 nested SSH 已验证 indirect media 拒绝、direct RGBA 成功及桌面同名对象不变，macOS（#152）延期。
- Kitty 双 pane 1080p 容量（#146）按真实 allocation owner 提升默认有限预算：daemon 每 pane encoded／RGBA 权重各 512 MiB、aggregate 各 1 GiB，queue 1,280 MiB，work 保持 128／256 MiB；桌面 cache 为 1,536／3,072 MiB、source 为 1,024／2,048 MiB，向 daemon 上报每 pane encoded／RGBA 各 512 MiB。新增 `scripts/kitty-capacity.mjs`，通过既有真实 PTY／Kitty 路径逐帧产生两组固定 seed、各 60 张不同 1920×1080 raw RGBA 噪声帧与 8-bit frame ID 条码；单 frame buffer、逐帧 ACK／backpressure、路径独立的脚本 SHA-256／source manifest，以及 load／daemon start-stop／edit／compose／完整 replacement／delete／普通输入与 sender 退出入口。现有 concepts 公布 `P/F/N` 所有权公式、实际退休边界与 PresentMon 2.6.0＋ffmpeg 9.0.2 自有 HWND WGC 复现命令；完整容量、小预算所有权和 120 帧生产 consumer 已通过。Windows 原生 120 Hz WGC 30 秒实测左右分别为 59.12／59.02 次图片变化每秒，p50 16.67 ms、p95 25／33.33 ms、最大 91.67 ms；源 ID 跳号下界 26／29，重复 capture 135／138，并分别记录分进程内存及 delete 后释放。经用户授权的一次管理员 PresentMon 记录 1,941 个 present、6 个未显示，DisplayedTime p50 16.658 ms／p95 20.841 ms；与 120 Hz WGC 共同通过约 60 fps 原生验收。input-to-photon 指标全部 NA，不冒充输入延迟；真实 SSH 全尺寸加载／steady／fresh 另已验证，macOS 未验证。

### 维护

- 发布前清理构建警告：SCM 原子身份分配迁到 `try_update`；SCM／图片检查辅助接口及其元数据仅编入测试；桌面 SSH 回归共用唯一夹具入口，同步捕获数据仅由实际消费者编译，保留全部回归断言。macOS 专属测试移除无用导入，Windows handoff 辅助接口按平台限定，不新增 lint 屏蔽。
- Debug／测试默认启用轻量优化，覆盖工作区及依赖以缩减未优化代码及展开信息，现有热路径依赖的级别 `3` 保持不变。保留平台默认链接、panic 展开、析构和回溯，不关闭 compact unwind 或 linker diagnostics，Release 配置不变。候选 macOS CI 揭示手动 DWARF-only 选项导致预期 panic 用例中止，该选项已撤回，原用例与断言保持不变。
- CI 与 Release 的 musl 交叉工具链同步固定到正式 Zig `0.17.0`，避免 `0.16.0` 自带 libc++ 的 nullability 警告；保留真实静态链接／平台断言与精确构建来源校验，不屏蔽 C++ 或 linker diagnostics。
- 修复基线 `2187893` 的 CI `42000861816` 已通过 Windows／macOS／Linux 构建测试、独立 server 与真实发布打包验证；本版版本变更与警告清理仍以其自身提交的 CI 和 Release 结果为准，不沿用该基线冒称重新验证。

### 验证入口

- 本轮十项问题通过 Windows GUI 后台消费者 `3026 passed / 21 ignored`、core（串行）及 server 完整 all-features、Linux headless 完整 features、Release 和真实 ZIP／Inno 包校验。真实固定公钥 SSH／SFTP 的树状点击、文件／目录选择及指定子目录上传逐字节回读通过；真实 Windows ConPTY 的六项恢复回归与正式 `omp/18.16.0` 恢复身份／标题、陈旧桥路径修复通过，最终恢复审计零请求且消息数不变。单次并行 workspace 曾在未改动的端口转发用例出现 `AddrInUse`，串行 core 通过，不将其记为单次全 workspace 绿灯。CI 修复通过 95 项自动化、actionlint 与实际测试目标预编译；未推送／重跑远端 CI。macOS 实机由用户后续自行检查，未将 Windows／后台消费者结果当作原生 macOS 验收。

- #172 最终生产候选 `ead83a2` 已实际通过 Windows 格式、locked workspace all-features（5166 顶层标准测试、另 CLI E2E 26，56 ignored 不算通过）、updater 及 Release；updater 30 项已含于标准总数，8 个嵌套 helper 不重复累计。真实 musl server、Windows ZIP／Inno 包布局／版本／ConPTY 与三个自有 exe 的 VC++ runtime 检查通过，没有运行安装器或发布。
- #172 最终 Release 的本地 ConPTY 分片／Unicode／alternate-screen、可见选择复制、同步期 CLI live／GUI held、静默释放、编辑器／Markdown／Diff、明暗与自定义主题／字号／侧栏／窗口控件、同 daemon／PTY 重开及真实 pinned-key SSH 双图片／新 GUI 恢复取得原生证据。同条件重复采样的已测成本／内存 median 均未超过 5% 退化；load 基线仅一个独立 cohort、旧 baseline 未重新交错运行，指标不等于 GPU 或物理延迟，不保证提速。真实 IME、隔离 LSP、远端编辑输入／保存、部分快捷键与焦点恢复等仍 blocked／unverified；维护者决定以实现交付收尾并随 PR 合并关闭 #168／#172，不将这些项目记作通过。完整身份、原始证据和边界见 `docs/design-system.md`，macOS 仍由 #167 单独验收。
- #172 最终 scale1 的受控黑白／checker 像素验证 opaque、opacity0.88 的 Off 及 Light／Dark 的 Blur／Mica／Acrylic 往返，字号保留；终端／两侧栏透出逐 RGB 与升级前相同，Blur 降低同一空白 ROI 的 checker 方差。Mica 的恒定 patch 不解释为实时桌面 blur，不推广到多 DPI／任意背景或 macOS；最初文件名与实际主题不一致的样本按 manifest 明确区分。
- native SSH 增加真实 Linux daemon 的动画／partial edit／compose／单帧删除／relative 与 Unicode 共享版本／隐藏多消费者／无 consumer 变更／fresh 恢复／replay-live cutover 契约，补充拒收／取消 grant／observer 控制权／不兼容方言及 Linux file／temp／POSIX shm 与 nested locality 实连入口。新增非 shipping `native_ssh_surface` 执行器，通过普通远程 workspace／machine tree 打开标准 GUI，复用同一全尺寸双 pane workload 并分别记录初始与 steady 的真实 SSH 图形有效负载计数；不新增 renderer／协议替身／GUI flag。当前 Linux 实连的 4 项 transport、4 项 rooted consumer，以及标准 GUI 的 relative／mutations／双 pane 1080p capacity 均已执行；该轮基础 transport 使用 `--skip nested_ssh`，随后第二层 nested 实连单独执行并通过，未将其混入四项基础计数。每 pane 60 个 Resource 包后 steady／fresh 仅 Select 增长，fresh 恢复与 delete 实图通过。修复空 graphics replay 的 sampler 启动屏障和隐藏删除的剩余间隔，不宣称 SSH 60 fps 或 macOS 通过。
- 正式 OpenCode 2.2.0 隔离保存对话夹具使用公开 import／resume／export，无 prompt／推理；完整目标保留用户附件、tool-result、每卡三图、旧消息分页、滚出返回、native tab 删除与重挂载及 disabled 策略。中央已完成 546 条消息 prepare 和 enabled／disabled consumer；独立 placement instance 计数保留官方 16×8／gap=1、容器文本与 viewport 边界，同色／同原点放置、同 ID 替换和精确删除均已覆盖。正式 2.2.0 的 production consumer 与 Windows 原生首尾六图、卡片几何、分页、滚离返回、tab 卸载／重挂载及显式禁图均通过，公开 export 未变、零模型用量；macOS 与真实 SSH 组合未验证。
- #151 集成验收：`cargo test --workspace --locked`、WSL `cargo test --locked --workspace --exclude lyrel --all-features`、`cargo test --locked --features updater --bin lyrel-updater`、`cargo fmt --check` 与 Host 边界检查通过；WSL 使用隔离 Node `22.23.4` 执行 TypeScript bridge。正式 `omp/18.14.1` 的 pictures／budget／disabled 三项、pi `1.4.0` 两项和 OpenCode `2.2.0` 两项真实 PTY／生产消费者均通过，omp 审计零推理。真实 SSH 另执行 4 项基础 transport、4 项 rooted consumer 和原生 relative／mutations／capacity，第二层 nested 单独实跑通过。审查修复后已重跑 Windows／WSL 全套件、三项正式 agent 组合和全尺寸场景；管理员 PresentMon＋WGC 交叉记录约 60 fps，1×／1.25×／1.5×／2× DPI／z-order 通过，正式 pi 与 24 张 1080p 压力图片同屏且保留画面正确。input-to-photon 全部 NA，Kimi／Hermes 仅承诺普通 Kitty 输出而非历史回放；正式 agent 跨 SSH 未执行，macOS（#152）由用户后续自行验证。

## [0.3.0] - 2026-10-31

### 新增

- Markdown/MDX 默认使用原生即时编辑，可在同一文档内切换完整源码；共享内容、revision、可逆事务和撤销历史，各呈现独立保留选择、焦点和滚动。移除 `--markdown-workbench` 开发入口及旧只读预览设置；普通代码、新建纯文本与 scrollback 保持原编辑器，并新增显式“新建 Markdown 文档”命令。
- Markdown/MDX 按行保留 LF／CRLF，局部编辑、撤销／重做和组合输入取消保留片段表示；保存保留编码与 BOM，不执行全文整理。损坏的声明编码拒绝打开，不可编码输入保留可见错误；显式编码／换行转换继续沿用既有工作台。
- 连续正文／标题、行内标记、链接、列表／任务、GFM 表格、代码、公式、Mermaid 与跨结构源码剪贴板共用原生输入和文档事务。查找／替换、行命令、导航、所属 Host 保存、外部冲突及关闭协调沿用现有工作台；图片加载、系统截图与拖入资源不跨 Host。
- 全部 12 个专用 MDX 组件及未知 JSX 支持属性／children 与局部源码编辑，不执行项目 JavaScript。MDX 使用正式 `ox_content_parser 3.4.0` 公开 AST；公式、图形使用正式 Merman、RaTeX 与原生光栅化，保留相关字体、来源与许可，不依赖 WebView 或项目运行时。
- 本机、远程工作区文件树及普通 SSH 的 SFTP 列表支持 Ctrl／Cmd 多选与真实文件／目录批量复制、粘贴和删除，支持 Shift 点击范围选择；右键保持已有选择，删除确认固定所属 Host 与完整目标，文件树祖孙去重、根目录保护及部分失败报告沿用同一操作入口。

### 依赖升级

- #169：GUI 锁定正式 `gpui-kit =0.9.0`、配套 `gpui-pre 0.3.11` 与 `gpui-pre-reqwest-client =0.3.11`，保留 Pre 中间检查点、语言能力与测试支持，不切换 Fast／Rio。适配后台 Mermaid 测宽缓存的并发回调边界，并补全 macOS 平台电源模式与原子凭据写入委派；保留已有 Root 和主题同步。构建、原生工作流与性能验收由 #168 分阶段记录，macOS 仍需 #167 实机证据，不沿用此前验收结论。
- 合并 #131 的九项 minor／patch 升级，以及 #132 `rusqlite 0.41.0`、#133 `russh 0.65.0`、#134 `sysinfo 0.39.6`、#135 `parking_lot 0.13.0`；保持正式 registry 依赖和锁定构建，不引入第三方源码补丁。


### 修复

- 修复远程工作区文件树停留在仓库根、SFTP 同一窗格 `cd` 后不跟随的问题：浏览读取前景终端的实际目录，项目搜索仍保留原范围；SFTP 按 workspace／pane 隔离浏览记忆，拒绝关闭、切换与重连后的迟到导航。
- 修复文件树折叠后重新展开复用旧缓存、跨 Host 防抖丢事件及旧连接监听结果接管新连接的问题。重新展开和监听安装后补读目录，目录级通知也刷新子项，列表加载期间保留可用内容；普通 SSH 的 SFTP 列表约每两秒后台重读，暂停编辑期间的刷新并隔离旧请求。
- 修复 Windows GPUI 不写出 `ExternalPaths` 导致复制后粘贴仍不可用的问题：保留实际来源 Host，使用受支持的带身份剪贴板格式，外部复制覆盖剪贴板后使旧载荷失效。原生 SSH 的“信息”不再将本机默认 shell 冒充远端登录 shell。
- 修正 CI 的三个真实环境／时序问题：Windows 拖入消费者等待完整文件内容而非刚创建的文件；图片 PTY 夹具使用不申请图片资源的有序应答屏障，避免 resize 合法取消 query 导致夹具退出；Unix PowerShell 使用私有 HOME／XDG runtime，避免失效运行目录使 PSReadLine 历史 mutex 初始化失败。
- 修正 macOS 多主机防抖回归的等待边界：原测试只等待 100ms，短于生产 200ms 的真实定时器；改为有截止时间地等待两台 Host 的失效转换，保留跨 Host 不丢事件的双重断言，不修改生产周期。
- 修复 Markdown 跨扩展名另存为后的重载解码不一致、分屏 change 命令误用其他呈现、多行缩进丢失选区、正文边界删除破坏表格、行移动／复制改写混合行尾及表内 MDX 控件重复。查找复用正式 SearchMatcher 的匹配结果，渲染派生图片／无障碍快照按资源事件或 revision 失效；补齐相关原生控件的中英文无障碍文案。消费者回归验证原逻辑失败、修复后通过；最终实际窗口覆盖相关输入、保存与完整字节。
- 表格／段落内的 Tabs 和 Accordion 按组件源码范围隐藏未激活／折叠的子内容，保留周围单元格、原始字节和输入映射；空子内容保留明确的零宽源码插入锚点，显式激活后输入不会落到组件外，普通可见正文导航也不会被空锚点抢走。切换与折叠不创建文档编辑，完整源码模式继续显示所有分支。
- 修正六项 Markdown 工作台消费者夹具的临时目录拼写：按 `LocalHost` 解析 runner 的 `/var`／`/private/var`、Windows 8.3 名和 junction，避免未规范化路径误用于文档身份与重命名通知。保留产品逻辑、原有字节／身份断言和超时；Windows 别名 `TEMP` 中原失败用例先红后绿，完整编辑器 282 项回归通过。

- Windows AUMID 构建目录识别补齐 `target/[<triple>/]{debug,release}/deps/`：即使缺少 `CACHEDIR.TAG`，测试／开发程序也不会被误当作安装版重写正式快捷方式。15 项 AUMID 回归及完整默认成员 `cargo test --locked` 通过；从 `deps` 路径启动的隔离实窗未创建正式 Start Menu 快捷方式。
- 修复单行 JSX 子内容在 Tab、Step 等原生组件中重复显示：后台投影安装区分同起点的空 children 边界与非空正文，每个旧 leaf identity 最多复用一次，避免正文污染边界的渲染／输入缓存。新增回归先失败后通过，编辑器 267 项回归通过；修复后二进制实窗确认单行正文只显示一次，IME 保存重开源码不变。
- 修复隔离原生 Markdown 在连续输入时被面包屑 outline 周期性同步解析阻塞的问题：仅该路线将 outline 派生移到后台，校验语言与源码快照，保留显式符号导航和普通代码行为。新增 fresh／排队旧结果及关闭后同路径替换的工作台导航回归；最终无探针二进制已实测新标题、符号跳转及保存重开的完整字节。真实本机／SSH 长文、单大段落／代码块和多资源输入到绘制已量测，边界及原始记录位置见现有编辑器文档。
- 修复原生 Markdown 阅读中切换主题／字体／字号时的视口跳动：仅颜色变化保留高度提示，重排先物化稳定 leaf，保留纠偏后的锚点与 offset，并使过期帧几何不能触发迟到公式的光标重入。Windows 实窗覆盖离屏光标、12–18 UI 字号、明暗主题、Arial／默认字体与窄宽窗口往返；新增消费者保留原始选择及完整源码字节。
- 修复完整宽表进入正式 `gpui-base 0.8.1` retained-layout 清扫阻塞：共享列宽、实际可变行高与二维虚拟化只保留可见单元格布局，表格独立横向滚动，源码显露／资源／重排使测量失效，删除表格回收滚动状态。完整 120×160 表格已在 Windows 本机与真实 SSH 输入、滚动、保存关闭重开并独立核对全部字节；不修改第三方源码或用固定行高规避。
- 修复非焦点编辑分屏首次图片拖入被活动呈现校验取消：资源位置请求先同步组焦点，保留文档 Host、选区与统一事务归属；新增工作台消费者先失败后通过。
- 修复完整二维宽表的组合输入首轮候选位置和单元格源码映射：大表单行编辑有界重解析当前行并保留表格布局，边界、空单元格及无外管道末格保留正确插入归属；结构换行临时源码按物理行呈现并保留待解析语义，连续输入不吞掉换行。Windows 实窗已以真实微软拼音复验本机／SSH 提交、取消、撤销／重做及保存重开，完整宽表响应重新量测，见编辑器文档。

### 验证范围

- Windows 实窗补齐 live／组件系统微软拼音、跨结构系统剪贴板、本机与真实 SSH 的系统截图资源写入／显示／撤销／保存重开，以及仅阻断测试程序后的冷启动离线公式／图形编辑。上述结果分别记录，不用输入注入、静态截图或本机 Host 替代 OS IME、离线或真实 SSH。
- 完整宽表阻塞已解除；切换前编辑器定向回归 272 项、默认成员测试累计 4973 项通过，36 项显式忽略。后续已补齐真实 Explorer 跨窗口拖入、本机与 SSH 的宽表系统 IME、延迟保存、旧另存为条退役、完整编码保真和组件可见性连续序列，原始记录见编辑器文档。负责人明确决定本次不做 macOS 实机测试，记录未执行而非通过。
- 生产切换及最终审查修复后，完整 `cargo test --locked` 为 4985 项通过、36 项显式忽略，桌面 Debug 构建、格式和 Host 边界检查通过。最终实窗复验表内空／非空组件、相邻正文插点、重复缩进、混合行尾移动和跨扩展名显式重载；实际范围与未验证项见编辑器文档。


## [0.2.3] - 2026-10-28

### 变更

- GUI 升级到正式 `gpui-kit 0.8.1` 与配套 `gpui-pre 0.3.10`，同步 HTTP transport、平台信息、动态快捷键、修饰键、菜单提示和滚动 API；保留 `tree-sitter-languages`、测试 `test-support`，不启用 `gpui-fast`。

### 发布

- 本阶段不要求 Developer ID 或 Apple 公证，macOS 正式包继续使用 ad-hoc 签名，Windows 包保持未签名；保留架构、完整代码签名、归档 SHA-256、精确版本及更新来源校验。ad-hoc 身份随代码变化，升级后仍可能需要重新授权，不将本版本描述为权限延续修复。

### 已知限制

- omp 表格缺字尚未复现：生产 transcript、终端单元格快照及隔离真实 PTY 重放均保留完整内容；原生截图与 Accessibility 仍被 macOS 拒绝，不能将框架升级记作终端丢字修复。

### 维护

- 按职责拆分设置窗口状态与工作台桥接、终端文件／图片粘贴，以及 daemon pane 的启动准备与回放缓冲；保持现有 CLI、IPC、配置格式和会话行为。清理无调用的终端辅助方法及测试无用绑定，将测试专用导入、图片快照与 Unix Attach 判别限定到对应测试范围，并把 SSH 十六进制解码夹具移至唯一使用它的 server 测试。

## [0.2.2] - 2026-10-26

### 修复

- 修复合法 zlib RGBA 图片使桌面解码额度减法下溢、锁中毒并导致 GUI 退出的问题：解码完成后，在 scratch 额度仍有效时去除像素缓冲区的多余容量，再转为常驻 CPU／GPU 资源；不以饱和减法隐瞒额度错误。新增真实解码、像素与资源释放回归，并在原生窗口复验原崩溃路径。

### 验证

- 补充 Windows 100% 显示缩放的 direct／Unicode／alpha、分屏／字体／透明材质、GUI 进程重开无重传及 4K 历史压力／GPU 内存采样；Windows 原生 SSH → WSL2 Linux 验证图片、替换／删除、压力和本地 router 重建后的同一远端会话恢复。精确边界与数值见 `docs/getting-started/concepts.mdx`、`docs/remote/ssh.mdx`。
- 修正真实 PTY 验收夹具的完成边界：图片 GPUI 消费者等待光标恢复之后的有序标题标记；CLI resize／capture 等待末行 CRLF 完整到达，避免 ConPTY 分包导致过早光标断言，或换行落入 resize 后的新 replay segment；不改产品的光标、拒收或 capture 行为。
- 正式 `omp/18.12.6` 的公开 `--resume` placeholder 路径在原生窗口显示受控 SVG 与真实测试窗口截图；没有模型推理，不冒称真实新回答。direct 模式原生重放出现位置重叠，暂推荐 `PI_KITTY_PLACEHOLDERS=1`；macOS、其他 DPI、远端正式 omp 与完整平台验收仍未完成，#39／#40–#47 保持开放。

## [0.2.1] - 2026-10-25

### 新增

- 接入 Kitty 图片资源与独立放置（#40）：`a=t` 只上传，`a=p` 独立／重复显示，`a=T` 保持上传并显示；多次放置共享像素，并支持源裁剪、透明背景、身份替换、资源重传、静态删除选择器及带身份的错误／quiet 模式。资源与放置由 daemon 的共享核心状态管理，桌面保留异步解码、旧工作取消及 atlas 释放，不新增图片落盘。当前数值预算见下述 #44；当前 pane 协议为 `15`，control 保持 `16`，旧 daemon／远端 server 需要更新。
- #40–#46 的核心、真实 daemon-session、GPUI 消费者与 reader／decoder 代次回归已接入；Windows 完整默认成员测试通过，Linux 默认 core 与图片 daemon 回归通过，另有真实 Windows→Linux 原生 SSH transport／rooted consumer 通过。Windows/macOS 原生像素、SSH 原生窗口、正式 omp 实窗与最终资源峰值仍待完成，不据此宣称 #39 已交付。
- 加固 Kitty 流式传输与拒收（#41）：支持 PNG 的外层 `o=z`，分块逐次解码并限制 partial／APC 分配；对 PNG 做有界逐行真实校验，对 raw／zlib 检查尺寸、精确字节数及展开上限后才应答、保留或创建放置。查询不保存资源，文件／共享内存能力按负责主机与实际读取结果应答；临时文件删除要求标记路径。Linux POSIX shm 使用有界 descriptor read；macOS 使用不可重复截断对象的受限只读共享映射，复制后立即释放并预留映射加副本的峰值。中断、取消、删除与下一条完整命令保持有序，匿名／quiet 请求不污染后续输入。
- 隔离异步图片 reader／decoder 的代次：退休连接不能应用迟到帧或解码完成，也不能在延迟退出时关闭新连接的图片队列。新增真实 daemon 的格式、压缩、拒收、49 MiB 超限与输入存活回归，GPUI 实际 decoder／paint 回归，以及生产 decoder 的受控代次竞态回归。
- 修正分块续传首个键为 `q` 时 quiet 模式被忽略的问题；`q=1/2` 与 `m` 的排列顺序不再改变应答，真实 daemon 的成功／失败静默、恢复应答及后续输入回归已通过。
- 接入 Kitty 终端布局与屏幕生命周期（#42）：daemon 权威文本锚点计入历史丢弃，复用 VT 网格的行重排，处理页边距滚动及永久裁切、主／备用屏隔离、精确清屏／reset／历史清理，并按实际设备像素统一回答 `CSI 14t`／`CSI 16t`。隐藏窗格保留图片状态但不触发无谓整窗重绘；清理命令仅允许控制器。真实 daemon 与 GPUI 几何／状态回归已在 Windows 通过；最终 GPU 裁剪／透明／z 合成、字体与 Windows/macOS 显示缩放原生验收按用户安排延后，测试窗口无 `HeadlessRenderer`，不以缓存或绘制标记冒充像素通过。
- 接入 Kitty Unicode 占位符（#43）：`U=1` 仅创建不可见 prototype，真实 Rio 单元的 indexed／true-color 身份、行列／高位 diacritics 和规范左至右继承决定显示切片；无效引用不造像素、不绘制 PUA 字形。共享投影同时供 daemon 资源引用和桌面 viewport 使用，直接／虚拟放置共用资源与预算，删除语义分开，文字移动／擦除、屏幕与几何沿用现有路径。共享真实 PTY fixture、独立 daemon 到 rooted GPUI production paint 的单链路回归及独立 CLI／ConPTY 协议冒烟已在 Windows 通过；原生 GPU 像素仍待验收，不承诺 tmux/screen。
- pane 会话开启前强制双向版本协商（从协议 `10` 开始）：本地、路由及原生 SSH 的新建／附着／观察连接在发送会话请求前确认版本；旧客户端在创建 shell 或订阅图片前明确拒绝，新客户端遇到旧 server 显示版本与更新要求。独立诊断探测保留，不把 EOF 当作版本判断。
- 加入图片保留预算与历史回收（#44）：按 daemon 全局准入序号先回收无引用资源，再回收最旧非可见历史；direct 与 Unicode placeholder 共用可见性。保护内容耗尽预算时拒新图，历史原位置仅绘制双语 muted 回收提示，不改终端文本／选择／光标／PTY，不为主动删除或 clear／reset 留提示。隐藏／detached pane 仍受预算与元数据配额限制，异步退休资源实际释放前继续占账。
- 图片预算默认：单图编码／RGBA 权重各 32 MiB（容纳约 31.64 MiB 的常规 4K RGBA 截图）；daemon 每 pane 编码 64／RGBA 权重 128／工作 128 MiB，全 daemon 分别 128／256／256 MiB，图形 packet 聚合另限 256 MiB。桌面 source 字节池合计 256 MiB，派生 CPU／GPU／解码工作合计 1,024 MiB；按实际图片与至少 `1024×1024` 的 atlas page rounding 预留，空 pane 不占整额 lease，小图没有“两张”或“四个窗格”的计数限制。资源 offer／grant 在成功准入前获取真实 source／decode／GPU 额度；不持 pane 锁等待，新桌面装不下保留图片时明确拒绝附着而不丢权威内容。
- 图片及无像素回收提示的元数据也持有真实 byte ticket：每 owner 512 B，每 pane 4 MiB，daemon／桌面各自 aggregate 16 MiB；像素回收不释放 notice 票，snapshot 实际释放前继续占账。新独立放置及 replay notice 通过零像素 offer 准入，`a=T` 的资源与放置授权全部成功后才修改内容。
- 真实 daemon-session／GPUI consumer 小预算回归已在 Windows 通过，并提供 `scripts/kitty-pressure.mjs` 原生压力夹具；原生 Shell／GPU、macOS 与真实 SSH 峰值仍未测。文档公布每进程会计和组合范围，不把桌面 1,296 MiB 会计或 daemon 656 MiB 会计当作 RSS／驱动峰值实测，不提前关闭 #44／#39。
- 历史 pane `14`／Windows Debug 的独立生产 CLI → ConPTY 实测：两张 4K raw RGBA 加一张 PNG 均收到关联成功应答，daemon 工作集在保留时为 85,999,616 bytes，内核高水位为 118,820,864 bytes，pane 正常退出后回落到 19,660,800 bytes；普通输入可用，CLI／发送器／私有 daemon 正常退出。此结果仅覆盖当时的 daemon CPU，不覆盖本次 pane `15` 修复，不包含桌面／GPU 或原生压力验收，不能替代 #44 平台证据。
- 接入存活会话的有序图片恢复（#45）：新消费者从同一 pane daemon 恢复保留资源、多放置／裁剪、占位符、历史与 screen；detached 产生／替换／删除和 replay 到 live 的并发保持一致。纯文字／几何变化不重复取消适用资源授权，资源集合变化先释放旧授权再有限重取；容量不足明确拒绝附着，不复活已删除／回收像素，不要求应用重传，也不新增图片落盘。
- 接入真实 native SSH→Linux 图片回归（#46）：生产 router／SSH 引擎／Linux PTY 到 rooted GPUI 共用 pane 协议，核对直接及 Unicode 放置、像素与几何、拒收／历史提示、本地 router 重启后的同 daemon fresh 恢复及无重传。历史 pane `14` 的 Windows `10.0.26300`→WSL2 Ubuntu `26.04.1` 实测中，1 项 transport、2 项 rooted consumer 已通过；文档列出固定主机公钥、隔离 HOME／SFTP 和复现命令。此历史结果不覆盖本次 pane `15` 修复，不将其冒充原生 GPU 或 macOS→Linux 验收。
- 验证正式 `omp/18.12.6` 的公开图片选项：保持 `TERM_PROGRAM=lyrel`，`PI_FORCE_IMAGE_PROTOCOL=kitty` 配合 `PI_KITTY_PLACEHOLDERS=0/1`，真实交互 TUI 在 Windows ConPTY 中发送 SVG 栅格 PNG 与合成 toolResult PNG，独立 daemon／生产 `PaneClient` 收到对应资源和直接／虚拟放置。隔离会话未提交新 prompt，`/exit` 正常退出；此结果不替代原生像素、真实桌面截图、GUI 关开或 SSH 验收。

### 修复

- 修复终端 grid 锁竞争导致最终滚动视口丢失的问题：至多保留一个后台唤醒，取得锁后采样最新状态；隐藏撤销已有视口，首次 Screen 与重连后重新报告，不积压过时快照。发送连接仍使用既有积压错误边界，不将其描述为无限重试或可靠送达。
- 修复桌面图片压力请求被 daemon 拒绝后永久锁住的问题：每资源只允许一个未完成请求，daemon 在有序公布权威回收效果后，仅向请求 endpoint 返回 `DaemonMsg::GraphicsPressureResult { key: u64, reclaimed: bool }`；客户端只按匹配 `key` 解锁。另一消费者滚离历史、隐藏或 detach 后，即使没有新文字或资源更新，后续准入需求仍可重新请求，不新增自动重试定时器、不循环重发，也不回收仍受保护的图片。pane 协议升至 `15`，`CONTROL_VERSION` 保持 `16`；此修复不新增原生窗口／GPU 或 macOS 验收声明。
- 修复隐藏 pane 退休 atlas 无条件刷新整窗的问题：旧 tile 与 GPU 票仍及时释放，仅实际失效的已显示 owner 触发整窗重绘；存活图片在后续 paint 重新注册，隐藏后返回保留当前像素。真实 daemon／生产 GPUI headless 回归覆盖后台窗口 render 次数、拒收后的再准入和可见删除；不将其作为原生 GPU 像素或 macOS 验收。
- 本次 pane `15` 代码通过 Windows 完整默认成员测试及 Linux 原生 headless `--all-features`；真实 Windows→Linux SSH transport 1 项、rooted GPUI consumer 2 项通过。独立私有 daemon 的 24 张 4K 混合路径 CLI／ConPTY 冒烟中，普通输入及退出正常；不将这些结果作为原生窗口、GPU 峰值或 macOS 验收。

- 修复 #42 审查发现的五项图片布局／清理边界：本地 ConPTY 的 daemon 镜像与桌面共用高度调整，远端标准 PTY 保持独立模式；部分裁切／过期图片按保留切片重排；匿名图片失去最后放置后释放容量；`DECCOLM` set／unset 同步恢复全屏滚动区域；退出／断链保留窗格的用户清理改为真正本地清理，活连接仍只走 daemon 权威路径。Windows 核心、8 项真实 daemon 布局、16 项生产 GPUI graphics consumer 与 14 项 prompt reflow 回归通过；不增加原生像素或其他平台的通过声明。
- 真实 GPUI daemon fixture 显式固定初始视口并等待有序 resize，重绘时使用实际 grid 尺寸而非旧 requested size；macOS 的生产责任归属 re-exec 标记放在 libtest 参数分隔符后，避免被当作未知选项。实际 test runner 的旧参数失败／新参数通过已验证，macOS 端到端结果仍以新的 CI 为准。
- 修正 Unicode 图片消费回归中相互冲突的尺寸来源：rooted window 已完成真实 40 列布局后，不再直接注入另一组 cell 像素尺寸，避免下一帧恢复实测尺寸时取消在途 Kitty query。保留独立 daemon／PTY、严格 `OK` 屏障、重排／共享像素及 1.5 倍几何断言；不修改生产取消语义或宣称原生 DPI 验收。
- 修正 Unix Kitty 共享内存夹具：随机名称缩至 Darwin 的 31-byte 上限内，使用 `ftruncate`／`MAP_SHARED` 初始化并覆盖非页对齐偏移，不再对不支持写入的 macOS shm FD 调用 `write`。生产读取按平台选择受限路径，失败信息保留实际 OS error；不跳过用例或放宽主机／临时删除限制。
- 修复 #44 的大接收缓冲所有权：先前文字 Snapshot／合并帧留下的超额容量不再转移进已授权的图片源，避免授权成功后静默漏图；core daemon 夹具补齐 macOS re-exec 的 libtest 参数分隔。
- 修复回滚正在查看的历史图片被误回收：pane `13` 以连接级绝对行锚点和屏幕代次报告真实视口，direct／placeholder 及跨 daemon 桌面池采用一致保护；新输出不使旧视口漂移，隐藏、detach、抢占和 observer 移除撤销对应事实。普通文字输出只更新当前 pane 的资源保护，回收优先级改用具名枚举且保留最旧顺序。新增真实 daemon／rooted GPUI 回归已在 Windows 通过，不新增原生或其他平台通过声明。
- 修复窗口先关闭时可见性注册表继续强持有图片 cache 的生命周期：registry 移除及 reader 退休改为窗口无关的 `on_release`，仅实际 atlas 清理保留在 `on_release_in`，避免失效窗口 handle 跳过回调后永久钉住 GPU 信用。真实关闭后再次准入回归已通过，保留完整像素与票据释放语义。
- 修复控制器接管后的连接退休：writer 排空后关闭双向 socket；Windows 输入 reader 使用保留半帧的有界接收检查既有 owner，避免阻塞读取钉住 Session 与 capacity lease。真实 daemon 验证旧 socket 仍存活时的最终释放、关联应答、正确像素及半个 Input header 跨期限后继续同一子进程。
- 修复普通 SSH 间接媒体的主机判断：只在媒体接受／读取点重新核验整个前景进程组，拒绝同组本地父进程后的 SSH 远端命令及未知来源；跨片段已拒权限不能因后片段回到本地而升级，后续独立本地请求仍可恢复。真实 Linux CLI／PTY／OpenSSH 同进程组冒烟已验证远端拒绝与本地恢复，不给普通文字或 direct 传输增加全进程扫描。
- 修复非活动主屏历史被全部保护而阻塞备用屏新图片的问题；daemon 与桌面共享真正实时引用，保留仍可见内容与 observer 历史视口，优先回收旧非可见历史。移除 daemon 冗余的常驻图片体副本，保留唯一 packet 与在途租约；普通输出的保留优先级更新不再逐资源反复扫描全部 placement。
- 拒绝文件／POSIX shm 短于显式声明长度的间接媒体请求，保留 offset、quiet、身份及临时文件删除边界，不以有效的短 raw 像素内容掩盖 `EBADF`。
- 修复三张合法 4K PNG 在 live 可显示、fresh replay 却因重复预留解码峰值而失败的问题：每资源保留 CPU／source／完整 GPU page 票，同 store 的 decoder 跨代次串行执行并共享最大 scratch，取消不提前返还活动票；384 MiB／1,024 MiB 上限不变。真实 daemon→fresh rooted GPUI 回归核对完整 BGRA、paint、同 pane、生产者 PID／上传数／字节数；附着失败仅在精确确认 pane 不存在时允许替代 shell。
- 修正 Kitty 续块 `q=0` 错误解除首块 quiet 的行为，仅非零 `q` 可以覆盖；真实 daemon 覆盖键顺序、成功／错误静默、非法 quiet 与后续输入。
- 外层 zlib PNG 改按实际 inflate、增长和逐行校验阶段预留工作空间，保留 parser 输入与 normalized source 的真实占账，不再因固定 100 MiB scratch 错拒合法大 PNG。真实约 30 MiB PNG 经分块／单 APC、任意 PTY 切片、完整 PNG 像素解码及坏流／超限后的恢复回归已在 Windows 通过；128 MiB 每 pane 工作上限不变。
- 修复 fresh 图片恢复在权威 `Screen` 到达前短暂使用默认 history origin 绘制的问题；首个零值 `Screen` 也会唤醒待绘制资源，重连后旧坐标代次不再可绘制。图片事件批次现在立即归还 retired atlas lease，避免跨 daemon／主备屏压力只能等待周期清理后错误拒绝合法图片。
- 修复退出／断链保留窗格本地清屏后，历史图片虽保留却无法继续重绘的问题：退休 reader 保留已知权威坐标，本地清理原子更新 history origin；新 reader 仍须等待自己的首个权威 `Screen`，不借旧坐标提前绘制。


### 维护

- 修正 #44 真实 daemon 回归的同步边界：重连接收同时处理 Snapshot 与实时 Output，等待旧请求的 PTY 屏障后再发下一条输入，避免把新命令吞入尚未完成的应答；资源释放通过真实子进程 EXIT／有序 Exited 观察，不再等待主动 kill 路径刻意抑制的退出通知。保留元数据原子拒收、迟到 grant 取消及回收顺序断言，未扩大时限或预算；修复后的 7 项保留回归已在 Windows 通过。
- server 的 pane 隔离／quiet 回归在重连时重建派生接收状态，并核验 daemon 恢复原资源身份与像素；保留单连接内重复 Resource 的唯一性断言，不把合法 replay 当作资源 key 重用，也不放宽隔离与应答语义。
- 共享真实 PTY 夹具给 `KITTY_REPLY` 文本记录附加原请求 serial，core／server 消费者只将对应记录归为本次应答；完整 Snapshot 仍作为历史消费，quiet 的空应答断言不会被旧回复文本污染，失败时保留实际 unexpected bytes。
- 共享 Rust／Node PTY 夹具统一使用带非数字终止符的 `KITTY_DONE_<serial>;`，避免覆写旧文字行后将 serial 2 残留成 serial 20、提前注入下一条命令；不清行或改变图片生命周期。真实超过 8 MiB 重放尾部的 fresh GPUI 图片／文字投影回归已在 Windows 通过；废弃的测试专用输出解析副本改为生产 `graphics_reader::consume` 的任意 split 字节透明性回归。
- 备用屏图片压力回归显式请求窗口帧并推进现有 GPUI 任务与纹理 compaction 时钟，等待真实 decoder 结果，不以只读 `sleep` 阻塞后续完成；成功路径另要求真实关联 `OK`，避免将准入拒绝误报为解码超时。两页缓存、30 秒期限、direct／placeholder、live 像素及历史 notice 断言保持；quiet 的独立回归继续执行，平台结果以对应 CI 为准。
- 跨 daemon 压力回归在进入／离开历史视口后，先通过原 daemon 的真实 PTY 屏障确认视口更新，再让另一 daemon 施压；等待准入期间刷新旧 atlas 所属窗口和新图窗口，保留原 1 秒准入窗口及历史保护、权威回收、拒替换与关闭后释放断言。
- 真实 resize 回归复用首次挂载前已按实际字体设为 80×24 的共享工厂，避免默认窗口首绘向 ConPTY 发送意外 resize；正式收缩前核对 cursor、anchor 与 `row=23` 的 `LAST_WRITTEN`，保留 Windows 收缩后锚点 `row=3`／尾文 `row=17`（均从零起算）及 macOS 的原几何预期。Windows 7项真实布局回归与完整默认成员检查通过，macOS 完整结果以对应 CI 为准。
- 真实图片夹具的 PTY、decoder 与恢复等待统一推进所有测试窗口的实际帧、执行器和时钟，使原窗口 atlas 可在既有准入期限内释放；保留接收 Size 前禁止旧 surface 回写的边界，不调整预算、quiet 或等待期限。布局夹具复用预挂载工厂，字体裁切回归通过真实字体／窗口及测得的设备像素构造五行自然图片，不再把仅注入后端的虚假 cell 尺寸当作实际呈现。
- 光标不变与真实 ConPTY resize 前提等待完成标记之后的实际光标恢复，避免将暂时位于诊断文字行的 cursor 当作命令最终状态；保持原光标坐标、最后写入行和几何断言，不重新提交被拒的请求。图片等待失败保留现有资源／放置诊断，真实发送器退出保留原始拒收输出。
- rooted GPUI 图片夹具在 PTY 等待期间推进真实 UI 任务及测试时钟，使已绘制纹理经现有窗口 compaction 归还 GPU 票；历史 notice 回归通过生产滚动入口同步视口并核验 placeholder notice 的实际 paint。大文字恢复用合法的小于 1 MiB 的 Input 分批构造原总量 Snapshot，不放宽输入、图片预算或等待期限；修复后的 23 项 graphics consumer 回归已在 Windows 通过。
- GPUI 回归先同步 rooted 窗口的实际列数再修改终端尺寸，避免推进 UI 时由旧窗口布局回写覆盖 reflow；Screen／原点回归等待其后有序文本 marker 与实际解码状态，不以一张图片已出现替代 Screen 已消费。
- 原生 SSH 夹具的请求 serial 跨 fresh consumer 不复用，避免旧 Snapshot 回复被误归为当前请求；恢复等待唯一文本屏障及权威最终 `Screen`，不在先到的 Snapshot 上提前返回。GUI 从不接管／不 resize 的真实 Observe raw Output 核对应用收到的全部关联 ACK，保留有限预算、像素与 placement 断言，不依赖终端网格保存已被覆盖的诊断文字。
- fresh GPUI 恢复夹具通过公开 deactivate 完成 headless active-window／input-handler owner 释放，再正常销毁实体、图片 store 和 texture；首帧先测实际字体并设置物理尺寸，再挂载终端，避免默认最大化首绘改写存活备用屏。保留三重弱引用、真实上传 ACK、权威 observer 顺序、像素与 resize 断言；不以测试平台生命周期问题掩盖丢图。
- 真实 daemon resize 回归先按实际字体计算目标窗口尺寸，等待权威 `Size` 时只读 reader 共享网格，不推进仍为 24 行的旧测试窗口；消费后先同步物理窗口再走 PTY 屏障，避免旧 surface 回写尺寸并扰乱历史锚点。保留 ConPTY／标准 PTY 的不同预期及最后写入行断言；Windows 7 项真实 daemon／rooted paint 回归通过，macOS 完整结果以匹配提交的 CI 为准，不以 Windows 代替。
- 桌面测试明确声明 `base64` dev-dependency，修复 macOS 仅因 Windows 生产依赖可见性而编译失败；移除固定内部三帧排列的旧 resize wiring 测试，真实 daemon／GPUI 尺寸与图片顺序回归继续执行，不把新 `Screen` 元数据当作异常。
- Darwin 共享内存回归按 `fstat` 返回的页对齐实际长度映射，并将完整像素放在对象末尾；保留 `S=0` 的实际读取及区域越界拒绝断言，非法请求偏移不再用于夹具自身写入。fresh GPUI 恢复夹具先等待唯一 replay／live 文字屏障再断言生产绘制，避免资源已解码、权威 `Screen` 尚未到达时提前验图；不放宽 fence、预算或绘制／几何断言。macOS 与完整套件结果仍以匹配提交的实际 CI 为准。
- x86_64 musl 的 CI 与 Release C++ 构建统一使用上游 `SIMDUTF_IMPLEMENTATION_ICELAKE=0` 配置，避开正式 `simdutf 0.7.0` 可选 Ice Lake backend 的 `evex512` 编译错误；保留 Zig `0.16.0` 与 AVX2／SSE／scalar 运行时分派，不修改第三方源码或提高远端 CPU 要求。

- 工作台标题栏的文档输入回归在操作前有界等待真实 Host 文件加载完成，避免将 GPUI 队列空闲误判为 I/O 已完成而在 macOS CI 偶发失败；保留侧栏切换、文档／终端焦点及按键隔离断言，不修改生产行为。
- 主机指标的连接替换回归使用固定合法快照验证 daemon identity 与错误优先级，避免准备阶段被无关的实时 OS 采样超时打断；真实采样、断线恢复和远端 Host 隔离仍由既有 UI 集成用例覆盖，生产采样与请求期限不变。

## [0.2.0] - 2026-10-24

### 新增

- 新增标签页内可交互的画中画（PiP），支持在内容区移动、调整大小及恢复原分屏关系；复用原终端与会话，不创建第二个 PTY，单窗格也可进入画中画。
- 新增窗格停放牌堆，可从菜单或拖动手柄到底部停放运行中的平铺／PiP 窗格，再逐个预览和恢复；窄窗口及大量卡片提供“更多…”列表与键盘导航。停放不关闭或休眠会话，也不撤销期间的其他布局修改。
- 全局状态栏新增当前工作区所属整台主机的真实 CPU／物理内存指标；本地和远端均由所属 daemon 采样，等待、断开及失败明确显示，不以本机数据或零值冒充远端结果。
- 新增应用级通知收件箱，在本机保留最新 500 条通知及未读状态；多个主窗口和设置窗口共用历史，可跳转到仍存在的来源，停放来源仅打开预览，不自动恢复布局。

### 变更

- 重构原生工作台：标签导航统一位于左侧，窗口品牌、常驻功能按钮和原生窗口控制独立保留；终端与文档共用全局状态栏，文档行列、问题、编码、换行符等操作不再占用第二条底栏。
- 移除顶部横向标签布局及悬停显示标题栏按钮两项设置，仅清理对应失效配置键；侧栏共享原生窗口材质，收起侧栏或缩小窗口时保留全局操作入口。
- PiP、停放与恢复关系由所属 daemon 持久化并广播，焦点及预览仍属于本地窗口；原生 SSH 与远端窗格保留各自所有权，CLI 继续使用原 pane ID 访问会话，内部组合布局不出现在工作区导航中。
- control 协议由 15 升至 16，pane 协议仍为 7。旧 daemon／远端 server 必须更新后才能连接；重启 server 会结束该机器上的活动会话，升级前请保存工作。

### 修复

- 修复 shell 未报告命令完成标记时，在已识别的父仓库子目录中执行 `git init`／clone 后，“更改”仍指向父仓库的问题。可见窗格按原始 cwd 合并低频轻量重查，仅仓库位置变化才重新读取完整状态；路径发现统一物理身份，失败保留已有结果，过期查询不覆盖新状态。
- 修复仅打开 GitHub 页时，新增、改址或删除 remote 不使缓存失效的问题。复用仓库元数据监视和去抖通知，区分元数据订阅与完整状态扫描，补充无 remote 空状态的刷新入口，并拒绝已过期的 remote 查询结果。omp、VT、ConPTY 与终端文本渲染保持不变。

### 维护

- 同步 `aes-gcm 0.11.1` 的 manifest 与 lockfile；发布工作流升级至 `actions/github-script@v9`，保留草稿、资产及 SHA-256 校验流程。

## [0.1.3] - 2026-10-22

### 修复

- 发布草稿创建先将远端版本标签（包括附注标签）解析为真实提交 SHA，再作为 GitHub API 的 `target_commitish`，避免将标签名传入而反复触发 HTTP 422；非法目标在创建草稿或上传资产前拒绝。保留无证书的 Windows 未签名／macOS ad-hoc 路径和既有资产校验。
- 修复启动时读取 Hermes 历史或统计，因 Homebrew 入口不支持源码安装专用 `--print-runtime-command` 参数而报错的问题。POSIX 单行 env/exec wrapper 使用其显式 `HERMES_PYTHON` 运行时，保留虚拟环境路径；源码安装与 Windows 继续使用公开运行时查询，不猜测系统 Python，也不将失败伪装为空历史。
- 修复 macOS 自动更新无法识别 ad-hoc 的 `# designated =>` 输出，以及正常跨版本 `cdhash` 变化导致校验失败的问题；保留归档 SHA-256、版本与完整签名验证，ad-hoc 仅接受相同签名 Identifier，证书签名由 `codesign` 求值当前 requirement 并拒绝降级。
- 修复 macOS 启动查询 OpenCode 历史时，正式 CLI 的大 JSON 响应在 stdout pipe 中被截断、却以成功状态退出而报错的问题。改用私有匿名临时文件完整接收，保留期限、大小上限与原生错误，不重试或过滤 JSON。

### 维护

- 终端重放回归在写入标记前等待 daemon 的尺寸确认，避免并发附着时把尚未处理的 resize 误判为重放丢失；保留原有屏幕、几何和控制权断言，不修改生产行为。

## [0.1.2] - 2026-10-21

### 修复

- 统计 Token 卡片和热力图提示移除 `≥` 前缀，热力图 Token 使用百万单位 `M`，费用卡片使用 `$` 金额格式，提示文案改为「花费$…」；保留原始统计数值、USD 口径及「已知小计」「数据不完整」提示。
- Command Code 的 5 小时、每周和每月额度右侧仅显示剩余百分比，不再附加剩余 Credit 数量；下方保留已用、上限、已用百分比和重置时间，缺少有效比例时显示未知。

### 维护

- 清理桌面端无调用方的终端与工作台包装、重复 Git 状态排序、只写不读的设置字段及未使用翻译；测试专用构造器和缓存查询仅在测试构建中保留。移除终端、Host 与 SCM 相关模块的整模块 `dead_code` 屏蔽，保留实际使用的 SSH／SFTP 实现、平台分支和 IPC 协议，并同步错误通知的开发约定。
- CI 合并前置格式／Host／自动化检查，按完整 push 或 PR 合并差异裁剪无关构建；文档跳过覆盖 main，Cargo 输入、嵌入 LICENSE 和不确定范围保守全量验证。发布流程变化共用只读的真实构建／打包工作流，不继承签名凭据、不创建发布；正式 Release 保留独立草稿写权限。新增同 SHA failed-only 恢复入口，以及同源可信 main CI 的独立 Unix server 产物复用，严格核验身份、摘要与构建契约，不合格即正常重编，保留各平台测试与二进制断言。
- 修复远端 CI 暴露的两处问题：server 产物来源记录改用 `cargo-zigbuild --version` 查询工具版本；SCM 提交详情空闲测试复用既有静默屏障，等待真实 Git／文件监视回调完成后再严格检查零重绘。

## [0.1.1] - 2026-10-21

### 修复

- 修复 omp TUI 执行 `/resume` 恢复包含原始 `OSC 133` 输出标记的历史会话后，侧栏 agent 图标消失的问题。shell 注入的命令边界使用 `aid=lyrel` 归属属性，普通 TUI 输出块不再覆盖 agent 识别依据；真实退出仍清除身份，CLI 保留退出码。
- 随处搜索的搜索行与底部导航行统一为 40px，Esc 提示由输入行布局垂直居中；保留筛选、清空、上下选择、Tab 切换及子列表返回。
- Command Code 用量仅保留订阅的 5 小时、每周、每月三项额度，展示真实余量、限额、百分比和重置时间；移除账单、请求数、购买／赠送 Credit、Sandbox 与组织模型列表，不再请求无关的 usage summary。旧快照收敛到三项额度，保留账户及凭据引用；未知套餐不伪造月限额。
- 修复新建或克隆仓库后“更改”与 GitHub 面板需重启才能识别的问题：以提示符周期捕获两次前台采样之间完成的短命令，右侧跟随最新工作树根目录，GitHub remote 列表随仓库变更失效；覆盖非仓库转仓库、嵌套仓库及后续添加 `origin`。

## [0.1.0] - 2026-10-20

### lyrel 当前变更

- 在现有 `LICENSE` 集中说明 tty7 来源并汇总必要的第三方版权与许可，仅为本次实质修改的文件保留修改声明，不批量添加源码文件头；macOS 包随附 `LICENSE.txt`，裸 server 内嵌同一份许可并通过 `--license` 提供，远端自动安装沿用同一二进制。
- 启用 `.github/workflows/` 中的 CI 与 stable 发布管线，同步迁移打包、平台断言、Dependabot 和 issue 模板；显式准备 Node.js 测试运行时与官方 ConPTY，移除旧的 server 缺失跳过。发行汇总全部八件平台资产与 SHA-256 清单，先创建草稿再审核发布，重跑不得覆盖已发布资产；macOS ad-hoc／未公证和 Windows 未签名限制保持明确。
- CI 与发行改用 Node.js 24 的 `actions/checkout@v7`，移除仍依赖 Node.js 20 的 Zig setup action，改用固定 `ziglang 0.16.0` 的官方 PyPI 分发；Linux runner 固定为 Ubuntu 24.04，避免 `ubuntu-latest` 迁移时改变构建环境。修正平台专属测试 helper 的条件编译和 GPUI 测试绘制后的 arena 清理；拖动测试验证真实手势状态与原生移动请求次数，不再依赖 headless 平台的 `unimplemented!` panic。
- CI 与 Release 的桌面构建统一 updater feature 图，避免重复编译；发行桌面与 server 并行，仅打包等待 WSL server，并保留 Thin LTO、完整测试、ConPTY 和平台校验。缓存按 CI／发行分区，仅 main 保存供标签复用，已压缩包不再做外层压缩。草稿汇总直接使用创建响应的 release ID，支持严格核对仓库、源提交、成功构建与未过期完整 artifacts 后恢复；相同 SHA-256 的草稿资产可复用，不重编译、不覆盖已发布版本。
- Windows 中间归档及恢复改用相对路径，避免 GNU tar 把盘符识别为远程主机；已用真实发行二进制验证归档恢复与原打包／完整性检查。Windows／macOS 编译和打包分别建模，macOS runner 暂时无容量时，不再连带阻塞 Windows 打包；正式汇总仍要求全部平台成功。
- 统计扫描按所属 Host 解析已选文件来源根，使用同作用域的已确认根别名匹配 Pi 父会话，而非仅处理 Windows 扩展前缀；历史记录中的父路径不触发 I/O。渐进扫描保留已解析的来源描述，不再被原始路径覆盖。原入口消失时只保留受限旧事实，不继续跟随旧目标读取新文件，保持分叉去重、身份冲突、缺父与可读智能体日计量口径。真实 Bash PTY 测试改为正常退出并完整释放 console、writer 与 reader 后删除临时目录；macOS 恢复默认 unwind 链接选项，移除禁用展开元数据的覆盖。
- 修复无显式认领的 pane 创建竞态：发送 `Spawned` 成功回复前完成注册，确保调用方能立即通过另一连接发送输入；显式认领仍等待同连接的 `Attach`，失败或断连的临时 pane 仍按原规则回收，不新增重试或 IPC 消息。
- 技能安装、移除及其 UI 测试统一使用规范化的真实临时目录，避免 macOS 的 `/var` 符号链接触发本应保留的路径防护；生产链接拒绝规则、安装确认和祖先项目权限断言保持不变。
- OpenCode 的受控客户端测试在 `accept` 后显式恢复阻塞模式，避免继承 listener 的非阻塞状态而提前断连；认证、重放防护、原生身份和创建后保留断言不变。

- 统计活动悬浮提示收敛为 Token 总量、等价 USD、会话数，删除图下日期、连续天数、峰值与日详情长文案；已知正 USD 费用在其他来源失败或部分未计价时仍按 `≥` 下界着色，不换算其他币种、不分摊未知日期或身份冲突记录。统计快照与解析缓存改为进程内共享，支持 60 秒复用、过期先显示旧值、跨窗口／重开单飞和主窗口打开后延迟 5 秒的本机后台预热；手动刷新仍重扫，日期筛选不因缓存过期触发扫描，来源／单价／配置文件变化与 Host 重连隔离失效，不新增远端预扫、账户请求或公共价表下载。
- 继续精简技能与统计设置：技能元数据统一左对齐，删除调用、来源引用与来源记录；统计仅保留日期范围菜单，删除重复的今天／本周快捷按钮。本地历史先展示，Hermes／OpenCode 并行补充并保留未完成标记；日期与分析筛选复用只读快照后台重算，扫描中切换不重复排队。JSONL 热命中减少一次计量事实深复制及缓存写回，仍检查实际文件变化、删除和父子目录关系。
- 精简技能、统计与用量设置页：技能详情移除重复安装路径和「发现位置与原生状态」长说明；统计增加扫描动画与紧凑状态，将 OpenCode 独立会话改为有界并发读取，避免串行原生 CLI 请求耗尽扫描期限，缺失 Token 不再抹去已知小计，仍以 `≥` 区分不完整总量并排除身份冲突；用量页删除重复引导与认证方式说明，将账户范围提示放入添加表单，并精简服务商接口文案。保留原生身份校验、分页去重、扫描上限及凭据与权限边界。
- 修复空工作台仍绘制旧 ASCII 品牌的问题，关闭最后标签页后统一显示 `lyrel`。设置三页继续按 vortex 对齐：技能改为紧凑机器／项目工具栏和双栏列表详情，拆分 renderer 避免 Windows Debug 主线程栈溢出；统计区分最近 7 天与本周，加载时展示实际日期而非重复周期名，恢复快照观察时间，热力图以描边区分未知并仅显示可证明的正活动下界，身份冲突不着色；星期列按文字测宽并固定在滚动区域外，水平滚动条使用独立轨道；活动、日历与明细分区渲染，修复真实历史载入后的 Windows Debug 主线程栈溢出，不提高全局栈上限；费用分解保留同币种明确估算。用量改为单列紧凑账户与明确来源、空状态及原生操作菜单。OpenCode 原生查询按同一服务公开 schema 协商早期／当前 2.x 接口并保留进程身份守卫；Windows 浏览器测试导入与平台专用项按实际目标隔离。Hermes 网关响应、原生关闭和进程退出共用同一操作期限，避免有效结果被独立的 5 秒退出期限误判失败，真正超时及非零退出仍报错；Windows 当前安装的 launcher 已实际验证原生历史、统计传输与聚合，并验证默认命令与明确路径入口。桌面在 Windows 本机验收，另以 Ubuntu 26.04 原生回归验证统计、OpenCode 及 Hermes 共享契约；真实 SSH 与其他平台的原生运行继续按独立证据验收。

- 设置的技能、统计与用量页以 vortex 设计图为构图基准，仅适配 lyrel 语义主题、字体、本地化及必要的机器范围与窄窗交互：技能恢复紧凑双栏、元数据先于动作与 Markdown 正文；统计恢复三指标卡、真实组成条／日趋势、年度活动区、双栏明细及两张管理入口卡；用量恢复服务商分组、紧凑浅底双列账户卡与原生更多菜单。保留 Host、计量、凭据及权限边界；修复弹层搜索穿透、菜单返回焦点、热力图压缩与键盘轴向、指标切换后的明细主值、命名额度池类别及窄窗工具栏越界。Windows 隔离桌面已实际对照设计图验证中英、深浅、宽窄和 14／18pt；真实服务商账户、受保护浏览器权限及 macOS 仍按独立证据保留未验收。
- 修复设置集成最终审查的八项问题：Host 检查按实际测试模块及传递子模块排除，不截断父文件生产后缀，仅为三个已证明本机路径增加精确豁免；Chromium LevelDB 恢复增加文件数、单文件与总字节预算；替换来源同时原子保存编辑名称；共享根内精确别名清理区分自身目录变化与外部变更；技能／统计新增主控件复用原生键盘菜单与按钮；默认来源发现失败保留同作用域的原时间与不完整统计；Cookie 导入及增强在真正系统权限边界核对可撤销表单代次；祖先项目技能绑定其实际项目与来源锁，维护前要求明确切换所属项目。确定性 GPUI、真实隔离文件／junction 和缓存失败冒烟分别记录，真实账户与 macOS 权限不由受控测试代替。
- 统计补全五种来源的审查、添加、检查和只移除关联，按所属 Host 原生目录／文件／服务／profile 重建并拒绝重复重叠，不提供 OpenCode／Hermes 旧数据库入口。自定义每百万 Token 单价、精确服务商／模型／币种／类别匹配与明确 opt-in 的 models.dev 公共目录共用总览、热力图和明细计价；原生零费用优先、原生估算独立、部分缺价与跨币种保持受限，累计不摊日。来源和价格配置、目录缓存及时间本地原子持久化，默认零联网，检查失败保留旧缓存；真实桌面、SSH 和跨平台验收按独立证据报告。
- 统计增加桌面时区年度活动热力图、Token／记录费用／主会话指标、真实来源智能体筛选、键盘日期详情与连续使用，以及同范围智能体／模型／项目分页明细。总览与分析共用唯一去重及主会话活动流，按智能体与有效作用域隔离；未知、未计价、估算、累计跨日和缺少日期分别保留，不伪造日格、排名、调用次数或连续天数。Windows GPUI 原生键盘与双语／宽窄／字号工作流、Windows→Linux 隔离 SSH Host 路径分别验收；真实桌面视觉与 macOS 不据自动化测试宣称通过。
- 统计接入 Hermes 0.22.2 正式 launcher／公开网关，复用所选 Host 的有效 home/profile，以独立 `hermes-statistics-v1` 协商；实际费用与原生估算分别保留，累计用量不摊日，子代理缺口、无 offset 的窗口和不可判定空响应明确受限，失败保留带时间的旧值，统计传输剔除 `title`／`preview`。Windows 正式 runtime 的 default／work、主／分叉／子代理及 502 条正式 SessionDB 隔离记录窗口已实际验证；Linux headless server 已构建，Hermes 真实 SSH 因正式依赖不可取得未验收，macOS 与真实桌面矩阵按独立证据验收。
- 统计加入独立 pi 1.3.0 本机／远端 JSONL 适配及智能体来源筛选；遵循原生存储选择与全会话 usage-totals 口径，保留零费用、未知字段及可核实分叉新增用量，缺父链和冲突显式受限。与 omp 的配置、身份、缓存和事件解析保持隔离；真实桌面、SSH 与模型账户证据分别验收。
- 设置新增只读技能页：明确选择本机／已连接远端和绝对项目路径，按五种智能体有效配置根及 profile 发现共享、私有与已安装包技能；按真实安装归并链接来源而不合并同名副本。支持名称／描述搜索、来源筛选、实际 Markdown 详情、只读来源锁识别及绑定 Host 的文件入口；代次与设置会话身份拒绝晚到结果，读取失败、旧快照、未知原生状态及扫描上限明确可见。不修改智能体配置或投递终端命令；Windows 文件／junction／GPUI 与 Linux 真实 stdio、相对链接和权限验证分别记录，真实 SSH、macOS 和桌面视觉验收另列。
- 技能页新增 GitHub repo／ref／subpath 多候选发现和 skills.sh 搜索；用户明确选择本机／远端用户／项目共享目标、审查具体目录与 CLI 来源锁后安装，同名默认保留并逐项确认替换。GitHub 获取始终由桌面复用既有环境／gh 授权，凭据不下发；授权拒绝、权限、不可见来源、限流和网络失败分别显示。下载采用固定 SHA、有界 tree／blob，拒绝不安全路径与链接；Host 写入按目标平台解释路径，审查锁定机器与 home／项目，保留未知锁字段与无关内容，取消零写，失败及部分结果逐项可见。不创建私有链接、不改原生配置或运行中 TUI；真实私有来源需明确授权仓库另行验收。
- 技能详情新增有来源安装的受保护更新与共享安装安全移除：区分可用更新、无更新、上游消失、失败及未知来源；本地改动或未知基线默认保留，明确逐项替换而不合并。移除审查列出具体共享目录、来源锁与精确关联 symbolic link／junction，执行前复核固定 Host、路径与目标，保留真实副本、其他目标及无法证明一致的条目。取消零写，失败／部分结果分别显示；来源锁保留合法未知字段与无关项。新增最小只读 Host readlink 能力，control 15、pane 7，旧远端明确升级且不回退本机。
- Windows 目录枚举读取 `DirEntry` 缓存的原始 reparse 属性，保留 junction 识别与安全 unlink，不再为每个目录项额外执行 `symlink_metadata`，避免技能维护反复校核大型祖先目录时放大磁盘调用。
- 设置新增用量页：DeepSeek 官方余额、本机全局多账户、验证后写入系统凭据库、分离凭据槽的安全替换与本地移除；手动／全部刷新与仅页面可见期间的可选周期查询共用路径，旧快照和分类错误可见，稳定身份及操作代次拒绝晚到旧结果。新增明确区分的私有平台手工 userToken／Harness grant 与主动选择浏览器 profile 的 LocalStorage 导入：Chromium LevelDB 和 Firefox SQLite 真实只读，材料仅内存、不后台申请权限、不静默换来源；平台充值／赠送钱包分开保留原单位。网页 Bearer 尚缺第一方和真实账户核验；其他服务商、Cookie 导入及真实浏览器／平台矩阵验收另列。
- 用量页增加 StepFun Plan／Xiaomi MiMo 控制台手工会话及主动选择浏览器 profile 的 Cookie 查询，复用验证后安全保存、替换保护和页面可见刷新。MiMo 按第一方 SPA 的 balance／userProfile／usage.items 与 UTC 到期字段呈现；StepFun 套餐家族与额度池保留各自单位。Firefox 真实只读 SQLite，Windows ABE 明确拒绝，macOS Chromium 使用官方 Keychain 且后台 UISkip；浏览器材料仅内存、受信目标固定且禁止重定向。真实服务商账户、浏览器权限与 macOS 实机矩阵另列未验收。
- Command Code 增加主动选择浏览器与 profile 的会话增强闭环，先匹配 API-key 主要身份再读取订阅；浏览器只保存非秘密绑定，不保存 Cookie，不因增强失败覆盖主要额度。补充 Windows 传统 DPAPI／AES-GCM 与域名校验，保留 ABE 拒绝；macOS Safari binarycookies 尽力读取并明确全盘访问限制，后台不读取 Safari 或弹系统授权。取消、删除、来源替换和关闭后的晚到材料不能重绑账户。真实浏览器／服务商账户与 macOS 实机权限仍单列未验收。
- 项目交付文件中的产品标识、内部样式、示例路径与图标打包输入统一清理；外部产品隔离 fixture 使用中性命名，保留不误连接、不认领及不删除其他资源的行为覆盖。原始版权、Apache-2.0 与第三方资源许可保持不变。
- 移除继承的发行历史、性能成绩与过期截图／视频，保留可复现测量工具及方法，不将历史成绩改称 lyrel 实测。
- 将产品术语、依赖政策、平台取舍、终端容量限制及智能体验收缺口收敛到现有正式文档，移除独立决策、术语与临时工单材料，并同步维护规则及文档路由。
- 修复 Windows pane 清理误杀 detached 共享服务：以 ConPTY 的实际控制台归属挂断附着进程，不再按历史 ParentPID 终止整个后代树；真实 OpenCode 2.0.32 重连后，关闭拉起服务的父 TUI 或 pane 均保留共享服务。
- 本机 GUI shell 创建迁至后台，通过 `spawn-claim` 与匹配 ID 的显式认领处理晚到结果和取消；未认领 pane 不进入持久 registry／handoff。关闭窗口／slot 后准确回收新建结果，恢复的持久 pane 不被误杀。Hermes 操作先公开 `session.close` 再关闭 stdio，保留真实目标与部分成功；headless OpenCode URL 校验直接使用锁图既有 `http` parser，不再误依赖可选 installer feature。
- 修复真实运行暴露的三处问题：设置原生窗口标题随界面语言同步；OMP standalone扩展使用正式`@oh-my-pi/pi-utils`根包导出；OpenCode TUI桥直接消费SDK已解包的返回值，避免再次读取`.data`导致身份／会话查询失效。
- 原生 Windows、Linux headless、桌面语言与重启，以及 Windows→Linux SSH／Host／SFTP／PTY 的验证范围分别记录；Hermes 即时身份、Kimi 自动首任务及完整模型／平台矩阵仍有缺口，不据套件通过推定全部能力已验收。
- 修复双轴审查发现的八项可达问题：pi 集成管理复用已确认的原生开关契约，支持 `--offline`／`-ne`；工作树首任务按目标 Host 默认 shell 渲染；pi／Kimi 历史与管理及恢复共用有效配置作用域；集成常规诊断改为类型化数据并提供中英文界面；桌面通知及输入框显示真实 outcome／reason，`Done` 保持中性结束含义；所有实际结果载荷协商 `agent-results`；远端已创建分叉的连接失败保留并显示真实新 ID，重试只打开原目标；首页只列五种唯一产品并移除旧支持数量和失效演示名称。正式 GUI／平台验收须分别记录。
- Hermes 11 在同一 opt-in observer 中接入原生 UUID 人工输入、按 turn 关联的最终 outcome、压缩身份更新与异步子代理续跑门控，复用通用请求集合而不新增超时状态机；配置显式 profile 与 Host home 范围参与 hooks／历史／恢复。历史经 Host 的正式 `projects.tree` 只返回元数据，远端独立能力协商；501 条探测使 500 条原生窗口溢出明确报错，不静默截断。首任务采用真实 TTY 下保活的 `chat --query=<literal seed>`，保留多行／中文／连字符与原生参数，拒绝 one-shot 和已有 query 冲突。上游早退绕过 observer、完整后台进程最终态、即时裸会话身份、历史分页以及真实 GUI／PTY／provider 和跨平台验收仍有明确缺口，
- pi 独立适配正式 `@earendil-works/pi-coding-agent@1.3.0`：统一 Host 操作通过原生外部 `--fork`／RPC 核定真实新 ID 后打开原生 TUI，保留 session-dir、配置根、模型和自定义扩展参数，清除互斥旧会话选择；精确源缺失不新建空会话，开窗失败保留目标。扩展仅以 `agent_settled` 结束，按共享请求／结果契约保留未决输入和失败／中断，不使用首轮 `agent_end` 回退；增加原生交互首条任务、模式诊断、`PI_CODING_AGENT_SESSION_DIR`／`settings.sessionDir` 平铺历史和正式 `summary` 字段。未取得完整模型、GUI／daemon／PTY、远端、活跃写入／并发运行证据，不宣称全面验收。
- omp 固定正式 `18.12.1`，使用独立扩展按 `willContinue`／`awaitingAsyncWork` 判定最终结束，覆盖工具审批、计划审阅、提问与主会话切换；通用状态机按稳定请求 ID 保留并解决未决集合，权威快照可恢复状态，不因一个工具完成清空其他等待。删除五种智能体的静默／键入超时假 Idle／Done，运行状态与失败／中断等结果分开；旧 daemon 通过 `agent-requests` 能力明确拒绝不适配的状态桥。OMP 工作树初始任务使用原生 `launch --` 安全边界；有效 profile／home／XDG 与已捕获 session-dir 参与安装、历史及统一 Host 会话操作，安装不改用户禁用策略或自有内容。真实 GUI／PTY、模型流程与跨平台未据源码或事件样本宣称通过。
- Hermes 按正式 `0.22.2` 增加 opt-in Python observer 最小身份桥与自有插件文件管理；通过官方 source-install launcher 的公开 `--print-runtime-command --module tui_gateway.entry` 取得准确 managed Python/bootstrap argv，直接启动 stdio 网关，避免 Windows `.cmd` 超时遗留后代。统一 Host 操作以 `session.branch_stored` 返回持久化新 ID、以原生 chat 恢复；错误不退化为空会话，已知身份后的部分成功保留真实目标。不采用只支持 ACP 来源的 fork，不向原 pane 输入 `/branch`，不修改内部会话文件或绕过插件授权。正式 GUI／daemon／PTY 双窗格、活跃写入及即时身份覆盖仍未验收，
- 内置智能体身份收敛为 omp、Kimi Code、pi、OpenCode 2.x、Hermes；移除其他产品的专属识别、入口、hooks、历史、恢复／分叉分支、composer 适配、图标与退役专属测试。旧配置和快捷键不再生效，普通 shell 不受执行白名单限制；未知继承身份仅失去专属标注，不丢弃整份 pane／machine／handoff 状态。移除 Claude 专属折行复制与 CopyRaw 入口，保留普通复制；提供按 lyrel 归属核对的人工旧 hooks 清理说明，不扫描清理用户目录或终止已有进程。新增行为回归待集成后统一运行，正式智能体、GUI 与真实旧 daemon 升级／handoff 尚未验收，不据静态改动宣称通过。
- 恢复／分叉入口统一经共享核心 Host 会话操作与既有后台调度处理，删除失败后静默新建空会话的回退；omp 按正式 `18.12.1` 原生恢复／分叉及持久化身份校核返回真实目标。新增 Host 归属、目录与身份错误、部分成功保留和远端能力协商回归；窗格打开失败可恢复同一目标，不重复分叉。真实正式智能体、GUI／daemon／PTY 与平台验收须分别记录，不因代码落地视为通过。
- OpenCode 桥接迁移到正式 `@opencode/cli@2.0.32` 的客户端本地 TUI 插件：以前台 route 和本客户端 tabs/family 绑定身份并聚合 execution、permission/form、outcome；重连及定期权威快照不把丢事件或关闭客户端当作成功。新增通用显式执行结果与 `agent-results` 能力，`Done` 改为中性结束标记。支持 `opencode2` 和原生 `--prompt`，保留默认共享服务，授权更新仅迁移 lyrel 自有旧桥，禁用项不自动解禁。代码回归与正式智能体／GUI 实测分开记录；未取得完整运行证据前不宣称全面支持。
- OpenCode 2.x 历史／恢复改用正式 `api`／已连接 TUI client 的 `session.list/get`；实时分叉经每个客户端独立的私有认证通道调用原生 `session.fork`，返回真实新 ID 后核验连接再打开，不再拼根 `--fork`。历史保留来源作用域，过滤子会话和归档，错误与部分成功保留真实身份；修正正式 SDK `data` envelope 的状态读取。五种智能体的稳定 `SessionScope` 统一保存到 daemon／窗格恢复模型，瞬态桥与认证 nonce 不写持久化。正式共享服务／桌面矩阵、standalone 第二 TUI 认证交接及服务存储来源证明须分别记录，不能以接口实现冒充运行验收。
- OpenCode客户端桥安全修复：严格v2以标准HMAC-SHA256和一次性双方challenge认证完整请求／响应，不再把原始capability发给可能被接管的loopback端口；accept绝对认证期限与独立未认证容量避免滴流占尽操作slot。原生fork后再次正式get核实新目标，失败保留已知ID；默认历史作用域优先级修正为绑定scope、显式启动环境、Host缺省，并在合成后检查存储限制。专项消费者回归已补，真实运行验收须分别记录。
- Kimi 初始任务不再被静默丢弃：GUI 与 CLI 在创建工作树／窗格前传播明确错误，空任务仍保留自定义原生启动配置。隔离正式 `@moonshot-ai/kimi-code@2.2.0` 已实跑 CLI、ACP、KAP 和 `SessionStart` hook 探针；KAP 能接收任务，但原生 TUI 投递／就绪闭环尚未验证，模型授权也缺失，不能据此声明此功能完成。
- Kimi Code固定正式2.2.0：补权限回复／提问／后台任务契约，主会话只读durable turn记录提供真实结果，不再把可阻断Stop或普通工具失败当作最终成功。有效Host环境采用非凭据白名单，远端home不借用本机环境；历史与恢复固定同一来源。原生fork取得新ID后严格校核stdout和真实目标，再恢复新窗格，部分成功保留ID。隔离正式CLI原生分叉已实跑；模型、GUI／PTY双端继续、并发及远端完整验收仍未通过，初始任务缺口独立保留。
- 界面翻译收敛为英文与简体中文，默认跟随操作系统最高优先界面语言，并允许固定语言；选择模式与实际语言分离，明确脚本优先于地区，模糊中文、繁体及读取失败使用英文。旧日语、俄语与无效配置回退英文而保留其他设置，仅加载不强制改写；删除日俄翻译资源与专属复数／搜索入口。设置、菜单、托盘及已有 Explorer 标签随选择刷新，不自动新增注册；CLI 与 shell 字符环境不变。行为回归已补充，真实窗口及平台验收由集成阶段分别记录。
- 更新 Cargo 直接依赖约束与正式锁文件，并适配 `russh 0.64.1`／`russh-sftp 3.0.1`、`ureq 3.4.2`、`rusqlite 0.40.2`、`portable-pty 0.9.0`、`gix-discover 0.57.0`、`miniz_oxide 0.9.1`、`sha2 0.11.1`、`resvg 0.48.1` 等发布版 API；保留 registry-only、Kit/pre 与 Rio 边界。`tree-sitter` 保持 Kit 依赖图可兼容的 `0.26.13`，不通过 fork／patch 绕过原生 `links` 冲突。
- GUI 主题与共享 agent 历史的 YAML 直接调用统一切到已批准的 `yaml_serde 0.10.7`，不保留 `serde_yaml` 兼容别名；updater 的 ZIP 读取迁到受支持的 `async_zip::base::read1`，保留路径、符号链接、展开大小与 CRC 校验。
- Fig 补全固定来源为 `@withfig/autocomplete 2.692.3` 并核验 npm SHA-512 integrity；将 34 处应用说明文案及 SSH／传输命令的主机、路径补充纳入显式生成流程，全部 97 份既有输出重新生成后与升级前逐字节一致。以真实 SSH 配置／Include／known_hosts 候选行为回归替代注册键文本检查；Windows／macOS 打包同时携带上游 MIT 版权通知，来源与 npm 许可元数据不一致见 `scripts/fig-convert/README.md`。
- 品牌资源生成环境切到 Python 3.13+／Pillow 13.0.0，保留 `resvg-py 0.5.0`；实际重生成的 11 项资源保持全部图像帧像素与 SVG 不变。Hack 四个字重／字形文件核对为官方最新 3.003，字体与图标设计不改；独立 DOOM-fire 的 Zig 0.14.1 基准例外继续保留。
- 修复共享核心 stdio／路由测试对 Windows 上 `cat`／`sleep` 的隐式依赖：Windows 使用系统 PowerShell 的原始字节流并禁用用户 profile，Unix 使用系统工具，不要求安装 Git Bash 或修改全局 `PATH`。字节往返回归覆盖 NUL、非 UTF-8 与 CRLF；释放链路的回归等待子进程就绪并实际核验退出，不再仅等待固定时长。仅调整测试夹具与断言，生产 SSH／WSL／stdio 行为不变。
- 依赖升级后，Windows 隔离桌面实例已实跑中文／emoji／组合字符与 ANSI 输出、深浅主题切换、复制粘贴、Fig 键盘补全、分屏焦点、滚屏、最大化／还原、窄窗口重排及字号缩放；编辑器 Unicode 修改实际保存，Git 并排／统一 diff、全局搜索与 Escape 焦点恢复正常。Windows 拼音候选提交、终端／Kit 输入框双向切焦点与 composition 跨 pane 不串入已复验。关闭设置与主窗口时日志仍出现 `window not found`／`无效的窗口句柄`，未阻止 GUI 退出或 daemon 保留 pane，尚未确定是否由本次升级引入；不据此宣称 macOS 实机、性能基准、正式安装或在线更新已通过。
- Windows 平台收窄为 x86-64：取消 `aarch64-pc-windows-msvc` 桌面 GUI 与 headless server 构建、发行腿与 ConPTY arm64 固定包；ARM64 Windows 远端主机（`PROCESSOR_ARCHITECTURE=ARM64`）被明确拒绝，不再安装 x64 模拟 server，与 macOS Intel 拒绝策略对齐。ARM64 Windows 桌面用户可改用手动安装的 x64 包（`x64compatible`），其在线更新继续取 x64 资产。发布材料、打包脚本、准备脚本、安装与远程工作区文档同步为 x64-only。

- 平台适配迁入正式 GPUI 接口：Windows Acrylic/Mica 等背景材质由应用自有材质状态与官方 `DwmSetWindowAttribute` 接线保留，17763／22621 等 build 分流与 Blurred/Transparent 回退契约不变，主题切换、激活／失焦与窗口重建均重放材质并清理 WCA／DWM 两组属性；macOS `font_thicken` 经完整委派的 `Platform`／`PlatformTextSystem` 包装注入 `Application::with_platform`，dilation 真实进入字形光栅化与精灵缓存键，不写全局 `AppleFontSmoothing`；原生菜单 enter／tab 字形经官方 `NSMenuItem` 后处理修正，快捷键不变。平台构建与原生验收证据分别记录，不把实现存在当作全部平台通过。
- GUI 框架入口统一迁到正式 `gpui-kit 0.7.3`／`gpui-pre 0.3.9`，组件状态、通知层、主题映射及测试入口按公开 API 适配；GUI HTTP 使用匹配的 `gpui-pre-reqwest-client 0.3.9`。正式集成工作树已完成 Windows 构建、实际启动及适用行为回归，完整原生工作流、性能和最终交付仍逐项验收。
- GUI 与 CLI 的终端解析统一迁到正式 `rio-vt =0.5.28`（关闭默认 features），保留 lyrel 自有 daemon、PTY 和绘制；不引入 Rio 窗口／PTY／Sugarloaf。消费者宽度、选择与捕获随之迁移；约 65k 同时存活样式及 raw 缓存行作为 GC root 时的颜色回退属于已接受边界，不宣称长期无限保真。原生输入、resize 与最终性能结果仍逐项验收。
- ConPTY 使用显式准备的官方固定包 `Microsoft.Windows.Console.ConPTY 1.25.260930003`（文件版本 `1.25.2609.30003`）：`pwsh scripts/prepare-conpty.ps1` 校验 nupkg SHA-512、逐文件 SHA-256、文件版本与 Authenticode，输出到不入库的 `.conpty/`；支持共享目录和 `-Offline` 复用。`lyrel-core` 构建脚本为 GUI、server、CLI 与测试程序统一暂存，打包校验相同摘要；daemon 首次创建 pane 前拒绝缺失、损坏、架构或版本不匹配的旁加载文件，不静默回退系统宿主。仓库不保存这对二进制副本。
- 本地产品身份切换为 lyrel，GUI、CLI、共享核心、server 与 updater 从 `0.1.0` 独立编号；命令、crate 路径、构建和打包调用方同步更名。
- 默认配置、状态、凭据服务、daemon 发现、shell 与 coding agent 集成采用独立 lyrel 命名空间及 `LYREL_*` 环境，不自动读取、迁移或停止其他产品的数据和实例。
- 当前界面、所有支持语言、运行文档及项目入口同步更新；保留上游来源、第三方 fork、历史记录和既有 IPC 协议版本。平台注册、远端部署及在线发行验收分别记录，不因名称切换而视为已验证。
- 远端 SSH、Windows OpenSSH 与 WSL server 采用独立 lyrel 路径、程序和资产；远端下载仅使用 `dlivxpr/lyrel` 并执行校验和验证，WSL 缺少随包 server 时明确失败，不回退上游或网络下载。
- 修复共享 agent 配置仅按 `agent-hook` 子命令误认归属的问题：刷新、安装和卸载精确识别 lyrel 程序入口，保留其他产品与用户条目及混合 matcher 中的其他 hook；Windows 远端 hook 识别按协议命名的 server。
- 平台安装与启动契约使用独立 AppId、AUMID 和 bundle identity；macOS 包内图标、便携／Inno 标记、CLI 和 WSL 随包 server 保持配套布局。Linux 不提供桌面安装或通知注册入口。
- 打包与校验入口统一为 `.github/scripts/`，既可由 Actions 调用，也支持受控工作树手动打包；安装文档补全各平台布局、卸载边界和签名／公证前提。
- stable 更新只读取 `dlivxpr/lyrel`，按严格 SemVer 从分页 feed 选择最高桌面稳定版，支持 `0.1.0`／`v0.1.0`，拒绝同旧版本、预发布、错误版本或平台包及缺失校验和。复用锁定的 `semver 1.0.28`，第三方 Rust 依赖统一采用正式 registry 发布包。
- 完整 Windows 包必须携带精确架构的 WSL server；macOS ZIP、Windows Setup／ZIP 的更新形态及独立安装标记保持一致。保留其他产品资源共存下的替换／恢复隔离回归及 SHA-256 清单入口。在线更新仍以本项目真实 release 和平台资产为前提，未擅自发布或安装。
- 整合验收修正 CLI capture tail 回归：使用无外部提示符的真实 PTY 输出，观察尾部行、前部裁剪、原始 byte 计数及退出清理，避免用户两行 prompt 令正确实现被误判。移除仅重复 GUI 程序名常量的测试，保留真实 GUI 启动与 CLI／daemon 行为验证。
- stdin E2E 按实际外部 payload 的 byte 长度核对报告，不再硬编码改名前的长度；保留不回显秘密与真实 pane 送达验证。
- 双轴审查修正 server hook 报告测试的哨兵长度：按正式 sentinel 的 byte 长度同时检测 socket 路径不写 TTY 与拒绝后回退 TTY，保留状态断言。Windows hook 生命周期测试通过既有 `HookTarget::local_for_exe` 使用隔离的 `lyrel-app` runner 路径，不再把 Cargo 测试程序名当作产品身份；保留旧 其他产品、用户配置、配置覆盖与幂等保护，不扩大生产归属范围。
- 更新模块集中 Inno 标记内容，并让桌面资产版本解析与各平台包名构造共享精确后缀常量；支持的平台、架构、包形态及拒绝条件不变，不新增品牌注册中心。此次修正的运行结果由集中验收补记。
- Unix 退出观察测试先等待 reaper 完成，再断言最终 PID 不存活，并核对真实退出信号为 SIGTERM；避免将并发回收过程的短暂观察当作退出失败，仍要求 helper 在原有 grace window 内确认退出，不改生产生命周期逻辑。
- 当前快捷键说明不再将上游版本当作 lyrel 迁移历史；应用、窗口标记、托盘、安装器、文档与 favicon 使用已批准的 lyrel 图形及可复现生成入口，保持 UI 主题不变。跨平台真实 GUI、包内容与安装显示须分别验收。
- SSH／SFTP 使用正式 registry 依赖，保留主机密钥、撤销、提示与记住决定，不新增证书／CA 信任。GSSAPI 使用正式接口，仍依赖适用平台的真实服务与本地票据，不能以编译或 ignored 状态代替验收。
- 修复 LSP 初始化期间提前打开文件导致服务器退出的问题：客户端在同一发送闸门内先发 `initialized`，再释放排队的文档消息。保留协议顺序回归；Windows 正式工作台已用真实 `rust-analyzer` 完成符号重命名、同步修改引用与 UTF-8 磁盘保存。其他平台与最终原生验收分别记录，不由此推定全部迁移完成。
- 修复终端与 Kit 输入框切换焦点时的预编辑归属：失焦清除原控件的未提交文本，Windows 在公开根准备阶段取消原生 composition，避免候选提交串入新焦点或取消消息误删新输入框选区。Windows 正式窗口已复验双向切换、提交／取消、终端 BackTab 与连续最新剪贴板；其他平台及最终候选仍分别记录。
- Windows 便携 updater 在替换安装前核验固定 ConPTY 配对文件与 MIT 通知的 SHA-256；缺失、损坏、架构或版本不匹配时拒绝包并保留原安装，不再将系统宿主当作缺文件回退。受影响 updater 回归和只校验冒烟已运行；未执行真实安装或在线更新。
- 修复同批输出内连续同名命令的标题退休顺序，按流内切点清理 Rio 标题去重状态，并保留同步输出结束后的 shell 标题；Windows 终端 1030 项回归通过。CLI resize E2E 使用真实有限 PTY 输出，验证内容而非把含模式恢复前导的 raw capture 错当作整段字节后缀。
- 自有编辑命令的空白修剪保持非空活动选区的行作用域，保留选区之外内容与 CRLF；LSP 补全不再用客户端子串过滤丢弃服务器返回的 fuzzy 匹配。
- Windows 远端 server 使用 `lyrel-server-windows-x86_64.zip`，携带同架构固定官方 ConPTY 和必要许可；完整暂存、协议核对与配套先行落位，旧裸 exe 安装或缺件会修复，禁止裸 exe 回退。
- 普通双击选词、反向拖选与自动滚动沿用用户配置的 `word_separators`，包括双列分隔符；保留智能精确范围、括号、软换行和宽字符完整选择。GUI 输入栏与 CLI 表格共用 Rio 公开 grapheme／presentation 占列规则，避免 ZWJ emoji 与 VS16 导致光标及列对齐偏移；副屏期间修改的历史容量在返回主屏时应用最新值。
- Windows 便携 updater 在校验整个下载包之后，读取包内 `ConPTY-manifest.json` 核验目标版本的文件摘要、PE 架构、配对版本与许可；旧 helper 不再把自身编译时的 ConPTY 固定版本误当成后续发行包的唯一合法版本，不执行目标程序来完成校验。缺件、损坏、混版与错误架构仍明确拒绝，尚未执行用户安装替换或在线更新。
- 取消 Linux 桌面 GUI、AppImage、桌面 updater 与专属构建／打包接线；桌面端保留 Windows／macOS。Linux 的共享核心、server、CLI、SSH／SFTP、Host／PTY、GNU／musl server 资产及 Windows 随包 WSL server 保留。Linux 根桌面构建明确拒绝，CI 材料改为验证 headless crate；历史 Wayland IME 失败不记作已修复，Windows／macOS 的未验收项不因退役而自动通过。
- 移除内嵌的 `bash-preexec` 实现，Bash 4.4 及以上改用公开 `PS0`，Bash 3.2 保留自有 `DEBUG` 兼容路径；用户自行加载的库只通过公开函数数组接入，不安装或下载第三方脚本。真实 PTY 覆盖命令边界、退出码、多行／控制字符转义、截断、用户 prompt／trap、vi 模式及远端临时目录清理。旧 Bash 的 prompt `$_`／`PIPESTATUS` 限制经冻结基线对照确认并保留，不将其误写为新回归或已修复。
- 修复正式 Kit 编辑器水平滚动时正文穿入行号栏的问题；按用户批准，行号窄栏使用不透明语义主题底色，不再透出壁纸／渐变，正文背景、行号、折叠和滚动保留。正式 release 已实际打开同一中文长行样本并水平滚动复验；此限定外观差异不扩大到终端或窗口材质。
- Windows 远端 detached 启动在目标机优先选择已安装的 PowerShell 7，未安装时保留系统 5.1；经 WMI 创建的隐藏 cmd 包装启动选定 supervisor，避免直接 WMI→PowerShell 在部分主机无法执行。解释器选择只执行一次，保留环境、日志及带 nonce 的退出码，不改安全策略或新增必装依赖。真实首装、运行中强制替换与缺件修复均已运行，
- 降低 Rio 正式 viewport 消费成本：在同一锁定帧内复用已解析样式，直接读取源网格可见行切片，逐格保留文字、组合符、链接和选区；复用绘制缓冲区时只清空源网格外的 padding，避免全量默认初始化后再次覆盖。缓存不跨帧、不跨 palette／theme 变化，不影响隐藏 pane 持续解析与输出通知门控。正式小块 Unicode 绘制成本已回到冻结基线多轮范围，完整输出、隐藏恢复、大 diff／列表及最终无诊断候选范围须分别记录，不外推为所有平台或所有指标都加速。
- macOS 支持范围收窄为 Apple Silicon：不再构建、发行或更新 Intel macOS 包，`lyrel-server` 仅发布 `macos-aarch64`；安装文档、README、远端主机说明、禁用发布矩阵与 bug 模板同步。更新器不再把 `-macos-x86_64` 视为桌面包，远端资产解析拒绝 Darwin x86_64 并提示 Apple Silicon。macOS 实机证据仍记录为未运行。

- 更新收敛为仅 stable：删除渠道设置、搜索入口、专属配置及滚动发布元数据；保留 stable 发布分页、草稿与预发布排除、精确平台包选择、完整性校验和安装失败恢复。
