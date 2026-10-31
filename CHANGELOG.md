<!-- 本文件源自 l0ng-ai/tty7，lyrel 已修改（含本声明）；原有版权与许可保留。 -->
# 更新日志

本文件仅记录 lyrel 的当前变更，不继承衍生来源的发行版本。lyrel 从 `0.1.0` 独立编号。

格式参考 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，版本遵循 [Semantic Versioning](https://semver.org/spec/v2.0.0.html)。

## [Unreleased]

### 修复

- omp 的公开 `PI_FORCE_IMAGE_PROTOCOL`、Kitty placeholder opt-in／硬关闭项沿用非敏感启动上下文，在新启动和原生恢复中保持显式图片策略；不覆盖 `terminal.showImages: false`、不伪造终端身份、不修改全局 agent 配置。修复 Windows 快捷启动将字面量环境前缀当命令执行，以及恢复命令将子进程图片策略遗留在父 shell 的路径。
- 补充 released omp 原生 `--resume` 的隔离合成会话与生产消费者用例，覆盖 assistant／toolResult 图片、原生 SVG 栅格图、8 张上游 cap 删除与显式关闭。正式 `18.14.0` 的中央三图几何／关闭策略及独立八图预算回归通过；Windows 原生八图身份、旧图 demotion、滚动及两种关闭策略通过，三轮审计均消息未增、零请求／模型用量。上游 `18.14.0`／`18.14.1` 的 assistant 按 content slot 共享 key，跨消息同 slot 会碰撞；预算夹具使用同一正式消息的不同 slot，不修改代理或生产 renderer，不宣称绕过此上游限制。最新 release `18.14.1`、macOS 与真实 SSH 组合未验收。
- pi 图片接入（#149）区分 `earendil-works/pi` 与 omp，将公开 `PI_IMAGE_PROTOCOL` 纳入既有 Host／启动／会话 scope 与 Pi hook 报告，恢复时保留显式 `none`，不自动覆盖 `terminal.images=false` 或 `terminal.showImages=false`。正式 `1.4.0` 的真实 PTY／rooted 卡片像素和几何、删除／重挂载／重传及显式关闭优先级回归通过；Windows 原生二十图合成会话两轮重放、历史导航、删除、零新增消息与四种图片策略通过。无新 prompt／推理、不读私人历史；macOS 与真实 SSH 的 agent 组合仍待对应集成验收。
- OpenCode 集成按既有 `agent_launch` 的受检 Host／`XDG_CONFIG_HOME` 作用域定位 CLI 插件与配置，不再误写默认目录；保留显式 `session.image_preview=false`，不全局改设置或伪造终端身份。补充公开 `-s`／`--auto` 启动参数识别。正式 `2.2.0` 的 production consumer 与 Windows 原生首尾六图、卡片几何、分页、滚离返回、tab 卸载／重挂载及显式禁图均通过，公开 export 未变、零模型用量；MacBook Air M5 与真实 SSH 的 agent 组合尚未验收。
- 修复 synchronized update 中 Kitty 命令先于暂存文字执行、导致多图共用旧光标锚点的问题：daemon 权威镜像按流顺序及时应用文本，桌面保留自己的同步呈现。真实 PTY／daemon／rooted consumer 回归先失败后通过；pi、OpenCode 的卡片几何和 Windows 原生图片随后实测通过，omp 的中央三图几何通过，完整预算／原生组合仍按独立门槛验收。

### 新增

- Kitty 图片支持 daemon 持有的完整帧追加、停止／手动选帧和同一存活 daemon 的 fresh-client 恢复；同资源全部放置共用选择与像素，帧序列按整体回收并沿用有限源内容／派生／元数据准入。pane 协议升级为 `16`，control 不变；兼容显式 `f=0` 与无歧义的最终块无 padding Base64。真实 PTY／daemon／rooted consumer 及 Windows Debug 原生窗口已验证三帧同步切换和发送器退出后的 fresh-window 恢复；自动播放、partial-frame、完整容量／性能和 macOS 不由此切片宣称通过。
- Kitty 自动播放由 daemon 保存 stopped／loading／running、frame gap、有限／无限循环和剩余间隔；聚合当前连接 viewport 的 direct／relative／Unicode 可见性，全隐藏或断开时暂停，显式命令仍生效。循环复用保留像素，只发布选帧／播放元数据，pane 升级为 `17`，control 保持 `16`。Windows 五项受控时间、一项真实双消费者／fresh-window 回归与原生播放／loading／追加／隐藏返回／发送器停止后恢复已验证；不据此宣称双 pane 1080p、60 fps 或 macOS 通过。
- Kitty `a=f` 增加 partial append／retained frame edit：支持 prior-frame 底图、32-bit RGBA 背景、像素矩形、alpha／overwrite、raw／PNG／zlib，使用锁外有界准备与新不可变内容 key 原子提交，保留全部 placement 及 fresh-consumer 最新版本。Windows 核心事务拒收、stale prepare 竞态、真实生产消费者像素／恢复回归和原生局部追加／编辑／root 编辑与无重传恢复通过；播放中合成／单帧删除、完整容量／高帧率、真实 SSH 和 macOS 由后续票验收。

- Kitty 相对放置在既有 daemon Placement／pane transport 中保留 `P/Q` 父身份、signed `H/V` 及八链接有界图；支持 direct／实际 Unicode 单元父锚点、无光标移动、级联生命周期和 fresh consumer 恢复。Windows 五项真实 PTY／daemon／rooted view 回归及 `scripts/kitty-relative.mjs` 原生 geometry／recovery smoke 已通过；完整 DPI／性能、真实 SSH 组合及 macOS 不由此结果替代。
- Kitty 间接媒体增加 Windows 本地 regular file、eligible 临时文件与 named shared memory；`O/S` 只限制实际图片区域，允许小区域位于大对象中。Windows 固定已验证本地磁盘 handle、拒绝网络／device／未知 reparse，并按对象 handle 删除临时 handoff；Unix 保留文件／POSIX shm 语义，macOS 按偏移附近有界映射，临时文件使用身份核验及 no-clobber 恢复避免删错替换对象。发送方 locality 每次接受及读取前重新证明，嵌套 SSH／WSL 或未知命名空间 fail-closed，直接传输不受影响；不新增图片磁盘缓存。Windows 真实 PTY／rooted consumer 与原生三类媒体红绿像素／安全应答 smoke 已通过，Linux headless 完整 features 回归通过；真实 SSH 图形组合及 macOS 仍待对应票验收。

### 验证入口

- 正式 OpenCode 2.2.0 隔离保存对话夹具使用公开 import／resume／export，无 prompt／推理；完整目标保留用户附件、tool-result、每卡三图、旧消息分页、滚出返回、native tab 删除与重挂载及 disabled 策略。中央已完成 546 条消息 prepare，disabled consumer 通过；enabled 在首次卡片重叠检查失败，后续阶段未到达。加强后的用例按独立 placement instance 计数，不再合并同色 bbox，并保留官方 16×8／gap=1、容器文本与 viewport 边界及完整几何诊断；新增同色／同原点放置、同 ID 替换和精确删除的生产消费者用例。新增诊断尚未运行，不能据此声称生产几何或 Windows／macOS 最终像素已修复。

## [0.3.0] - 2026-10-31

### 新增

- Markdown/MDX 默认使用原生即时编辑，可在同一文档内切换完整源码；共享内容、revision、可逆事务和撤销历史，各呈现独立保留选择、焦点和滚动。移除 `--markdown-workbench` 开发入口及旧只读预览设置；普通代码、新建纯文本与 scrollback 保持原编辑器，并新增显式“新建 Markdown 文档”命令。
- Markdown/MDX 按行保留 LF／CRLF，局部编辑、撤销／重做和组合输入取消保留片段表示；保存保留编码与 BOM，不执行全文整理。损坏的声明编码拒绝打开，不可编码输入保留可见错误；显式编码／换行转换继续沿用既有工作台。
- 连续正文／标题、行内标记、链接、列表／任务、GFM 表格、代码、公式、Mermaid 与跨结构源码剪贴板共用原生输入和文档事务。查找／替换、行命令、导航、所属 Host 保存、外部冲突及关闭协调沿用现有工作台；图片加载、系统截图与拖入资源不跨 Host。
- 全部 12 个专用 MDX 组件及未知 JSX 支持属性／children 与局部源码编辑，不执行项目 JavaScript。MDX 使用正式 `ox_content_parser 3.4.0` 公开 AST；公式、图形使用正式 Merman、RaTeX 与原生光栅化，保留相关字体、来源与许可，不依赖 WebView 或项目运行时。
- 本机、远程工作区文件树及普通 SSH 的 SFTP 列表支持 Ctrl／Cmd 多选与真实文件／目录批量复制、粘贴和删除，支持 Shift 点击范围选择；右键保持已有选择，删除确认固定所属 Host 与完整目标，文件树祖孙去重、根目录保护及部分失败报告沿用同一操作入口。

### 依赖升级

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
