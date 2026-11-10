# Markdown 编辑器 Host 能力清单

## 当前交付边界

本清单是 #236 的实现契约，不是验收通过声明。#237 提供可执行的文件生命周期与共享文档基础；其行为测试、原生字体、系统 IME、真实 SSH、Release 可用性与平台结果必须分别记录。后续能力按下表完成；依赖 #237 的票不得以独立文本控件演示替代真实工作台。最终完整功能与默认路由验收属于 #245。

源码基准固定为 [Zorite `9f069c73a56b06efc84fb7abb700beb50083c827`](https://github.com/packetThrower/zorite/tree/9f069c73a56b06efc84fb7abb700beb50083c827)。原始文件摘要、版本和适配范围见 `UPSTREAM.toml`；MIT 原文见 `licenses/`。导入限于 editor/bidi，不包含 GPL 应用编排。公式 GUI 导入由 #241 完成。GPUI 只经 `gpui-kit` 的 Fast 类型，不建立第二个 GUI runtime。

## 所有权与修改协议

- `lyrel_markdown_editor::Document` 独占文档身份、LF/UTF-8 原文、逐行原始行尾、格式策略、history 与单调 revision。
- `EditorState` 是 presentation：选择、方向、focus、scroll、marked range、layout/geometry cache 都独立。其 `Arc<String>` 是当前不可变 revision 快照，不是第二份可编辑权威。普通编辑不以 `set_text` 或整篇 diff 同步其他视图。
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
| 最小真实行列、go-to-line | Lyrel 工作台，#237 | 状态来自当前 presentation 的共享源文；Unicode 跳转、选择与变更后的行列一致，不为查询复制全文。 |
| 本文宽度、字体层级、段距、padding、theme、zoom、disclosure 占地 | 上游排版 + Lyrel shell，#244 | 样式接入在 #237；最终对照固定参考的 ordinary 文档，在窄/宽实窗连续书写、导航和滚动。#237 不将未对照的样式配置称为视觉一致。 |
| 快捷键、caret-local 标记显露、选择、列表/任务/标题/callout | 上游保留，#244 | `bind_keys` 使用当前 Kit 类型；基础输入由 #237 验证。完整格式/折叠/转换/连续编辑与 Zorite 对照归 #244。旧“光标处仍隐藏全部标记”约束已被替代。 |
| 插入/slash menu、construct picker、table insertion | Lyrel Host + 上游命令，#239/#244 | #239 完成真实 table picker 与结构编辑；#244 完成通用插入/slash 和文档上下文。不能把源文插入 API 当成已提供 picker。 |
| 表格行列、对齐、样式、导航、resize、code fence 语言/复制/高亮 | 上游保留 + Lyrel Host，#239 | native hit-testing、跨单元格输入/Undo、完整宽表；安装真实 code highlighter，验证 code language/copy 控件。 |
| 文字/高亮预设颜色与自定义颜色 | 上游保留 + Lyrel Host，#244 | 预设源文操作已导入；自定义 picker 待接线。仅在真实 Host 订阅 `PickColor` 后启用 `set_host_custom_colors(true)`；取消、提交和 Undo 均验证。 |
| 普通相对/外部链接、hover、caret 附近的源码访问 | 上游行为 + Lyrel Host，#238 | 采用 Zorite 的普通单击打开语义，不保留旧 Ctrl+Click-only。在真实 OpenLink/HoverLink 接线前不启用 `set_host_link_navigation(true)`，不显示无效手形控件。相对路径绑定文档 Host，延迟完成不重定向活动组；失败可见且不丢编辑。 |
| 图片插入/paste/drop、资源、preview、resize/zoom | 上游保留 + Lyrel Host，#238 | 当前无伪造 bitmap/provider；源码 intact。完成 owning local/SSH Host 读写、唯一资源命名、可见失败、延迟资源隔离、输入/Undo/save/reopen 与 native 预览几何。 |
| inline/display math 渲染、二维结构编辑 seat/menus/commit/cancel | 指定 MIT 公式 GUI 导入 + Lyrel Host，#241 | #237 不导入公式 GUI。渲染 provider 未配置时允许原文直接编辑，不让 arrow/click 被未接线结构编辑事件吞掉。#241 安装真实 renderer 与结构编辑器，并经同一 Document 修改协议提交。 |
| Mermaid cache/render/entry/refresh/error/stale-result | Lyrel Host，#242 | 源码可访问；#242 完成真实 native 图、错误/恢复与布局变化，不以 parser 成功或无限 loading 冒充可用。 |
| status/search/outline、tabs/groups、MDX source-only、focus/layout | Lyrel 工作台，#240 | #237 已建立所有权和最小状态/跳转；#240 完成完整导航/search/outline 与多视图布局消费者。MDX 不恢复旧 rich component 系统。 |
| 编码/BOM、显式换行转换/cleanup、save-as/conflict/failure/close | Lyrel 工作台 + `lyrel-text`，#243 | 混合行尾事务基础属 #237；#243 扩充 supported encodings、失败恢复、真实 local/SSH 和在途第二视图快照。不得再复制 codec 到 GUI crate。 |
| extreme nowrap 行、完整源文/尾部 caret/input/Undo/save | 上游布局 + Lyrel 安全保护，#237/#244 | 实际 native 宽度超过 65,536 逻辑像素时保留有界视觉折行；不插入源换行、不截断、不更改 wrap 偏好。#235 的 not_planned 不是 SDK 修复证据。 |
| imported bidi 映射 | 上游保留，#237/#244 | 保留 `bidi/` 与 source/display/native position mapping。完整 RTL 仍在已批准验收范围之外，不能由普通 Unicode 成功推导 RTL parity。 |

## 明确排除但保留源码

| 构造 | 行为与负责人 |
| --- | --- |
| notebook/database、页面管理、知识库搜索、backlinks、wiki 页面、block-reference 浏览 | 不导入 GPL app 或建立 notebook。`[[...]]`、`![[...]]`、`#^...` 及相关 source 均原样保留；#238 区分普通文档链接和这些排除构造，不留下无目标导航。 |
| 上游 property/wiki 渲染表面 | 不等于旧 Lyrel inspector 的迁移要求。property provider 未安装时原文直接显示/编辑；`SyntaxStyle::property_icon` 为 `None` 时不把 property 源文折成不可编辑面板。#244 对照与确认这些排除表面。 |
| 旧 Lyrel MDX rich components、visual inspectors、自定义 workspace references/embeds、额外 visual extensions | 旧实现与专属消费者随 cutover 删除，不能以 test-only adapter 继续保留。MDX 使用 source-only；frontmatter 和未知语法始终 intact/source-accessible。负责人 #240/#244，最终文档 #245。 |

## 测试与证据入口

- editor 回归：`cargo test --locked -p lyrel-markdown-editor --test foundation`。
- GUI-free codec：`cargo test --locked -p lyrel-text`。
- 工作台：`cargo test --locked -p lyrel --lib`；`src/ui/code_editor/markdown/foundation.rs` 经生产 lifecycle 运行 shared Host 与 composition 场景，不依赖旧 Markdown 内核。
- 真正 native 字体目标：`cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown -- --nocapture`。headless `TestAppContext` 不能替代系统中文 IME 或真实 SSH。
- Linux：`cargo test --locked --workspace --exclude lyrel --exclude lyrel-markdown-editor --all-features`；`lyrel-text` 可参与，GUI crate 不被选入 headless 范围。
- Release 输入/导航/scroll/Undo-memory、系统 IME、真实 SSH 与跨平台结果由集成者保存精确 commit、固定 fixtures、执行序列、display 条件、命令、退出码和未验前提。早先实现的历史验收不自动转移到本源码导入。

## 最终许可证分发验收（#245）

#237 的 editor/bidi MIT 原文同时保存在 crate 的 `licenses/` 与现有打包入口携带的根 `LICENSE`；源码目录不等于实际产物验收。#245 须对 Windows ZIP/安装包、macOS app/archive 和 release artifacts 核验 editor/bidi 与 #241 新增 ratex 的实际许可证内容。检查入口包括 `.github/scripts/bundle-windows.ps1`、`bundle-macos.sh`、`verify-windows-package.ps1` 的根 allowlist，以及 `.github/workflows/release-build.yml` 的 artifact 白名单；不能只增加源树许可证而漏发二进制归属。

## 原 14 项 native 消费者迁移清单

以下以 baseline `5a38d0a9242761024335fe06263b040ddb3f5d74` 的真实 registry 为准，名称省略共同 `ui::code_editor::markdown::` 前缀。旧文件依赖已删除的 Rope/SourcePresentation/投影与旧格式菜单；不编译旧内核来让测试继续绿。新基础的三个 case **不等价于原 14 项全通过**。后续票必须在真实 replacement 工作台恢复其仍有效的用户行为与几何边界；仅已明确替代的交互或排除的旧 UI 可以退役。

| baseline selector 后缀 | 处理与仍有效的验收边界 | 所属票 |
| --- | --- | --- |
| `source::review_tests::native_down_visits_each_soft_wrapped_row_without_skipping` | 迁移：新 native navigation 消费者逐视觉行 Down/Up，不允许跨行跳过。当前基础三 case 不声称覆盖。 | #237 |
| `source::review_tests::first_soft_wrapped_link_grapheme_hovers_edits_and_opens` | 保留行为，随新 Host link/hover 接线迁移；首个 wrapped grapheme 的命中/输入/open。 | #238 |
| `source::review_tests::fragment_tail_link_grapheme_hovers_edits_and_opens` | 保留行为，迁移分片末尾 glyph 的 hit-testing、编辑和打开，不按旧投影内部类型断言。 | #238 |
| `source::review_tests::native_soft_wrap_caret_keeps_clicked_side_across_navigation_and_input` | 迁移：真实指针点击 wrap 两侧，箭头/输入/Undo 后 caret 不跳至另一侧。 | #237 |
| `source::review_tests::native_modified_link_drag_retains_upper_wrap_anchor` | 旧 Ctrl+Click-only 激活契约退役；wrapped drag 选择锚点正确性仍保留，按 Zorite 普通单击链接和实际扩选方式验证。 | #238 |
| `source::review_tests::native_extreme_nowrap_line_keeps_pointer_selection_and_source_exact` | 迁移至 `foundation::zorite_foundation_complete_extreme_line_paint_caret_input_save_undo`；完整 2,523,166 B，不缩水为截断样本；保留指针/选择和原始源码条件。 | #237 |
| `source::review_tests::native_aligned_table_pointer_and_selection_follow_content` | 表格仍有效：新列宽/居中/右对齐 native glyph 与 selection/hit-test 同位置，Undo 后一致；不是基础三 case 覆盖。 | #239 |
| `tests::table_presentation::table_presentation_drag_fit_scroll_edit_history_and_host_reopen` | 表格完整 drag/fit/scroll/edit/Undo/Host 保存重开迁移到新表格 UI；旧 inspector/menu id 不保留。 | #239 |
| `tests::native_navigation_home_end_and_words_use_visible_logical_content` | 迁移新 navigation case 的 Home/End/Unicode word/grapheme，删除仅为旧 Rope chunk seam 的实现断言。 | #237 |
| `tests::native_text_color_picker_clear_geometry_and_shared_history` | 自定义色/取消/clear/geometry/history 仍有效；按 Zorite native menu + Lyrel picker 迁移，不重建旧全行格式工具栏。 | #244 |
| `source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics` | 保留超长 code 与离屏 diagnostics 的 caret/input/Undo 安全；旧 spelling UI id 不保留，新 pipeline 的 diagnostics 渲染另验。 | #237/#239/#244 |
| `source::spelling::tests::spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms` | 混合段落/高原子几何仍有效；图片/公式 provider 完成后在真实 renderer 下迁移，不能拿源码字符串通过替代。 | #238/#241 |
| `source::spelling::tests::spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom` | 保留 prose/heading/list/quote/zoom 后视觉行首与命中一致；按新排版层级迁移。 | #244 |
| `source::spelling::tests::spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows` | hard-break/图片/公式片段不能跨错行的边界保留，分别在真实资源与公式 renderer 连接后验证。 | #238/#241 |

这不是允许后续丢弃仍有效边界的豁免。#237 自有基础几何/导航迁移通过后才可宣称其 foundation gate 完成；依赖 provider 的行由对应票接线与验收，#245 汇总全部未决行并禁止以 case 数量替代实际覆盖。

### #237 当前 replacement 消费者入口

`foundation::zorite_foundation_native_wrapped_pointer_navigation_unicode_history` 恢复基础 native wrapped-row Down、行首指针/input/Undo、source Home/End/Unicode grapheme selection 与 word movement；`foundation::zorite_foundation_complete_extreme_line_paint_caret_input_save_undo` 使用完整 2,523,166 B fixture，分别在 live/source 的首尾执行 native pointer exact-offset、shift selection、输入、保存与 Undo。它们与 shared Host、composition owner close 两个生命周期 case 同处当前 native registry；运行结果仍由实际候选门禁决定，不从注册成功推定通过。

MDX 的 `source_only` 是 presentation 能力，分屏复制该约束；`ToggleDocumentPreview` 无法进入 live，状态栏模式按钮显示源码且禁用。workbench 回归 `mdx_source_only_survives_mode_action_split_history_save_reopen` 验证这一路由与 BOM/混合行尾/未知 JSX/attached-formula 原文、输入/Undo、保存/重开。源文编码策略不会因文档另存为或重命名自动转换。

### 基础消费者回归约束

工作台快捷键明确匹配 native `Editor` context，不借用 `Input` context 让普通输入框截获 native 按键。对外 Kit Backspace/Delete 与 editor 自身按键调用同一修改路径。分屏复制文档缩进宽度；多行缩进是保留选区的有序 batch，不替换被选正文。Host 命令的 before/after selection 纳入相同 history，Undo 恢复原选区；行重排保留原行尾且在移动无终止换行的片段时补齐操作必需的连接换行。

重复 occurrence 选择是 view-local 状态，工作台 Next/All 经真实 native selection、quads/carets 与原子 batch 输入执行；不是仅供测试的 Code-only no-op。晚到 reload 若遇到 source revision 或 composition 变化，保留编辑并显示 conflict，而非静默覆盖或仅丢弃结果。以上修复仍须通过对应消费者与 native 门禁，不由本说明推定通过。

### Reload 与 native 长代码基础门禁

`reload_request` 单独标识最近一次真实读请求；`reload_seq` 继续标识源文适用性。普通编辑不冒充“已启动另一读请求”：晚到的仍属当前Host/path的读会报告 conflict，既不覆盖新preedit，也不在观察到revision变化前静默return。真正的新reload、context失效、路径移动/删除仍使旧request失效。

原 `source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics` selector 已迁移到当前native registry的 `foundation::native_long_code`。保留完整 `"wide ".repeat(80_000)` fenced-code fixture与覆盖完整400,000B正文的离屏诊断，通过真实字体/paint/pointer、输入、保存、Undo、精确字节与重开验证；未保留旧projection/spelling内部接口，未缩短fixture。其余provider相关几何边界仍按上方逐项归属完成，新增case不代表这些边界自动通过。
