# 五种 coding agent 的 lyrel 集成能力调研

调研日期：2026-10-18。

## 问题定义与证据边界

拟精简后的智能体范围只有 **omp、Kimi Code、pi、OpenCode 2.x、Hermes**。本研究回答：能否在保留原生交互界面的前提下，补齐识别、启动、带任务启动、运行／等待状态、会话 ID、历史列表、恢复和分叉；尤其不能把“能拼出启动命令”当成端到端支持。

本轮只读调查官方文档、上游源码、正式发布信息及 lyrel 当前源码；唯一仓库写入为本研究文档。未修改生产代码或用户配置，未安装、启动任何智能体，未运行 build、lint、测试或 formatter，**没有真实端到端验证**。

本文是实施前调研快照，下文“当前源码／差距”指调查时状态，不是当前实现。现行操作与验收边界以[会话契约](../agents/sessions.mdx#acceptance-status)为准；接口存在、代码已合并、契约测试通过与真实 GUI／智能体验收分别记录。

- **已核实**：能从所引源码／协议／正式发布信息直接确认。
- **[INFERENCE]**：根据已核实接口设计的适配方案、静态风险分析；不代表在 lyrel 实测通过。
- 对“没有接口”的判断限定于已检查的公开事件表、CLI 定义和源码；不以索引未命中证明全仓不存在。
- `pi` 专指 `earendil-works/pi`；`omp` 专指 `can1357/oh-my-pi`，两者不是同一产品，也不能长期共用未经核实的事件契约。

## 结论与五种能力矩阵

**状态、历史、恢复与原生分叉这组核心闭环大部分有官方适配路线，但目前不能宣布五种都已完整支持，也不能保证所有能力有同一种接法。** pi／omp 已有可复用基础；Kimi Code 原生分叉可接、状态时序仍需修正；OpenCode 2.x 当前实质不兼容，应改用本地 TUI 插件做共享服务下的视图绑定并迁移接口；Hermes 可补状态、历史和恢复，但外部新 pane 分叉还需要接通官方会话协议，而不是拼启动旗标。

“带任务启动”是另外单列的扩展验收边界，不应混淆核心闭环的可行性。Kimi 暂无本轮已验证的原生交互预填入口；这不等于整个产品永远做不到，但不能用 print 模式、自动模拟按键或替换 TUI 偷换需求。

### 上游原生能力（本轮核实的版本）

本表“有”仅表示上游接口存在；不是 lyrel 已实现，也不是端到端验收结果。全部关键依据见各产品章节的版本化链接。

| 产品／版本 | 交互初始任务 | 状态／等待接口 | 会话历史／恢复 | 原生分叉 |
| --- | --- | --- | --- | --- |
| omp 18.12.1 | 初始 message；明确 launch／`--` 解析边界 | 扩展生命周期、续跑标志、工具审批／计划审阅 | JSONL、有效 profile／目录；`--resume` | `--fork <session>`，含 artifacts；与 no-session 不兼容 [O2–O6] |
| Kimi Code 2.2.0 | **无保留 TUI 的 CLI 预填参数**；`-p` 是非交互 | 正式 hooks；审批回复、提问、后台 turn、Stop 可阻断均需保留语义 | state.json／`session list --json`；`--session` | `kimi fork <id>`；返回新 ID 后另启动 TUI；stdout 非 JSON [K6–K15] |
| pi 1.3.0 | positional message；TTY 保持交互 | `agent_settled`、扩展 UI prompt；无统一内建权限系统 | JSONL v3；`--session`，注意跨项目确认／目录覆盖 | `--fork <path>` 或 `--fork <id>`；CLI 整树复制，与会话内选路径分叉不同 [P3–P10] |
| OpenCode 2.0.32 | 根命令 `--prompt` | TUI 本地 data／官方 client；execution、permission、form | session API／CLI；`--session`；当前 SQL 已换 v2 | 正式 `session.fork`；根 TUI 无 `--fork`，可经自带 `api` 或 TUI client [C10–C24] |
| Hermes v0.22.2 | TTY 上 `-q/--query` 提交首轮且保持交互 | shell／plugin observer hooks，人工输入和结束结果 | sessions CLI／有效 state.db；`--resume` | 会话内 `/branch`，或 TUI `session.branch`／ACP；无启动 fork 参数 [H3–H12] |

### lyrel 当前覆盖与实际差距

“已有”只指当前源码有接线；不隐含这些接线在本次实际运行过。带任务启动一列指现有工作树 Task／`prompt_args` 接口，不等于用户不能手工在终端输入任务。

| 产品 | 识别／裸启动 | 带任务启动 | 状态／pane 绑定 | 历史／恢复闭环 | 新 pane fork |
| --- | --- | --- | --- | --- | --- |
| omp | 已有 | 未接原生 message | 已有，但续跑／异步结束与审批语义需修正 | 默认路径已有；profile／覆盖目录与会话切换不足 | 已有原生命令；仍需真实验收 |
| Kimi Code | 已有；须区分 legacy／无依据别名 | 未接；不能改用 `-p` 冒充交互 | hooks 格式基本对齐；审批结束／提问／后台 turn 时序仍不足 | 默认结构大体对齐；远端 home 与 fork ID 捕获需补 | 未接原生 `kimi fork` 的两阶段流程 |
| pi | 已有 | 未接 positional message | 主通道已有；最终态回退和 UI 覆盖边界需收紧 | 大体已有；摘要字段／目录覆盖有缺口 | 未接已发布原生 `--fork` |
| OpenCode 2.x | 仅旧 `opencode` 名称，无 `opencode2` | 未接 `--prompt` | 共享服务主动禁用桥；当前执行事件／表单不兼容 | 查询旧 SQL；有恢复命令不代表能发现新会话 | 旧根命令 `--fork` 字符串与 2.0.32 不兼容 |
| Hermes | 已有 | 未接 TTY `-q` | **无 HookAgent／状态桥** | **无历史扫描／ID 发现**；仅有正确恢复命令 | 未接官方分支协议 |

### 推荐实施范围与顺序

1. **先保证既有承诺真实**：修正 OpenCode 2.x 识别／事件／历史／分叉路径；修正 omp／pi 最终态、Kimi 审批结束及后台 turn 等语义；明确错误、中断不是成功。不要只做白名单精简后沿用“均已支持”的文案。
2. **补原生现成入口**：pi 使用 `--fork`；Kimi 使用 `kimi fork → 验证返回ID → --session`；omp 保留原生 fork。带任务启动另接 omp／pi／OpenCode／Hermes 的原生交互参数；Kimi 该项暂无已验证的原生交互入口，另需插件／协议可行性与真实时序验证，不捏造 CLI 参数，不默认模拟按键。
3. **OpenCode 保留共享服务**：首选本地 TUI 插件获取本客户端 route／tabs，再按 session ID 路由状态；`--standalone` 仅为用户可选私有模式。分叉优先复用已连接 client，或自带 `opencode api`，不强加外部 Node／Bun helper。
4. **Hermes 补完整状态／历史／恢复链路**：在明确版本、hook 同意和平台前提下接 shell／observer hooks；对 main 的共享 gateway 迁移单列兼容策略。若要承诺新 pane fork，再完成官方协议连接／新 ID／新 pane 的真实闭环，不能用 `/branch` 文本注入算完成。
5. **统一验收而不是统一伪协议**：五种都验证同一组用户行为，但内部按各自官方接口适配。目录、profile、远端 Host、安全模式、客户端断线和多 pane 并发都进入验收范围。

## 当前 lyrel 基线

本节依据工作区静态源码，而不是文档承诺：

- `crates/lyrel-core/src/core/cli_agent.rs`：五种均有名称识别、快捷启动及恢复命令构造；其中 `OpenCode` 仅识别 `opencode`，尚未包含正式 2.x 的 `opencode2` 别名。五种里，现有 `fork_command` 只为 omp、OpenCode 返回命令；这不等于当前上游版本可执行。
- `crates/lyrel-core/src/core/agent_hooks.rs`：omp、pi、Kimi、OpenCode 已有桥接；Hermes 没有 `HookAgent`。OpenCode 的 `setup` 遇到 `--service` 主动返回，避免把共享服务冒充单个 pane。
- `crates/lyrel-core/src/core/agent_history.rs`：扫描前四种，缺少 Hermes；omp 只扫描默认目录；OpenCode 查询旧 `session`／`message`／`part` 表。
- [`docs/agents/sessions.mdx`](../agents/sessions.mdx) 的“除 Aider 外均可恢复”比实现宽，不能据此证明 Hermes 已有会话发现／绑定闭环。本轮不改该现有文档。
- 远端能力应区分原生桌面远程开发工作区与本地 pane 内的 SSH／WSL 上下文。`src/ui/app.rs` 对 `view.remote_context().is_some()` 的 pane 禁止 fork，但另有远端工作区创建 pane 后拨号执行路线；不能笼统写成“全部远端工作区不能 fork”。

## omp：可沿用原生恢复／分叉，但事件和 profile 仍需修正

### 原生能力与版本

已核实正式 npm `@oh-my-pi/pi-coding-agent` 为 **18.12.1**；本节以 `v18.12.1` 源码／文档为主，另核对 main 快照 `ba85aef0b343a25f3c90b4f86628a9bd92d8414d`，不把 main 独有能力当已发布能力。[O1–O4]

- 原生 `--resume [id|path]` 和 `--fork <session>`；fork 由智能体处理会话及 artifacts。`--no-session` 不可恢复／分叉，是明确的使用模式限制。[O2–O3]
- 交互初始 message 已原生支持；可使用明确 `launch` 子命令和 `--` 终止选项／子命令解析，避免任务文本被误当命令。lyrel 的 `prompt_args` 尚未接入。[O2]
- 扩展公开 `session_start`、`session_switch`、`session_branch`、`agent_start`、`agent_end`、`session_shutdown`，以及审批／计划审阅事件；`agent_end` 带 `willContinue?`、`awaitingAsyncWork?`。[O4–O5]
- named profile 使用 `~/.omp/profiles/<name>/agent/extensions`；`PI_CONFIG_DIR` 可改根目录，`PI_CODING_AGENT_DIR` 只影响 default profile。[O6]

### lyrel 差距与路线

1. 保留现有原生 `omp --resume`／`omp --fork` 路线；不手工复制 JSONL 冒充分叉。
2. 现共享扩展模板对每次 `agent_end` 直接 `stop`，并尝试订阅 `agent_settled`；但已检查的 omp 正式公开事件表没有 `agent_settled`、`ui_prompt_start/end`。**[INFERENCE]** 自动续跑、异步工作未结束时可能提前标记完成；不是本轮运行复现的 bug。
3. 按 `willContinue`／`awaitingAsyncWork` 的正式语义适配结束状态，并处理 `tool_approval_requested/resolved`、`plan_review_requested/resolved`；不要假装 pi 与 omp 的 UI 事件完全相同。
4. 扩展监听 `session_switch`、`session_branch` 后立即重报新会话 ID，不能等下一次 `agent_start` 才更新。
5. 安装位置与历史目录应按有效 profile、启动环境与 session-dir 解析，不能只覆盖 `~/.omp/agent`。

### 来源

- [O1：npm 正式包](https://registry.npmjs.org/@oh-my-pi%2fpi-coding-agent/latest)。
- [O2：v18.12.1 CLI 参数](https://github.com/can1357/oh-my-pi/blob/v18.12.1/docs/cli-reference.md)。
- [O3：v18.12.1 恢复、分叉与导出](https://github.com/can1357/oh-my-pi/blob/v18.12.1/docs/session-operations-export-share-fork-resume.md)。
- [O4：v18.12.1 AgentEndEvent](https://github.com/can1357/oh-my-pi/blob/v18.12.1/packages/coding-agent/src/extensibility/shared-events.ts#L210-L230)。
- [O5：v18.12.1 扩展事件契约](https://github.com/can1357/oh-my-pi/blob/v18.12.1/packages/coding-agent/src/extensibility/extensions/types.ts)。
- [O6：v18.12.1 扩展加载／profile 规则](https://github.com/can1357/oh-my-pi/blob/v18.12.1/docs/extension-loading.md)。

## Kimi Code：新旧产品必须区分，原生 fork 可直接接通

### 身份、发行版与现有兼容面

现行官方产品为 **`MoonshotAI/kimi-code`**，TypeScript，正式 npm **`@moonshot-ai/kimi-code@2.2.0`**（2026-10-14），实际可执行名只有 `kimi`。旧 Python `MoonshotAI/kimi-cli` 已归档，并明示被新产品取代；旧 PyPI `kimi-code` 是 legacy 别名，不是新 CLI。Homebrew formula 名 `kimi-code` 也不等于可执行名。[K1–K3]

以下 fork 命令已回查 npm 2.2.0 发布物；实现细节另回查当前 main 快照 **`4a82a07f08d0b135c4349c20aae53a2d51e5c3fa`**，区分发布证据与当前源码，不将未核实的 main 细节自动提升为稳定契约。

- 新产品有效目录为 `$KIMI_CODE_HOME`／`~/.kimi-code`，配置 `config.toml` 的 `[[hooks]]`，会话位于 `sessions/<workDirKey>/<sessionId>/state.json`。[K4–K5]
- 恢复为 `kimi --session/-S <id>`，隐藏别名 `--resume/-r`；`--continue` 与 `--session` 互斥，agent／agent-file 也与恢复选项冲突。lyrel 当前 `kimi --session <id>` 正确。[K6]
- 交互初始任务不能套用旧 Python CLI 语法：2.2.0 的 `-p/--prompt` 是非交互一次性模式，当前 `validateOptions` 强制 uiMode=`print`，正式 bundle 的根命令额外 positional 文本会报 unknown command。已检查的公开 CLI 没有保留 TUI 的启动预填参数；若要覆盖此能力，另需核实官方插件／客户端投递协议及真实就绪时序。本轮不把自动键入／粘贴、切换 print 或替换 TUI 当作已获授权的解决方案。[K6–K7、K15]
- state v2 实际有 id/cwd/title/lastPrompt/updatedAt/archived、forkedFrom 等，lyrel 当前默认目录扫描与大部分字段匹配；官方另有 `kimi session list --json`，可以避免长期绑定内部布局。[K5、K7]

### 原生 fork 与新 pane 路线

正式 **`kimi fork [sessionId]`** 至少已在 0.40.0、2.0.0、2.2.0 发布物中存在；官方 CLI 参考页尚未完整列出该命令，不能因为文档遗漏就说没有接口。`--cwd` 用于未给 ID 时查找最近会话，`-y/--yes` 跳过该选择确认；明确 ID 的 lyrel 动作不应默认回退成“最近一个”。成功输出 `Forked to <id> ...`，失败退出码 1。[K7–K8]

**[INFERENCE] 最小路线**：在源会话数据所在 Host 执行 `kimi fork <完整ID>`，检查退出码，提取实际新 ID，再在新 pane 启动 `kimi --session <新ID>`。这是两个阶段，不是仅给 `fork_command` 拼一个新字符串。CLI 返回目前是文案而非专用 JSON，解析必须限定版本／前缀，不能把文案变化当成功；如需结构化替代，再评估官方 SDK，不必先依赖实验性 web 服务。

当前源码的生命周期操作创建独立会话并写 `forkedFrom`，包含活跃 turn 拒绝（`SESSION_FORK_ACTIVE_TURN`）逻辑；**这不证明外部 CLI 对另一个运行中 TUI 的跨进程并发保护已验证**。失败时不能静默另开空会话绕过。TUI `/fork` 保留当前会话、不自动切换；REST `POST /api/v1/sessions/{id}:fork` 明确实验性，不是本地 CLI fork 的必要前提。[K8–K10]

**关键会话事件例外**：当前 `sessionExternalHooksService` 明确排除 source=`fork` 的 `SessionStart`，因此“订阅 SessionStart 就能发现所有新 fork”不成立。lyrel 自己发起 fork 应立即记录返回 ID；恢复打开新会话后的 hook 再校验绑定，不等待目录碰巧变化。[K11]

### Hooks、状态与兼容风险

当前公开 hooks 配置只接受 `event`、`matcher?`、`command`、`timeout?`，严格 schema 不接受随意附加 pane 字段。事件载荷为 stdin JSON，含 snake_case `hook_event_name/session_id/session_title/client_type/cwd` 与事件专有字段；CLI client_type 为 `kimi_code_cli`。[K12–K13]

当前 20 事件中，lyrel 使用的 `SessionStart`、`UserPromptSubmit`、`PermissionRequest`、`PostToolUse`、`Stop`、`Interrupt`、`StopFailure`、`SessionEnd` 均有依据，**但基本格式对齐不等于状态语义完整**。Interrupt 替代 Stop；StopFailure 是错误终止。仍需补审批结束 `PermissionResult`、系统／后台 turn 的 `TurnStarted`、`TaskStarted(kind=question)`、`PostToolUseFailure` 等时序覆盖并实测。**[INFERENCE]** 仅 PermissionRequest→waiting 而未处理回复，可能让授权后的工具运行仍显示 waiting，直到 PostToolUse；而 Stop hook 本身可阻断并继续，也不能直接等同最终 settled。这些是静态风险，不是本轮复现的 bug。[K12–K13]

已在正式 2.2.0 bundle 复核 20 事件／严格配置 schema，以及 fork 排除 SessionStart；这两项不是 main-only。相反，**hook cwd 修复 #4242 尚晚于 2.2.0**：agent 级 hook 在 2.2.0 可能使用 engine 启动目录，main 的 `c1459a61845056832b85a0a220ee11a2bc5132f8` 才改为会话工作目录；SessionStart／SessionEnd 原本走会话目录。不能用文档的理想 cwd 语义倒推所有正式版 hook 已一致。[K7、K16]

- hook 执行使用 `shell:true`，不能把 Kimi 工具执行用的 Git Bash 与 hook 的 shell 选择混为一谈；原生 Windows 实际解释器需要按 Host 实现验证，不能仅凭 Node 默认语义就写“已实测 cmd.exe”。[K14]
- 当前远端 hook 安装对 `KIMI_CODE_HOME` 的处理不完整，应按远端有效目录解析，而非拿本地环境变量决定远端路径。
- lyrel 注释“只有新版有 hooks”不实：legacy 也有另一套 13 事件／`~/.kimi` hooks。应明确仅支持新产品契约，而不是对旧 `kimi` 写新版配置。
- 不建议将没有官方 bin 依据的 `kimi-code` 自动当作产品可执行名；用户显式配置别名另论。
- 不手工改写 sessions 文件，不把 `--worktree`、新开空会话或 `/fork` 文本注入冒充已完成原生 fork。

### 来源

- [K1：新产品官方仓库](https://github.com/MoonshotAI/kimi-code)，[正式 npm metadata](https://registry.npmjs.org/@moonshot-ai/kimi-code/latest)。
- [K2：legacy 归档与替换声明](https://github.com/MoonshotAI/kimi-cli)。
- [K3：官方安装器](https://code.kimi.com/kimi-code/install.sh)，[Homebrew formula](https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula/k/kimi-code.rb)。
- [K4：数据位置](https://moonshotai.github.io/kimi-code/en/configuration/data-locations)，[环境变量](https://moonshotai.github.io/kimi-code/en/configuration/env-vars)。
- [K5：会话指南](https://moonshotai.github.io/kimi-code/en/guides/sessions)。
- [K6：当前 CLI 参数源码](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/apps/kimi-code/src/cli/options.ts)。
- [K7：正式 2.2.0 发布物（含 fork 与 session list 命令）](https://cdn.jsdelivr.net/npm/@moonshot-ai/kimi-code@2.2.0/dist/main.mjs)。
- [K8：当前原生 fork CLI](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/apps/kimi-code/src/cli/sub/fork.ts)。
- [K9：当前会话生命周期实现](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/packages/agent-core-v2/src/workspace/sessionLifecycle/sessionLifecycleService.ts)。
- [K10：实验性 server API](https://moonshotai.github.io/kimi-code/en/reference/server-api)，[slash 命令](https://moonshotai.github.io/kimi-code/en/reference/slash-commands)。
- [K11：当前 SessionStart 排除 fork](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/packages/agent-core-v2/src/features/externalHooks/session/sessionExternalHooksService.ts)。
- [K12：官方 hooks](https://moonshotai.github.io/kimi-code/en/customization/hooks)。
- [K13：当前 hook 配置 schema](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/packages/agent-core-v2/src/features/externalHooks/configSection.ts)，[事件类型](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/packages/agent-core-v2/src/features/externalHooks/internal/types.ts)。
- [K14：当前 hook 执行](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/packages/agent-core-v2/src/features/externalHooks/internal/runHook.ts)。
- [K15：当前根命令参数注册与 positional 校验](https://github.com/MoonshotAI/kimi-code/blob/4a82a07f08d0b135c4349c20aae53a2d51e5c3fa/apps/kimi-code/src/cli/commands.ts)，[官方 print 模式说明](https://moonshotai.github.io/kimi-code/en/reference/kimi-command)。
- [K16：晚于 2.2.0 的 hook cwd 修复 #4242](https://github.com/MoonshotAI/kimi-code/commit/c1459a61845056832b85a0a220ee11a2bc5132f8)。

## pi：原生外部分叉已发布，主要是小范围补齐而非自造协议

### 身份、版本与原生能力

对象是 **`earendil-works/pi`／pi.dev**，正式 npm 包 **`@earendil-works/pi-coding-agent@1.3.0`**（2026-10-15）；不是 omp。旧 `@mariozechner/pi-coding-agent` 已标 deprecated，不能把旧包停留版本当当前版本。[P1–P2]

- 交互启动支持 positional message；TTY stdin/stdout 下保留 TUI，`--` 可终止选项解析。`--print`／JSON／RPC 是另外的模式，不应为取得事件强制切换。[P3]
- 原生 `--session <path|id>`、`--continue`、交互选择式 `--resume`；`--session-id` 可指定新／已有项目会话 ID。跨项目 `--session` 可能提示是否 fork，不能当成无交互恢复协议。[P3–P4]
- 正式 `--fork <path|id>` 可直接从另一个进程创建新项目会话；可配 `--session-id <新ID>` 和 `--session-dir`。不能与 `--session/--continue/--resume/--no-session` 同用。[P3–P5]

**两种原生 fork 语义必须区分**：CLI `--fork` 调用 `SessionManager.forkFrom`，复制整棵条目树到新文件、生成新 ID，以 `wx` 防覆盖；交互 `/fork`／`/clone` 调用运行时分支操作，只复制所选路径及 label。lyrel 新 pane 的外部分叉应使用正式 CLI，不调用内部 `createBranchedSession` 或改写 JSONL 来模拟另一个语义。[P5–P6]

上游为新 fork 文件生成独立 ID 并防覆盖；但这不是源会话事务快照，活跃写入时可能缺最后未完整写入的记录，且源末行无换行时加载器会修复补换行，不能说“绝不修改源文件”。调查的相关会话文件中未发现排他锁，不是穷尽全仓的否定证明，更不是并发安全验证；不要默认让两个 pane 同时 resume 并写同一个原文件。分叉还会复制整树，需承认磁盘成本。[P5]

### 状态与等待接口

v1.3.0 正式扩展契约已含：[P7–P8]

| 语义 | 接口 | 限制 |
| --- | --- | --- |
| 当前 session ID | `ctx.sessionManager.getSessionId()` | `session_start` 包含 startup/reload/new/resume/fork 原因 |
| 开始工作 | `agent_start` | 重试可能再次开始 |
| 最终结束 | `agent_settled({ aborted })` | `agent_end` 后仍可能重试、压缩或排队继续 |
| 可行动结束边界 | `agent_before_settle` 的 completed/aborted/error | 与最终 settled 区分 |
| 等待扩展对话 | `ui_prompt_start/end`，select/confirm/input/editor/custom | 只覆盖包装的 `ctx.ui.*`，不能宣称所有内建 TUI 对话框都会报告 |
| 客户端退出 | `session_shutdown` | 不是每一轮完成 |

pi 没有内建统一权限系统；扩展 `ctx.ui.confirm` 和 `tool_call` block 是相关边界。不要注册 omp 专有 `tool_approval_requested`／`auto_retry_start` 后声称 pi 的审批／重试已覆盖；pi 对未知事件名不报错，只会永不触发。`agent_settled`、UI prompt 事件不是试验设想，已在正式版提供。[P7–P8]

### lyrel 差距与最小路线

1. 新增 pi 原生 `--fork` 到现有 fork 入口，并保留 `--session-dir`、去掉互斥旧会话 flags、尊重 `--no-session`；不用新 SDK／wrapper。
2. 现桥已监听 `agent_settled` 和 UI prompt，主通道可复用；不过首次见到 settled 前用 `agent_end` 回退，**[INFERENCE]** 首轮可能出现瞬时 Done 再续跑。对明确支持的版本直接采用正式最终态，兼容旧版则明示边界。
3. 默认 JSONL v3 历史已大体对齐，header 有 id/cwd/parentSession、条目有 id/parentId、`session_info.name` 和 `compaction.summary`。当前解析器的 `shortSummary` 回退字段与 pi 不同，应读正式 `summary`；这是标题回退缺口，不是整个恢复不可用。[P9]
4. 支持有效 `PI_CODING_AGENT_SESSION_DIR` 和 settings `sessionDir`；目前仅默认根／`PI_CODING_AGENT_DIR` 路线不足以枚举所有官方位置。[P3、P10]
5. 状态与历史适配按 pi 自身版本契约维护，不与 omp 共用“未知事件就试着订阅”的长期判断。

### 来源

- [P1：正式 npm metadata](https://registry.npmjs.org/@earendil-works/pi-coding-agent/latest)。
- [P2：正式 v1.3.0 release](https://github.com/earendil-works/pi/releases/tag/v1.3.0)，[旧包 deprecated metadata](https://registry.npmjs.org/@mariozechner/pi-coding-agent/latest)。
- [P3：v1.3.0 CLI 参数与交互模式](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/docs/cli.md)。
- [P4：v1.3.0 会话查找／分叉 flags](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/main.ts)。
- [P5：v1.3.0 SessionManager／forkFrom](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/core/session-manager.ts#L1815-L1866)。
- [P6：v1.3.0 进程内分支运行时](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/core/agent-session-runtime.ts)。
- [P7：v1.3.0 正式扩展事件类型](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/core/extensions/types.ts)。
- [P8：v1.3.0 UI prompt 包装](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/core/extensions/runner.ts)，[事件注册行为](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/src/core/extensions/loader.ts)。
- [P9：v1.3.0 JSONL v3](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/docs/session-format.md)。
- [P10：v1.3.0 环境变量](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/docs/environment-variables.md)。
- [P11：v1.3.0 安装／平台](https://github.com/earendil-works/pi/blob/v1.3.0/packages/coding-agent/README.md)，[原生平台构建／冒烟 CI](https://github.com/earendil-works/pi/blob/v1.3.0/.github/workflows/build-binaries.yml)。

## OpenCode 2.x：正式发布存在，但现有 lyrel 适配实质不兼容

### 产品身份与实际发行状态

已核实本研究对象是 **`anomalyco/opencode`、官网 `opencode.ai`**。npm 正式包 **`@opencode/cli@2.0.32`** 的 metadata 指向同一仓库，提供 `opencode` 与 `opencode2`；GitHub 有 `v2.0.32` 正式 release，commit 为 **`843a6c514e606b436a7fe713422ace0a93ad3d61`**，发布时间为 2026-10-16。[C1–C3]

GitHub releases 列表的前几项仍可能都是 1.x；不能由此判断 2.x 尚未发行。Context7 同时检出了旧 `packages/web` 文档和新 `services/www` 文档，旧页面的 `attach`／根命令 `--fork` 不能直接用于 2.x。以下关键接口均回查 **v2.0.32 tag**；另查看 dev 快照 `214fb7fdbab9ad13773629839f1178382400cf97`，不依赖它证明正式版能力。

**更正先前判断：OpenCode 2.x 不是“lyrel 已有较完整支持”。准确说法是：有旧的通用识别／命令与 2.x 插件骨架，但当前稳定 2.x 的事件、数据库、分叉命令和命令别名存在实质不兼容。**

### 独立／共享服务架构与环境边界

默认 CLI 发现或启动当前用户共享后台服务，服务拥有会话、配置、权限、集成与工具执行。`--standalone` 改为私有服务；`--server <url>` 连接明确服务，二者互斥。`opencode service set disabled true` 会改变用户全局默认行为并停止现有服务，不应由 lyrel 静默执行。[C4–C5]

共享服务进程的 `process.env`、`process.argv`、加载位置和当前目录都**不是每个 pane 的身份**。即使服务最初从某个 lyrel pane 启动，也不能把其后所有会话路由回该 pane。

2.0.32 另有真正的会话环境机制：CLI 本地连接时将 `Env.session()` 作为 TUI 启动环境传入；API `PUT /api/session/:sessionID/environment` 的语义是替换该会话本地 shell 的进程环境，core 以 `sessionID → variables` 的内存 Map 保存，`set` 覆盖原值。[C6–C8] 这不是服务全局环境，但同样**不是唯一 TUI/pane 归属协议**：没有客户端标识／连接生命周期，多个客户端设置同一会话会覆盖；明确 `--server` 路线也不沿用相同本地环境传递。不能用它冒充稳定 attach 事件。

### 会话 ID、恢复、分叉与历史

- ID 原生由 `SessionID.create()` 生成，前缀 `ses_`，后半部使用时间／计数及随机数据；API 创建会话可传 `id`。不需要 lyrel 从屏幕文本猜 ID。[C9–C10]
- 根命令 `--session/-s` 表示继续指定会话；不存在时，该 ID 留给第一次 prompt 创建新会话。恢复前若要避免误建空会话，应先通过官方 session 查询确认目标，而不是认定参数本身保证恢复成功。[C6、C11]
- **根命令没有 `--fork`**。完整 CLI 定义中 `session` 子命令只有 `list`、`delete`、`export`、`import`；`run` 有 `--fork`，但它是非交互运行，不应拿来替代用户的原生 TUI。[C11]
- 官方分叉 API 是 `POST /api/session/:sessionID/fork`／`client.session.fork({ sessionID, before? })`：省略 `before` 复制全部投影历史，返回新会话。官方 TUI 的 `DialogFork` 正是调用它并跳转到返回的 `session.id`。[C10、C12]
- 当前存储表是 `session_v2`、`session_message` 等；lyrel 查询旧 `session`、`message`、`part` 不会覆盖 2.x 新会话。优先官方 `session.list/get` 或 `opencode session list --format json`，而非继续绑定内部 SQLite schema。[C10–C11、C13]

### 事件、权限、问题、失败与中断

2.0.32 的正式事件 envelope 使用 `event.data`，会话执行事件均带 `data.sessionID`：[C14–C17]

| 语义 | 正式接口／事件 | 适配注意 |
| --- | --- | --- |
| 执行开始 | `session.execution.started` | 不再靠旧 `session.status.busy` 判断 |
| 正常完成 | `session.execution.succeeded` | 与失败／中断区分；结束的是执行，不是客户端 |
| 失败 | `session.execution.failed`，带 `error` | 不应全部显示成成功 |
| 中断 | `session.execution.interrupted`，`reason=user/shutdown/superseded/inactivity` | 用户取消、服务停机、被新工作取代等不是同一种结果 |
| 权限请求／回复 | `permission.asked`、`permission.replied` | 回复有 `requestID` 和 `once/always/reject`；拒绝不等于恢复运行 |
| 用户问题／表单 | `form.created`、`form.replied`、`form.cancelled` | 创建的会话 ID 位于 `data.form.sessionID`，并非统一的 `data.sessionID` |

官方 `question` 工具通过 `Form.Service.ask` 创建 `metadata.kind="question"` 的表单，不应仅监听旧 `question.*`。部分 MCP elicitation 使用 `sessionID="global"`，不能将它随意分配给当前 pane。[C16–C17]

权限和表单事件是 ephemeral；重连只恢复持久执行事件不足以恢复等待状态，应通过公开 session permission/form 列表同步当前未决请求。`/api/experimental/session/:sessionID/log` 可按 aggregate sequence 读取并 follow 持久日志，但它明确属于 experimental，而且不包含全部 ephemeral 状态；不是无条件可靠重放接口。[C10、C14–C18]

官方 JavaScript client 的 `event.subscribe()` 是 live-only，明确**没有 replay 或自动重连**；订阅源失败会结束当前订阅。因此外部 SDK 桥不能只写一个 `for await` 就声称可恢复，需在重新连接后同步权威状态。TUI 插件可复用 TUI 自己的数据层，但也必须验证重连行为；这里不把 SDK 的限制直接等同于 TUI 的实现。[C23]

更小的恢复路径是先查 `session.active`，再查 `session.get` 的 `outcome`／`time.idle`，结合未决 permission/form 列表重建当前状态；正式 `Session.Info.outcome` 已明确区分 succeeded／failed／interrupted，不必为了基本状态恢复强依赖 experimental 日志。注意 outcome 是“上次完成执行”的结果，当前仍 active 时不能拿旧 outcome 覆盖 working。[C10、C24]

当前 lyrel 只映射旧 `session.status`／`session.idle` 与部分权限事件，缺少当前执行结束、表单、失败与中断映射。即使删除 `--service` 的保护性 return，也不会自动补齐这些差距，反而会引入跨 pane 串线。

### 保留共享服务的官方 pane 关联入口

**已核实可用接口，不等于已验证实现：本地 TUI 插件。**

官方 CLI-only 插件在本地终端运行，使用 `cli.json` 的 `plugins` 或本地发现目录；即使 TUI 连接远端服务器仍在客户端本地执行。正式 `@opencode/plugin/tui` 的 `setup(context)` 暴露：[C18–C20]

- `context.ui.router.current()`：本客户端当前 route，session route 携带 `sessionID`。
- `context.ui.tabs.list()`：本客户端打开的根会话、active/busy/attention 状态；一个 TUI 可以打开多个会话。
- `context.data.on/listen`：订阅服务事件；**事件流本身不是已按该客户端归属过滤的证据**，仍需按已绑定 session 过滤。
- `context.data.session.get/root/family/status` 和 permission/form 查询／同步。
- `context.client`：已连接、已带服务上下文的官方 client。
- 返回 cleanup，可注销订阅；slot 输入／路由状态可随客户端导航响应。

**[INFERENCE] 推荐关联路线：**

1. 在 lyrel 启动的 TUI 本地插件里读取该客户端继承的 pane 标识，建立 `pane ↔ 当前会话/打开会话集合` 的绑定，回报真实 session ID；不读取共享服务环境推断 pane。
2. 以本客户端 route 为前台恢复目标，按 session ID 严格过滤事件；若产品需要后台任务汇总，另按本客户端 tabs 集合聚合，不能用“最近收到的根会话”覆盖绑定。
3. 监听本客户端新建、切换、分叉、关闭 tab；通过公开响应式路由／slot 重新获取绑定。官方 `tui.session.select` 是导航命令事件，不是已发现的 pane 连接／attach 生命周期事件，不能只凭其名字使用。[C19–C21]
4. 明确子会话策略：child 的 ID 不得覆盖前台恢复目标；允许汇总等待状态也必须沿 `root/family` 处理。fork 得到的新会话应按实际 route 绑定，不把所有带历史父来源的会话一概当后台 subagent。
5. 同一 session 被两个 TUI／pane 打开时，事实是多个视图，不存在可从会话 ID 推导出的唯一“所有者”。两边显示共享会话状态或选择单视图策略，必须显式定义，不能靠服务端 env 竞争。
6. cleanup／断线表示客户端桥接结束，不应当作 `session.execution.succeeded`：共享服务中的工作可能继续。

这个方案**不要求更换原生 TUI，也不必默认 `--standalone`**。本地插件加载、无可见 UI 的响应式导航监听、热重载、跨平台子进程回报、双 pane 并发和重连后的状态恢复仍需真实验证。

### 最小分叉连接路线：无需强加外部 Node／Bun helper

**[INFERENCE] 路线 A：沿用本地 TUI 插件的已连接 client。** 插件调用 `context.client.session.fork({ sessionID })`，取得新 ID 后通知 lyrel；lyrel 在新 pane 启动原生 `opencode --session <newID>`。这复用现有 TUI／插件运行时，不新增外部 Node／Bun 执行前提，也不替换用户 TUI。[C12、C19]

**[INFERENCE] 路线 B：复用 OpenCode 自带 `api` CLI。** 若动作来自 lyrel GUI 而没有现成客户端请求通道，Rust 可通过 argv 调用：

```text
opencode api session.fork --param sessionID=<sourceID> --data '{}'
```

官方 `api` handler 自动发现服务、使用官方认证 header，从 OpenAPI operation ID 构造路径并输出原始 JSON；lyrel 解析成功响应的 `data.id` 后，再创建新 pane 执行 `opencode --session <newID>`。无需引入独立 JavaScript 脚本或 SDK 运行时。此命令是依据正式参数／handler／协议推导，**本轮未实际执行**；必须检查退出码，保留 `--server` 等有效连接参数，不把日志当 JSON，也不能误连另一数据源。[C10–C11、C22]

独立服务模式下要保留可访问的原服务 endpoint；为 fork 操作随意新起另一个 `--standalone` 服务可能不是同一个内存会话／数据源。SDK 预建 session 或 `--session <新ID>` 的 wrapper 可以预先绑定首个会话，但**wrapper 本身不足以覆盖 TUI 内后续切换／新建／分叉**。

`--standalone` 可作为用户明确选择的私有模式／降级方案，不应当成可靠关联的必要条件，也不能承诺它自动实现“一个服务只有一个根会话”。不要静默关闭用户共享服务。

### 来源

- [C1：仓库身份](https://github.com/anomalyco/opencode)。
- [C2：npm latest metadata](https://registry.npmjs.org/@opencode/cli/latest)。
- [C3：正式 v2.0.32 release／commit](https://github.com/anomalyco/opencode/releases/tag/v2.0.32)。
- [C4：v2.0.32 CLI／后台服务说明](https://github.com/anomalyco/opencode/blob/v2.0.32/services/www/src/docs/content/cli/index.mdx)。
- [C5：v2.0.32 服务连接选择](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/services/server-connection.ts)。
- [C6：v2.0.32 TUI 启动／恢复 handler](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/commands/handlers/default.ts)。
- [C7：v2.0.32 CLI 会话环境采集](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/env.ts)。
- [C8：v2.0.32 会话环境 Map](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/core/src/session/environment.ts)。
- [C9：v2.0.32 SessionID](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-id.ts) 与 [identifier 算法](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/identifier.ts)。
- [C10：v2.0.32 session API，fork／list／log／environment](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/protocol/src/groups/session.ts#L312-L327)。
- [C11：v2.0.32 完整 CLI 参数定义](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/commands/commands.ts)。
- [C12：v2.0.32 官方 TUI fork 实现](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/routes/session/dialog-fork.tsx)。
- [C13：v2.0.32 数据表](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/core/src/session/sql.ts)。
- [C14：v2.0.32 执行事件](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-event.ts#L240-L267)。
- [C15：v2.0.32 权限事件](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/permission.ts)。
- [C16：v2.0.32 表单事件](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/form.ts)。
- [C17：v2.0.32 question 工具](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/core/src/tool/plugin/question.ts)。
- [C18：v2.0.32 CLI 插件发现与本地执行](https://github.com/anomalyco/opencode/blob/v2.0.32/services/www/src/docs/content/cli/plugins.mdx)。
- [C19：v2.0.32 TUI 插件公开接口](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/plugin/src/tui/context.ts)，[构建说明](https://github.com/anomalyco/opencode/blob/v2.0.32/services/www/src/docs/content/build/plugins/cli.mdx)。
- [C20：v2.0.32 TUI 插件 route／tabs 实现](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/plugin/api.tsx)。
- [C21：v2.0.32 TUI 事件表](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/tui-event.ts)。
- [C22：v2.0.32 原生 api 命令 handler](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/commands/handlers/api.ts)。
- [C23：v2.0.32 官方 client／流恢复语义](https://github.com/anomalyco/opencode/blob/v2.0.32/services/www/src/docs/content/build/client/index.mdx)。
- [C24：v2.0.32 Session.Info 的 outcome／时间语义](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session.ts)。

## Hermes：原生能力充足，lyrel 缺的是整条状态／历史接线

### 产品、发布版与原生接口

对象是 **Nous Research 的 `NousResearch/hermes-agent`**，不是名称相近的第三方镜像。已核实 stable **v0.22.2**（2026-10-15，release target commit `d923a5eb487a17c0a68801bba9ef0f56c81040a8`）。本节将其与 main 的 gateway 迁移分开；未运行 Hermes。[H1–H2]

- 原生 `hermes chat --resume <id|title>`／根命令 `--resume`、`--continue`，支持唯一 ID 前缀。当前 lyrel 的恢复命令语法正确，但没有 ID 发现／状态桥闭环。[H3]
- 原生分叉是会话内 `/branch [--here] [name]`（别名 `/fork`）、TUI gateway JSON-RPC `session.branch`，以及 ACP 的 `fork_session()`；**没有对应 CLI 启动 fork 参数**。`--worktree` 不是会话分叉。[H4–H5]
- **交互初始任务已原生支持**：真实 TTY 上 `hermes chat -q/--query "<task>"` 将任务按字面作为第一轮提交，并保持会话打开，不解释成 slash 命令或 `!` shell escape。只有 `--oneshot`／`-Q`／非 TTY 才转为答完退出；`-z` 则是纯脚本一次性入口，不能混淆。程序化 `--format stream-json` 提供结构化事件与结果；本轮没有确认 `--query-file` 的交互保活行为。[H3、H5]
- 历史位于有效 `$HERMES_HOME/state.db`，named profile 有独立数据目录；默认 Windows 与 Unix 路径不同。文档化枚举入口为 `hermes sessions list/browse/export`。SQLite 是 WAL，包含运行时安全账本；只读连接可用，但不宜绑定内部布局或复制数据库／WAL 冒充快照。[H6–H7]

### 状态桥应选对 hook 体系

Hermes 有几种不同 hook，不可混用：[H8–H11]

| 体系 | 执行范围／用途 | 对 lyrel 的判断 |
| --- | --- | --- |
| gateway event hooks，`~/.hermes/hooks/...` | messaging gateway；CLI/TUI 不加载这套目录 | 不能冒充 CLI 状态集成 |
| Python plugin hooks | `ctx.register_hook`，CLI/Gateway；需显式启用插件 | 可选桥接，需维护 Python 插件加载与启用 |
| shell hooks，`config.yaml` 的 `hooks:` | stdin JSON，CLI/Gateway/Desktop/TUI 等 | 发布版最直接的适配入口 |
| outbound webhook | HTTP POST，可用 HMAC | 有队列丢事件与隐私边界，不作为默认必要组件 |
| observer v1 | 只读观察契约 `hermes.observer.v1` | 优先依赖相关 ID 和稳定载荷，不插手审批决策 |

shell hook stdin 有 `hook_event_name`、`tool_name`、`tool_input`、`session_id`、`cwd`、`profile`、`extra`；其他事件专有字段在 `extra`。审批钩子可能只有 `session_key` 等上下文，不能假定每次都有 session ID。[H9–H11]

建议事件映射（**[INFERENCE]**，需实测时序）：

- `pre_llm_call`／`pre_api_request` → working；`on_session_start` 取得 session ID。
- `on_human_input_request/resolved`，kind 为 sudo／clarify／approval；或 `pre_approval_request/post_approval_response` → waiting／解除等待。保留 request ID 和 choice，避免权限／问题尚未全部解决就回到 working。
- `post_llm_call` 仅表示成功 turn；`on_session_end` 带 `completed/interrupted` 处理完成／失败／中断。`api_request_error` 可能处于重试过程，不应一概提前结束整个 run。
- `on_session_end` 是 turn/run 结束，不是会话身份终结；后者看 `on_session_finalize/reset`。`agent_loop_stopped` 不覆盖 plain CLI，不能只依赖它处理取消。
- 子代理事件含父子 session/turn ID，不得把 child 写成 pane 的恢复目标。

shell hooks 默认有同意机制，safe mode 会禁用插件／shell hooks。安装器应明确呈现将执行的 lyrel 桥脚本与事件，让用户授权；**不自动设置 `HERMES_ACCEPT_HOOKS=1`、`--accept-hooks` 或写 allowlist 绕过同意**。卸载仅移除 lyrel 拥有的配置和授权项。[H8–H10]

### pane 归属与版本风险

v0.22.2 plain CLI 在客户端进程内执行智能体。**[INFERENCE]** shell hook 子进程可继承 lyrel pane 环境，与现有本地 hook 模式相容，但尚未实测；远端桥接也必须在对应远端 Host 安装并完成同意。

另有正式的终端面包屑 `$HERMES_HOME/terminal-sessions/<terminal_id>`，记录 session ID／cwd／时间；先使用 tty 标识，再按已知终端环境变量回退，并支持关闭、陈旧清理。这可辅助发现／恢复，但不是权威的实时运行状态协议，不能替代 hook 事件。[H12]

**main 尚未发布的变化必须单列**：classic CLI 正迁移为共享 gateway 的客户端；hook 可能改在 gateway owner 进程执行，失去 pane env。main 的 `client_env` 允许表透传对 shell hook 的可见性未验证；`--source` 也受 surface 白名单约束。不能为解决关联而强制每 pane 独立 profile，因为这会隔离历史、凭据和记忆，改变正常使用模式。[H13]

### lyrel 的最小补齐范围

1. 增加 Hermes 状态桥、安装／诊断／卸载入口，按已发布版本区分 hook 执行位置。
2. 增加有效 HERMES_HOME／profile 的历史列表与 session ID 绑定，沿用正确的原生 resume。
3. 保留原生 CLI/TUI；fork 可通过官方会话协议进一步接通，但“在 lyrel 新 pane fork”还需要真实协议连接／返回新 ID／启动链路，不能拼一个不存在的 `--fork`。
4. 不为统一功能矩阵而改为 one-shot、抓取屏幕文本、使用 messaging-only gateway hooks 或写内部数据库。

### 来源

- [H1：官方仓库](https://github.com/NousResearch/hermes-agent)。
- [H2：正式 v0.22.2 release](https://github.com/NousResearch/hermes-agent/releases/tag/v0.22.2)。
- [H3：v0.22.2 CLI 参考](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/reference/cli-commands.md)。
- [H4：v0.22.2 slash 命令](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/reference/slash-commands.md)。
- [H5：程序化集成](https://hermes-agent.nousresearch.com/docs/developer-guide/programmatic-integration)，[ACP 内部说明](https://hermes-agent.nousresearch.com/docs/developer-guide/acp-internals)。
- [H6：会话存储](https://hermes-agent.nousresearch.com/docs/developer-guide/session-storage)。
- [H7：v0.22.2 会话管理](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/user-guide/sessions.md)。
- [H8：v0.22.2 hooks 文档](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/user-guide/features/hooks.md)。
- [H9：v0.22.2 正式 hook 事件表](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/hermes_cli/plugins.py)。
- [H10：v0.22.2 shell hook 载荷／同意机制](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/agent/shell_hooks.py)。
- [H11：observer hooks](https://hermes-agent.nousresearch.com/docs/developer-guide/observer-hooks)，[v0.22.2 人工输入事件](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/tools/human_input_hooks.py)。
- [H12：v0.22.2 终端会话面包屑](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/hermes_cli/terminal_breadcrumbs.py)。
- [H13：main 的 gateway classic CLI（非 v0.22.2）](https://hermes-agent.nousresearch.com/docs/developer-guide/gateway-classic-cli)，[main session_client_env](https://github.com/NousResearch/hermes-agent/blob/main/agent/session_client_env.py)，[main session_policy](https://github.com/NousResearch/hermes-agent/blob/main/gateway/session_policy.py)。
- [H14：v0.22.2 平台支持等级](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/getting-started/platform-support.md)，[原生 Windows 说明](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/user-guide/windows-native.md)，[PowerShell 安装器](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/scripts/install.ps1)。
- [H15：v0.22.2 WSL 说明（与原生 Windows `/chat` 说明有冲突）](https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/user-guide/windows-wsl-quickstart.md)。

## 共同兼容边界与推荐范围

### 不应把“统一支持”变成伪兼容

- 保留各智能体的原生交互界面与服务模式；不要为了采集状态统一换成 one-shot、`run`、ACP 或自制聊天 UI。
- 原生支持分叉，不等于 lyrel 的“新 pane 分叉”已经接通；必须拿到新 session ID、在正确数据源启动、保留有效启动上下文，并正确处理 fork 失败。
- 恢复 ID、历史枚举、状态事件、任务启动是独立能力；识别到进程名不代表其他能力成立。
- 运行／等待状态与结果必须分开。当前 lyrel 的 `AgentStatus` 只有 Idle／Working／Waiting／Done，不能将 Done 的绿色展示误说成成功验证；至少保留上游失败／取消原因，不能吞掉差异后宣称全支持。
- 子代理、重试、后台工作、多个未决审批、多个会话 tab、多 pane 查看同一会话都应纳入定义；不要把“最后一个事件”当当前 pane 的事实。
- 当前五种的 `prompt_args` 均为空；工作树创建调用方因此隐藏 Task 输入并不追加任务。即使它们原生可接初始 prompt，lyrel 现有“带任务启动”也未接通，不能把裸启动当委派完成。
- CLI、hook、历史目录必须一起解析有效 profile／home／session-dir；同机不同 profile 或本地／远端来源，不能只用 agent slug + session ID 隐式合并。
- 避免在 GUI 线程同步运行 CLI 或扫描数据库；继续沿 `Host`／远端工作区边界。共享协议若需要增加信息，应使用现有版本协商，不另建仅本地有效的路径惯例。

### 已核实的平台分发，不等于 lyrel 桥接绿灯

- **omp v18.12.1**：官方 README 明列 macOS、Linux、原生 Windows，并明确不是 WSL bridge；有 PowerShell 安装路线。npm／Bun 路线要求 `bun >= 1.3.14`，另有预编译二进制，因此不能写成所有用户都必须额外装 Bun。[O1、O7]
- **OpenCode 2.0.32**：正式 npm metadata 明列 darwin／linux／win32，包含 Windows x64／arm64、macOS x64／arm64、Linux glibc／musl 平台包；这是原生 Windows 分发证据，不是 WSL 推断。[C2]
- **pi v1.3.0**：正式 release 有 macOS x64／ARM64、Linux x64／ARM64、Windows x64／ARM64 的 `pi.exe`；官方安装／终端文档及 Windows CI 都覆盖原生 Windows，不是仅 WSL。npm 路线要求 Node.js >=22.19.0，独立二进制另论；WSL 有专门终端行为说明，但本轮未取得单独的平台承诺页，不能代替 lyrel↔WSL 实测。[P1–P2、P11]
- **Kimi Code 2.2.0**：官方安装路线覆盖 macOS x64／ARM64、Linux glibc x64／ARM64 和原生 Windows；Windows 要 Git for Windows，可用 `KIMI_SHELL_PATH` 覆盖工具 shell。预编译 Linux 安装器不支持 musl／Alpine，官方建议评估 npm 路线，不代表本轮已核实其所有 native 依赖可用；npm 要 Node.js >=22.19.0。WSL 有明确识别／剪贴板桥实现，仍走 Linux 安装，不等同原生 Windows。[K1、K3–K4]
- **Hermes v0.22.2**：官方 Tier 1 包含原生 Windows 10/11 x64／ARM64，有 `install.ps1`，不是只能用 WSL；MSIX／Store 包另要求 Windows 11 22H2+。macOS Tier 1 是 Apple Silicon，Intel 桌面包依赖特定发行选项；Linux／WSL2 覆盖 x64／ARM64，WSL1 不可靠。[H14–H15]
- **Hermes hook 跨平台前提**：POSIX 直接 exec、依赖 shebang／可执行位；Windows `.sh/.bash` 映射 Git for Windows 的 Bash，不能用 MinGit 替代；`.py` 裸脚本可映射到 Hermes 自己的 `sys.executable`，有望避免额外 Bash 桥脚本前提。脚本显式以 `python ...` 开头又变成 PATH 解析，不能混为一谈；Windows allowlist 缺 `fcntl.flock`，仅 best-effort 线程锁。以上是源码证据，未实机验证。[H10、H14]
- Hermes 同 tag 原生 Windows 文档称 dashboard `/chat` 使用 ConPTY／pywinpty，而 WSL quickstart 又称该面板 WSL2-only；**该面板原生 Windows 能力未确认**。不要因 CLI 原生支持而替所有前端背书。[H14–H15]

任何平台还需要验证 lyrel 的实际 executable 路径、参数传递、插件解析、无 `sh` 环境、Unicode 路径、远端 Host 安装和退出清理；本轮没有这些平台实测。

- [O7：v18.12.1 安装与原生 Windows 说明](https://github.com/can1357/oh-my-pi/blob/v18.12.1/README.md#install)。

## 未验证项与后续实施验收条件

以下不是本轮已通过的测试，而是获得“完整支持”结论前应实际运行的场景：

1. **真实版本／身份识别**：在隔离配置里识别实际安装产品、版本、可执行别名；不把同名旧产品或开发通道当成另一个已支持版本。
2. **初始任务**：交互界面保留，任务内容只提交一次；多行、Unicode、以 `-` 开头、看似子命令的文本不会变成启动选项；任务不会因 `prompt_args=None` 被丢弃。
3. **状态闭环**：开始、普通工具、多个权限请求、问题／表单、拒绝、取消、失败、自动重试、压缩后继续、后台任务未结束等，都不误报完成。
4. **会话身份闭环**：首次创建即获得 ID，切换／分支及时重绑；子代理不覆盖主会话 ID；停止客户端与停止服务端执行分开。
5. **历史与恢复**：默认／覆盖目录、profile、session-dir、本地／远端来源分别验证；选中历史可恢复，目标缺失报错而不误建新会话；敏感正文不为列表展示被完整读取或外传。
6. **原生分叉**：保留原会话，新会话 ID 不同、历史／artifacts 符合上游语义；启动到新 pane 后可继续；失败不能产生虚假的“已分叉”UI。
7. **OpenCode 共享服务**：至少两个同目录 pane 同时运行不同会话、一个 TUI 多 tab、同会话双视图、用户切换／新建／fork、服务先于 lyrel 启动、另一终端也连同服务；所有状态都按已绑定视图路由。客户端关闭后服务中的任务可继续；重连同步未决权限／表单及最终 outcome。
8. **版本迁移**：已发布版本与 main／开发通道分别验收；未覆盖契约应明确显示不支持／降级，不能静默把“无事件”当 idle 或 done。
9. **平台与安全模式**：macOS、原生 Windows、Linux／WSL 分开验证；不依赖系统必有 `sh`。用户禁用扩展、shell hooks 未同意、safe mode、无 TTY、远端配置未安装时，应能诊断而不是假装状态可靠。

本轮证据仅证明接口存在、当前差距与路线可行性，**不证明上述任何真实端到端验收已经通过**。
