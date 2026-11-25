# 失焦时由上游接管标准输入框的 IME 组合结束策略

- 状态：accepted

## 背景与决策

`gpui-kit 0.11.0`（[#4032](https://github.com/longbridge/gpui-kit/pull/4032)）为 `InputBaseState` 实现了失焦时 `EntityInputHandler::accepts_text_input` 返回 `false`，交由平台后端（如 Windows IMM32）在失焦后完成当前组合。此前 [ime.rs](../../src/ui/ime.rs) 通过应用层 `RootPlugin` 拦截 `InputState`、`TextareaState`、`EditorState` 与自研 `MarkdownEditor` 的焦点切换，并在 Windows 主动调用 `CPS_CANCEL` 丢弃未提交组合。

本次升级选择**跟随上游设计**：从 [ime.rs](../../src/ui/ime.rs) 移除对 `InputState`、`TextareaState`、`EditorState` 的失焦拦截与主动丢弃，统一交由上游 `accepts_text_input` 机制处理；仅保留针对自研 `MarkdownEditor`（不属于上游 `InputBaseState`）的失焦 `unmark_text` 与 Windows 原生取消逻辑。

## 取舍与后续验证

- **收益**：消除应用层对框架输入框的重复 IME 钩子与平台特化分支，与 `gpui-kit 0.11.0` 保持一致。
- **语义变化**：框架输入框在失焦时不再应用层丢弃未选词拼音串，而是遵循上游及系统默认结束/提交语义；若从输入框直接切换至另一输入框，两端均具备焦点接收能力时依赖平台 IMM32 切换行为。
- **验收提示**：上游 [#4032](https://github.com/longbridge/gpui-kit/pull/4032) 合入时未在 Windows 真机完成实测，升级后须在 Windows 原生实窗手工核验中文输入法在「输入框 ↔ 输入框」「输入框 ↔ 终端窗格」「Markdown 编辑器 ↔ 其他控件」间的切换行为。
