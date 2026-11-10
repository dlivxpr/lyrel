# Markdown 编辑器 Host 能力清单

## 当前交付边界

本清单是 #236 的实现契约，不是验收通过声明。#237 提供可执行的文件生命周期与共享文档基础；其行为测试、原生字体、系统 IME、真实 SSH、Release 可用性与平台结果必须分别记录。后续能力按下表完成；依赖 #237 的票不得以独立文本控件演示替代真实工作台。最终完整功能与默认路由验收属于 #245。

源码基准固定为 [Zorite `9f069c73a56b06efc84fb7abb700beb50083c827`](https://github.com/packetThrower/zorite/tree/9f069c73a56b06efc84fb7abb700beb50083c827)。原始文件摘要、版本和适配范围见 `UPSTREAM.toml`；MIT 原文见 `licenses/`。导入限于 editor/bidi 与 #241 的 ratex-gpui，不包含 GPL 应用编排。GPUI 只经 `gpui-kit` 的 Fast 类型，不建立第二个 GUI runtime。

## 所有权与修改协议

- `lyrel_markdown_editor::Document` 独占文档身份、LF/UTF-8 原文、逐行原始行尾、格式策略、history 与单调 revision。
- `EditorState` 是 presentation：选择、方向、focus、scroll、marked range、layout/geometry cache 都独立。其 `Arc<String>` 是当前不可变 revision 快照，不是第二份可编辑权威。普通编辑不以 `set_text` 或整篇 diff 同步其他视图。
- caret reveal 由当前 presentation 的显式选择/跳转、键盘移动、本地输入与 Undo/Redo 请求；不依赖当时键盘焦点，不由其他 view/Host 的文档通知自动请求。同一 source wrap-boundary byte 可是上一视觉行末或下一行首；caret 与 selection anchor 的 affinity 只归该 presentation，pointer/Down/Up、selection、popup/IME bounds 共用该状态，普通 link/range 映射不全局改为 downstream。native 门禁须实际验证两侧指针、往返导航、输入/Undo 和非聚焦 go-to 的滚动隔离；源码修复不等于验收通过。
- 屏外 shaping 可用空占位保留行高，但当前 caret 所在的逻辑行始终保留真实 shaping，即使 presentation 没有焦点；否则非零列的 `range_to_bounds` 为 `None`，go-to 只更新源坐标而不能滚动。此几何需求不改变非聚焦 view 的 caret-local marker 显露规则，亦不强制全篇 shaping。native navigation 覆盖 source 跳转，以及 live 下的非聚焦屏外非空行跳转。
- prepaint 的全行命中区与选择区域复用当前 pass 已建立的行起点数组，不对每行重新扫描全文。跨可见区跳转后，只有量化 shaping band 变化才经现有 next-frame 通知补绘一次；不能依赖额外 hover、输入或持续重绘来填充新可见正文。native 消费者按实际请求递送后续帧，并核对非 caret 行的真实几何。
- #244 的 native Markdown inner inset 同时将真实首列 caret 留在现有 shell resize hitbox 之外；不再叠加 #237 的外层 `RESIZE_HANDLE_WIDTH / 2` padding。resize target 尺寸不变，不移动测试指针；留白属于 scroll owner 内的文档排版，不把 scrollbar 拉离 pane 边缘。实际宽窄窗口及缩放后的参考体验仍须原生对照。
- 所有源文变更和 Undo/Redo 发出 `DocumentEvent`。`edits()` 按实际应用顺序给出每一步 LF/UTF-8 range、removed、inserted；后一条 range 基于前一条已经应用的文本。消费者仅应用更新的 revision。
- 初始加载/外部重载明确调用 `Document::new` / `reload`；不是普通编辑协议。加载不整理公式、不补换行、不重写未知构造。
- splice 在同一操作中修改原文与对应行尾；未涉及的行尾保留。新增 LF 使用当前文档的首选行尾；显式 CRLF 输入保留 CRLF。删除换行同时删除它的表示；拆行新增表示；重排整行携带原行尾。Undo/Redo 原子恢复文本和表示。
- preedit 是 Document 持有的单一 composition lease，view 只持有本地 marked range；切换输入所有者先结束原 lease。正常关闭会提交 preedit；若所有者已销毁，后续 claim 将其原子提交为可撤销 history 并发布通知，不将过期 lease 留给后续 edit。
- 工作台保留 `BufferId`、Host/path identity、dirty、save/save-as/conflict/close、tabs/groups、导航与状态栏。`lyrel-text` 是两个真实消费者共用的 GUI-free Apache-2.0 codec/EditorConfig 策略；editor 不依赖 app 或 `lyrel-core`。I/O 通过工作台捕获的 owning Host 在 worker 执行。
- 保存捕获共享文档原文、表示、格式以及 Host/path；晚到完成不能将较新的编辑标为 clean，也不能改变另一个 group 的 close intent。

## 能力、负责人和可观察验收

“上游保留”表示行为实现已导入；不是声称 Host 已配置或原生结果已通过。“待接线”能力保持源码可访问，不暴露假成功的 provider 或无响应按钮。

| 表面 | 分类与所属票 | 当前边界与完成条件 |
| --- | --- | --- |
| 共享源文、revision、history、独立 view 与 composition | Lyrel 适配，#237 | 两个真实工作台 presentation 编辑/Undo/Redo；独立选择、scroll、几何；切换/关闭 composition owner 后继续输入、保存。 |
| open/edit/live-source/dirty/save/close/reopen | Lyrel Host，#237 | BOM + 混合 CRLF/LF 文件 untouched save；第二视图输入、生产 host range action、Undo/Redo、保存/重开；同名 mapped Host 文件不被修改。 |
| 最小真实行列、go-to-line | Lyrel 工作台，#237 | 状态来自当前 presentation 的共享源文；Unicode 跳转、选择与变更后的行列一致，不为查询复制全文。Markdown 跳转只设置一次选择，由其 view-local reveal request 等待布局；不复用普通 Code InputState 的跨帧光标重放，避免覆盖用户后续选择或分屏后的独立状态。 |
| 本文宽度、字体层级、段距、padding、theme、zoom、disclosure 占地 | 上游排版 + Lyrel shell，#244 | `markdown::configure` 明确使用 fluid width、1.75 rem 左右 inset、1 rem 上下 inset；Arial 1 rem 正文、1.45 行高及上游 H1/H2/H3 1.8/1.5/1.3 层级。列表间距/缩进、引用 inset、callout 图标与标签间距随正文缩放。主题 provider 每次呈现读取 semantic roles，不重置模式/选择/history；scrollbar 仍贴 pane。heading 只绘 hover/折叠态小 chevron，不增加行。代码已接入，固定参考实窗复测仍待集成者执行。 |
| 快捷键、caret-local 标记显露、选择、列表/任务/标题/callout | 上游保留 + Lyrel 写作适配，#244 | 复用原生格式/列表/任务/转换和 caret-local reveal；引用/callout Enter 延续 `>`，空引用 Enter 退出。`Ctrl/Cmd+Alt+F` 切换当前/祖先 heading 的 view-local fold，不 dirty；callout `+/-` 与任务勾选仍修改同一可撤销源码。所有输入仍经 foundation 的 affinity/composition 协议；原生消费者与实窗结果不由源码交付推定通过。 |
| 插入/slash menu、construct picker、table insertion | Lyrel Host + 上游命令，#239/#244 | 行首 `/` 或 `Ctrl/Cmd+/` 打开 Kit `PopupMenu`，方向键/Enter 选择、Escape 保留原文并返焦。支持正文、H1–H6、列表/任务、引用、五种 callout、代码/公式、分隔线、链接与行内公式。菜单捕获 view、revision、range，插入不清空 history。#239 的 `markdown::table_code` 为每个真实 presentation 提供 8×8 键盘/指针尺寸选择、自定义 1—100 行列与四种样式；尺寸、样式、源文与 history 同次提交。取消不写源文，revision 变化拒绝过期提交；`open_table_picker` 是通用插入/slash 的同一入口，不复制 GPL picker。#238 的 owning-Host 插图使用其实际 action。 |
| 表格行列、对齐、样式、导航、resize、code fence 语言/复制/高亮 | 上游保留 + Lyrel Host，#239 | primary/secondary 均配置 Kit registry/tree-sitter highlighter，按当前主题缓存 token 样式；原生 grid 的 in-cell pointer/Unicode、Tab/箭头/Enter、边框 drag/双击 fit 保留，另有 Kit 键盘菜单提供行列增删/对齐/四种样式/fit/宽窄调整。code 语言只修改语言 token，保留 fence metadata；复制正文与 delimiter 访问使用同一 editor。工作台/native 消费者已接入，集中 checks 和实窗对照尚未运行，不声明通过。 |
| 文字/高亮预设颜色与自定义颜色 | 上游保留 + Lyrel Host，#244 | `WritingProvider` 安装真实 Kit `ColorSelect`、HSLA/hex picker 与显式应用/取消/清除草稿，才启用 custom swatch；`PickColor` 事件仍可观察。`Ctrl/Cmd+Alt+C` / `Ctrl/Cmd+Alt+H` 直接打开文字/高亮 picker。草稿不修改源码，提交核对捕获的 view/revision/range；改色和清色各是同一 shared history 的一个原子步骤。native consumer 覆盖取消/hex 提交/clear/几何与双视图 Undo，执行结果待集成者记录。 |
| 普通相对/外部链接、hover、caret 附近的源码访问 | 上游行为 + Lyrel Host，#238 | `markdown::links` 通过当前文档 Host/path 解析普通链接；`ImageHost::attach` 在每个 presentation 订阅 OpenLink/HoverLink 后启用导航。普通单击在 release 激活；拖动取消待激活目标，Shift/Control 保留选择/编辑。wrapped 起止使用各自 affinity，并覆盖所有中间视觉行；hover 显示实际目标。异步目标捕获 tab/group、BufferId、revision 和 owning epoch，失败可见。外部仅允许 HTTP(S)/mailto；SSH 不接受 file URL、Windows/UNC 路径。源码内 wiki/tag/block-reference 不安装导航或 hover。源码和消费者已接线；native、实窗与真实 SSH 结果由集成者记录，不由本表推定通过。 |
| 图片插入/paste/drop、资源、preview、resize/zoom | 上游保留 + Lyrel Host，#238 | `markdown::images::ImageHost` 持有 owning Host bitmap cache、明确 loading/blocked/error、每 view 事件订阅与路径 generation。本机文件面板/OS drop/截图经 worker 读取输入，唯一 UUID 名称与 create-new 写入 owning Host 的同级 assets；未保存文件显示 needs-save，不创建目录。插入/resize 走同一 Document history；Undo 保留资源供 Redo，晚到 source/selection 不符不插入。单击真实 block/inline 图片打开 native preview，提供键盘可达 zoom/fit 与滚动，角部 drag 只改 width 属性且 revision 变化取消提交。HTTP(S) 默认不请求，逐资源主动加载由本机 client 发出，遵循 proxy，拒绝重定向和 SVG 的所有嵌套资源。输入上限32 MiB、单图1600万像素、文档bitmap256 MiB；失败保留源码与重试。 |
| inline/display math 渲染、二维结构编辑 seat/menus/commit/cancel | 指定 MIT `ratex-gpui 0.4.1` + Lyrel `markdown_formula` Host，#241 | `src/math` 保留 9 个 pinned source 摘要；renderer 为 registry RaTeX 0.1.15。每 presentation 独立异步 bitmap/seat，版本、document identity、原选区与原 range 校验后通过 `replace_ranges` 提交 body/对齐 batch。Enter/箭头退出应用、Escape 取消、Ctrl/Cmd+S 先提交再走原 Host 保存；Kit palette、native copy/export/块对齐菜单和焦点返回已接线。原生与参考实窗对照尚待集中执行。 |
| Mermaid cache/render/entry/refresh/error/stale-result | Lyrel Host，#242 | `markdown::mermaid_host::install` 在所有 presentation 的共同 configure 入口安装真实 Merman `0.9.2` → `resvg_safe` → Kit `SvgRenderer` provider；caret/selection 进入块即显露原始 fenced source，正常输入与 Undo/Redo 使用同一 Document。loading/失败在块首行显示，正文源码仍可编辑；错误修复触发新 request，不显示假图或无限等待。见下方所有权与消费者边界。 |
| status/search/outline、tabs/groups、MDX source-only、focus/layout | Lyrel 工作台，#240 | `MarkdownPresentations` 按稳定 TabId/GroupId 保留 view/search/observer；新 split 从实际来源 view 继承模式、选区与滚动。每个 view 的通知刷新工作台状态；输入及搜索焦点参与命令捕获阶段的组定位。MDX 路径身份变化只改变 source-only 能力，不替换文档/history。生产与 native 消费者入口见下方；实际原生窗口验收仍由集成者执行。 |
| 编码/BOM、显式换行转换/cleanup、save-as/conflict/failure/close | Lyrel 工作台 + `lyrel-text`，#243 | `saving::fidelity::SaveContent` 从同一 Document snapshot 捕获源文/表示/格式/字节；成功才采用另存为目标；新编辑退休旧close。消费者覆盖 UTF-8 BOM、UTF-16LE/BE、GB18030、Windows-1252、显式转换/稀疏cleanup与Undo、失败/冲突/取消/扩展名、第二呈现在途快照。沿用全部原保存消费者；真实local/SSH与native结果由集成者另记，不以mapped Host或历史结果冒充。 |
| extreme nowrap 行、完整源文/尾部 caret/input/Undo/save | 上游布局 + Lyrel 安全保护，#237/#244 | 实际 native 宽度超过 65,536 逻辑像素时保留有界视觉折行；不插入源换行、不截断、不更改 wrap 偏好。caret 几何以整数视觉行号乘一次行高，不能逐行累加 `f32` 后再让 native hit-testing 除行高反解。#235 的 not_planned 不是 SDK 修复证据。 |
| imported bidi 映射 | 上游保留，#237/#244 | 保留 `bidi/` 与 source/display/native position mapping。完整 RTL 仍在已批准验收范围之外，不能由普通 Unicode 成功推导 RTL parity。 |

## 明确排除但保留源码

| 构造 | 行为与负责人 |
| --- | --- |
| notebook/database、页面管理、知识库搜索、backlinks、wiki 页面、block-reference 浏览 | 不导入 GPL app 或建立 notebook。`[[...]]`、`![[...]]`、`#^...` 及相关 source 均原样保留；#238 区分普通文档链接和这些排除构造，不留下无目标导航。 |
| 上游 property/wiki 渲染表面 | 不迁移 notebook property inspector。`property_icon` 保持 `None`，不注册 property editor/provider 或 slash property 命令，`key:: value` 原文直接显示和编辑；wiki/embed 不建立假导航。frontmatter 与未知构造保留源码访问。它们不是旧 Lyrel inspectors 的替代实现。 |
| 旧 Lyrel MDX rich components、visual inspectors、自定义 workspace references/embeds、额外 visual extensions | 旧实现与专属消费者随 cutover 删除，不能以 test-only adapter 继续保留。MDX 使用 source-only；frontmatter 和未知语法始终 intact/source-accessible。负责人 #240/#244，最终文档 #245。 |

## 测试与证据入口

- editor 回归：`cargo test --locked -p lyrel-markdown-editor --test foundation`。
- GUI-free codec：`cargo test --locked -p lyrel-text`。
- 工作台：`cargo test --locked -p lyrel --lib`；`src/ui/code_editor/markdown/foundation.rs` 经生产 lifecycle 运行 shared Host 与 composition 场景，不依赖旧 Markdown 内核。
- native 字体目标：`cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown -- --nocapture`。runner 将真实 text system 接入 deterministic test platform，不创建产品 OS 窗口；test settle 须通过 SDK `simulate_next_frame` 执行平台帧回调，pointer move 后再 down/up，不能仅以 `render_frame` 冒充完整输入/帧生命周期。它验证 native 字体几何和生产消费者，不替代实际产品窗口、系统中文 IME 或真实 SSH。
- #238 工作台窄入口：`cargo test --locked -p lyrel --lib ui::code_editor::markdown::resources_tests`；native registry 包含 owning-Host paste/history/save-reopen、三项 wrapped link 消费者、native preview/zoom/resize/drop，以及完整3768 B resources fixture（固定SHA）live尾部输入/保存/Undo。插图由#244的插入命令/slash派发真实`InsertImage`，不增加常驻整行图片工具栏。`#241` 的完整400034 B mixed-tall 及 hard-break formula 消费者同时使用真实 ImageHost，不能以无bitmap源文测试代替。所有新命令在分票阶段按约定未执行，Windows实窗、真实local/SSH、macOS与最终checks由集成者验收。
- Linux：`cargo test --locked --workspace --exclude lyrel --exclude lyrel-markdown-editor --all-features`；`lyrel-text` 可参与，GUI crate 不被选入 headless 范围。
- Release 输入/导航/scroll/Undo-memory、系统 IME、真实 SSH 与跨平台结果由集成者保存精确 commit、固定 fixtures、执行序列、display 条件、命令、退出码和未验前提。早先实现的历史验收不自动转移到本源码导入。
- 文件保真专项：`cargo test --locked -p lyrel --lib ui::code_editor::saving`；native selector `saving::fidelity` 包括编码往返、显式策略、第二视图在途快照、首次保存失败、编码取消/转换和目标冲突/重命名。Markdown普通保存不执行EditorConfig代码清理；显式去尾空白只删对应字节，Undo恢复原表示。另存为写入失败不改变原path/untitled，转换到MDX仅改变各view源码策略，不重建Document。

### #242 Mermaid 原生 Host 与消费者

- `MermaidHost` 属于单个 presentation，由 provider 的实体句柄保活，持有共享 Document 与弱 editor；关闭 presentation 释放订阅和缓存并取消工作。primary/secondary 都经 `markdown::configure` 安装，不借用焦点确定结果目标。
- renderer 使用正式 registry `merman =0.9.2` 的 `all-diagrams`、`complete-svg`，复用 Lyrel 既有的 native 字体测宽、完整 `SvgPipeline::resvg_safe`、外部资源拒绝和 Usecase CSS 字符引用边界修复，再由 Kit/Fast `SvgRenderer` 光栅化；不导入 GPL Host、不运行 browser/Node/项目脚本。单图最多单边 4096 px、800 万像素；每 presentation 的图像缓存与单批请求均限 64 MiB。限制失败保留源码，简化源码后重新请求。
- 每个块有独立 owner/request lease，key 捕获 Document entity、创建 revision、精确 source 与实际字体/颜色/字号。`DocumentEvent` 的有序 edits 按各自 snapshot/revision 移动未改块；普通正文编辑不会丢图或重启其在途渲染。整块移除撤销 owner，即使相同文本随后回到同一位置也不是旧 lease。晚到结果必须同时匹配当前文档 revision 的已观察状态、owner、request、source、style；旧请求不能覆盖新版、另一文档或复活删除块。
- provider 显式返回 `MermaidPresentation::{Loading,Failed,Ready}`；Ready 携带逻辑像素尺寸，不以纹理像素除窗口 DPI 猜测大小。宽度按当前 pane 约束缩小；源文行与布局仍由原 editor 管线统一映射。异步高度变化使用每 view 的原 ScrollHandle，锚定首个可见源行，不以整篇高度差移动另一 view。
- 新 native registry 为 `mermaid_host::native_edit_refresh_history_save_reopen`、`native_stale_completion`、`native_theme_zoom_scroll_geometry`。分别覆盖真实生产输入、尺寸更新、live/source 同一历史、双 view、invalid-source 恢复和 mapped Host 精确保存重开；真实原生结果在发布前延迟的 stale/source-change/remove/other-document 场景；主题字号、离屏异步尺寸变化、scroll/caret 命中和 Undo/save。延迟 seam 仅在 `native-markdown-tests` 编译，不替换真实渲染结果。
- 运行入口：`cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown -- mermaid_host`。本票源码交付期间按共同契约不运行 build/test/lint/formatter；上述消费者、固定 Zorite 同文档实窗外观对照、真实 SSH、系统 IME、macOS 与最终产物许可证均由集成验收分别记录，不能从 bitmap 存在或历史 #204 结果推定 parity/通过。


### #244 写作与视觉验收入口

`writing_tests::continuous_writing` 经生产 workbench 覆盖列表续行/缩进/退出、格式、引用延续/退出、第二 presentation 插入及 shared history。native registry 新增 `writing::native_text_color_picker_clear_geometry_and_shared_history` 与 `writing::native_wrapped_starts_prose_heading_list_quote_zoom`，不删除完整 extreme-line 与 affinity 门禁。源码交付期间按并行合同未运行 build/test/formatter 或操作窗口。

早期实窗由集成者实际观察：foundation `fc3b5de8` 与固定参考在 1195×801、Light、同一 1092 B 源文下比较，候选正文只距 shell 约 5 px，参考约 28 px；候选 callout 缺少 info 图标，wrap/节奏亦不同。这是明确未通过的排版基线，不是 #244 的通过证据。本票以源内 inset、真实图标、原生写作 Host 修正这些路径；提交后的 Light/Dark、docked/full、zoom、系统 IME 和连续书写仍须用同源文在实际窗口复测，不以 native-font test 或静态截图代替。

### #241 公式验收路径

`markdown_formula::install` 安装 primary 与 secondary 的 provider、版本隔离、结构 seat 和原生菜单。显示渲染和临时树不修改 Document；未修改提交直接返焦，不重新序列化原式。正文提交只替换去除外围空白后的 body range，原 fences、邻文及逐行行尾不回写。对齐标记是同一 batch 的另一条必要 edit。结构 entry 通过 AST 保真比较拒绝字体、颜色、间距、不可表示环境或无效解析，不把 best-effort wrapper 丢弃带入生产；显示 renderer 仍保留其支持的原语法和显式颜色。

临时树内部的 Undo/Redo 仅管理未提交草稿；提交本身只占普通 Document history 的一项。原生文本输入经 `EntityInputHandler`，预编辑不进入文档；候选 bounds 与 pointer 共用实际 formula image/caret 几何。主题、zoom、DPR 与 revision 标识异步 bitmap，晚到结果不覆盖新请求；关闭 presentation 回收 host/seat/图像。NBSP spacer 防止公式/图片在 soft-wrap 中跨错行，不插入任何源码字符。

- 窄工作台入口：`cargo test --locked -p lyrel --lib formula_workbench`。
- 结构/renderer 回归：`cargo test --locked -p lyrel-markdown-editor math::`。
- native：`cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown -- markdown_formula --nocapture`；四项 selector 包含 commit/cancel/history/Host save/reopen、unsupported/unchanged exact source、完整 400034 B mixed-tall fixture 的 wrap/nowrap 与 pointer/input/Undo/save，以及 hard-break/fragment 邻文安全。
- 同 fixture 对照固定 Zorite 实窗：点击 inline/display、分式 numerator/denominator、根式/定界符嵌套、补全/菜单、Escape/未修改提交、系统 IME candidate/focusReturn；逐字节比较保存，另做本地/SSH 和浅深主题/缩放。未运行不计通过，native 字体 runner 不能替代实窗或系统 IME。


## 最终许可证分发验收（#245）

#237 的 editor/bidi MIT 原文同时保存在 crate 的 `licenses/` 与现有打包入口携带的根 `LICENSE`；源码目录不等于实际产物验收。#245 须对 Windows ZIP/安装包、macOS app/archive 和 release artifacts 核验 editor/bidi 与 #241 新增 ratex 的实际许可证内容。检查入口包括 `.github/scripts/bundle-windows.ps1`、`bundle-macos.sh`、`verify-windows-package.ps1` 的根 allowlist，以及 `.github/workflows/release-build.yml` 的 artifact 白名单；不能只增加源树许可证而漏发二进制归属。

## 原 14 项 native 消费者迁移清单

以下以 baseline `5a38d0a9242761024335fe06263b040ddb3f5d74` 的真实 registry 为准，名称省略共同 `ui::code_editor::markdown::` 前缀。旧文件依赖已删除的 Rope/SourcePresentation/投影与旧格式菜单；不编译旧内核来让测试继续绿。当前基础六项消费者 **不等价于原 14 项全通过**。后续票必须在真实 replacement 工作台恢复其仍有效的用户行为与几何边界；仅已明确替代的交互或排除的旧 UI 可以退役。

| baseline selector 后缀 | 处理与仍有效的验收边界 | 所属票 |
| --- | --- | --- |
| `source::review_tests::native_down_visits_each_soft_wrapped_row_without_skipping` | 已迁移至 native navigation：逐视觉行 Down/Up 与 wrap-boundary 两侧往返，不允许跨行跳过。 | #237 |
| `source::review_tests::first_soft_wrapped_link_grapheme_hovers_edits_and_opens` | 已迁移源码到 `resources::first_soft_wrapped_link_grapheme_hovers_edits_and_opens`：首个 downstream glyph 的 hover、真实修改/Undo、普通单击与 owning Host 打开。待最终 native 执行，不宣称通过。 | #238 |
| `source::review_tests::fragment_tail_link_grapheme_hovers_edits_and_opens` | 已迁移源码到 `resources::fragment_tail_link_grapheme_hovers_edits_and_opens`：分片末尾字形 hover、修改/Undo 和打开；不按旧 projection 内部类型断言。待最终 native 执行。 | #238 |
| `source::review_tests::native_soft_wrap_caret_keeps_clicked_side_across_navigation_and_input` | 迁移：真实指针点击 wrap 两侧，箭头/输入/Undo 后 caret 不跳至另一侧。 | #237 |
| `source::review_tests::native_modified_link_drag_retains_upper_wrap_anchor` | 旧 Ctrl+Click-only 激活契约退役；`resources::native_wrapped_link_drag_retains_upper_wrap_anchor` 恢复普通 link drag 的源锚点、选择替换/Undo 与不导航边界，单击激活另验。待最终 native 执行。 | #238 |
| `source::review_tests::native_extreme_nowrap_line_keeps_pointer_selection_and_source_exact` | 迁移至 `foundation::zorite_foundation_complete_extreme_line_paint_caret_input_save_undo`；完整 2,523,166 B，不缩水为截断样本；保留指针/选择和原始源码条件。 | #237 |
| `source::review_tests::native_aligned_table_pointer_and_selection_follow_content` | 迁移至 `table_code_tests::aligned_geometry`：真实 cell glyph 命中、wrapped Unicode、grapheme 选择/复制、边框 drag、fit 与精确 Undo；保留原 selector。源码已接线，native 结果待集中检查。 | #239 |
| `tests::table_presentation::table_presentation_drag_fit_scroll_edit_history_and_host_reopen` | 迁移至 `table_code_tests::wide_table`：完整 88,987 B、48 列/64 正文行 fixture 的 live grid、指针编辑/Undo、独立横向滚动、窄宽 resize、保存重开；drag/fit 另由同组 aligned 消费者完成。旧 inspector/menu id 退役，保留原 selector。 | #239 |
| `tests::native_navigation_home_end_and_words_use_visible_logical_content` | 迁移新 navigation case 的 Home/End/Unicode word/grapheme，删除仅为旧 Rope chunk seam 的实现断言。 | #237 |
| `tests::native_text_color_picker_clear_geometry_and_shared_history` | 自定义色/取消/clear/geometry/history 仍有效；按 Zorite native menu + Lyrel picker 迁移，不重建旧全行格式工具栏。 | #244 |
| `source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics` | 保留超长 code 与离屏 diagnostics 的 caret/input/Undo 安全；旧 spelling UI id 不保留，新 pipeline 的 diagnostics 渲染另验。 | #237/#239/#244 |
| `source::spelling::tests::spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms` | #238 已接真实ImageHost bitmap与inline image machinery；完整400034 B mixed-tall fixture的图片/公式联合几何消费者由#241的 `markdown_formula::native_full_mixed_tall_formula_pointer_fragments` 迁移并在集成后运行。不能以源码字符串或#238普通preview case代替完整fixture。 | #238/#241 |
| `source::spelling::tests::spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom` | 保留 prose/heading/list/quote/zoom 后视觉行首与命中一致；按新排版层级迁移。 | #244 |
| `source::spelling::tests::spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows` | #238 提供真实图片provider与完成后的native layout失效；共用inline atom不可跨行与hard-break消费者由#241的 `markdown_formula::native_hard_break_formula_fragments` 迁移，集成后联合验证。该边界不能以单独资源加载成功宣称通过。 | #238/#241 |

这不是允许后续丢弃仍有效边界的豁免。#237 自有基础几何/导航迁移通过后才可宣称其 foundation gate 完成；依赖 provider 的行由对应票接线与验收，#245 汇总全部未决行并禁止以 case 数量替代实际覆盖。

### #237 当前 replacement 消费者入口

`foundation::zorite_foundation_native_wrapped_pointer_navigation_unicode_history` 恢复基础 native wrapped-row Down、行首指针/input/Undo、source Home/End/Unicode grapheme selection 与 word movement；`foundation::zorite_foundation_complete_extreme_line_paint_caret_input_save_undo` 使用完整 2,523,166 B fixture，分别在 live/source 的首尾执行 native pointer exact-offset、shift selection、输入、保存与 Undo。它们与 shared Host、composition owner close、完整长代码和 input-free distant-jump 的消费者同处当前六项 native registry。最后一项仅递送产品实际请求的后续帧，验证跳转后非 caret 行的字形几何、指针输入、精确保存与 Undo，不以强制 redraw 掩盖缺帧。

MDX 的 `source_only` 是 presentation 能力，分屏复制该约束；`ToggleDocumentPreview` 无法进入 live，状态栏模式按钮显示源码且禁用。workbench 回归 `mdx_source_only_survives_mode_action_split_history_save_reopen` 验证这一路由与 BOM/混合行尾/未知 JSX/attached-formula 原文、输入/Undo、保存/重开。源文编码策略不会因文档另存为或重命名自动转换。

### 基础消费者回归约束

工作台快捷键明确匹配 native `Editor` context，不借用 `Input` context 让普通输入框截获 native 按键。对外 Kit Backspace/Delete 与 editor 自身按键调用同一修改路径。分屏复制文档缩进宽度；多行缩进是保留选区的有序 batch，不替换被选正文。Host 命令的 before/after selection 纳入相同 history，Undo 恢复原选区；行重排保留原行尾且在移动无终止换行的片段时补齐操作必需的连接换行。

重复 occurrence 选择是 view-local 状态，工作台 Next/All 经真实 native selection、quads/carets 与原子 batch 输入执行；不是仅供测试的 Code-only no-op。晚到 reload 若遇到 source revision 或 composition 变化，保留编辑并显示 conflict，而非静默覆盖或仅丢弃结果。以上修复仍须通过对应消费者与 native 门禁，不由本说明推定通过。

### #240 多视图工作台消费者

`src/ui/code_editor/workbench_views.rs` 经生产 open/split/focus/actions/search/outline/save/close/reopen 路径覆盖来源 presentation 上下文、跨 view selection rebase、IME owner handoff、迟到 preedit/commit/unmark 隔离、关闭一组后的共享 Undo，以及第二 view 保存期间切换标签页后的精确字节。MDX 重命名覆盖每个 view 的 source-only 与原历史；普通 code 不因扩展名变化隐式转换编辑器。

native registry 增加 `workbench_views::shared_context_and_ime_handoff` 与 `workbench_views::focused_consumers_and_layout`。后者覆盖实际 focus 后无中间 draw 的菜单跳转、状态行列、outline、搜索后焦点返回，以及 light/dark、界面 zoom、Dock/Fill、窄窗口回退与放宽。原 `document_column::gpui_tests::docked_editor_content_fills_its_column_across_scales_and_resize`、`close_gpui_tests` 与 root status/chrome 消费者仍保留原有效合同，不以新测试替代。

仅明确已退役且失焦的 IME lease 拒绝迟到 callback；仍拥有 composition 的合法失焦提交保持可用。`unmark_text` 只结束自己拥有的 lease，不能提交另一个 view 的新 preedit。以上消费者尚需集成者统一执行，字体 runner 不替代真实系统 IME、实窗 geometry 或 macOS 验收。


### Reload 与 native 长代码基础门禁

`reload_request` 单独标识最近一次真实读请求；`reload_seq` 继续标识源文适用性。普通编辑不冒充“已启动另一读请求”：晚到的仍属当前Host/path的读会报告 conflict，既不覆盖新preedit，也不在观察到revision变化前静默return。真正的新reload、context失效、路径移动/删除仍使旧request失效。

原 `source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics` selector 已迁移到当前native registry的 `foundation::native_long_code`。保留完整 `"wide ".repeat(80_000)` fenced-code fixture与覆盖完整400,000B正文的离屏诊断，通过真实字体/paint/pointer、输入、保存、Undo、精确字节与重开验证；未保留旧projection/spelling内部接口，未缩短fixture。其余provider相关几何边界仍按上方逐项归属完成，新增case不代表这些边界自动通过。

### Windows 基础实窗验收

固定参考为上述 Zorite commit 的原始 Release binary。当前无探针 Lyrel Release 已在隔离配置中完成普通 991 B 和完整长文 722,000 B 的输入、字符/词导航、选择替换、剪切粘贴、live/source、首尾滚动、保存关闭重开，以及各 100 次结构编辑的完整 Undo/Redo；保存结果逐字节核验。系统中文 IME 通过实际候选窗完成 `ni'hao` 提交“你好”、`zhongwen` 取消与 Unicode 选区替换/Undo/Redo，不以文本注入替代。

长文初测发现 prepaint 两段逐行命中区准备耗时约 0.83–0.91 秒/帧；修复为复用现有行索引后，100 次结构输入不再积压。最终相同长文工作负载的进程 CPU 增量为 4.875 秒，private bytes 从 171,372,544 增至 345,698,304；保留全部整串历史，没有截断文本或缩短 Undo/Redo。此完整 Windows 基础工作负载判定可用；内存的线性历史成本已记录，不宣称零拷贝历史或任意大小性能。自动化调用耗时不是 input-to-paint 延迟，进程内存也不是历史独占内存。

真实 loopback SSH Host 的 BOM/混合行尾编辑、保存关闭重开及本地同名诱饵隔离已有独立实窗与精确字节证据；后续改动只涉及 GUI 几何，不将早前 SSH 结果冒充最后 binary 的重跑。macOS 没有本机执行前提，未实测；Windows 与 Linux headless 不代替 macOS 验收。上述基础结论不覆盖后续图片、结构公式、Mermaid、宽表资源整合，也不是 #244/#245 的完整视觉一致或最终接受结论。

### #239 表格与代码原生验收路径

`markdown::table_code_tests::workflow` 通过生产 owning Host 打开 BOM/混合行尾文档，实际操作 table commands、code language/copy、键盘尺寸选择与自定义 picker，跨结构替换/Undo，以及两个 presentation 的共享 history/保存关闭重开。native registry 额外保留上述两项原表格 selector；完整 wide-table 在测试中生成，不依赖仓库外的临时目录、不截断正文、不切 source 掩盖布局。窄验证入口为 `cargo test --locked -p lyrel --lib table_code_workbench_controls_history_host_reopen` 与 `cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown -- table`。源码提交未运行 build/test/lint/formatter；实际 Windows 产品窗口、固定 Zorite 表格/code 对照、系统 IME、真实 SSH 与 macOS 证据仍由集成者逐项记录，基础历史证据不自动转移。

