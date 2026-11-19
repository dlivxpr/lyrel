# 仓库工作指南

## 产品与架构

lyrel 是基于 l0ng-ai 的 Apache-2.0 终端源码独立开发的 Rust/GPUI 终端工作台，面向桌面本地开发和桌面远程开发；Markdown/editor 是辅助能力，不扩展成独立写作产品或手机网关。保留原始版权、许可证和真实第三方资源来源。

产品/根包为 `lyrel`，GUI 为 `lyrel-app`，CLI crate/命令为 `lyrel-cli` / `lyrel`，共享核心为 `lyrel-core`，server 为 `lyrel-server`，updater 为 `lyrel-updater`。独立 stable 版本、`LYREL_*` 环境变量、数据与凭据命名空间不自动读取、迁移或接管其他产品实例；更名不重置 IPC 协议版本。仓库：`https://github.com/dlivxpr/lyrel`。

GUI/CLI 经 `lyrel-protocol` 连接独立 Session Daemon；`lyrel-core` 重导出协议并持有 daemon、PTY、shell/coding agent 和 SSH/stdio 远端接线。会话不依赖窗口存活；GUI 的 `--daemon` 与 headless `lyrel-server` 使用同一实现。正式架构见 `docs/reference/architecture.mdx`。

## 修改入口

| 路径 | 职责与约束 |
| --- | --- |
| `src/main.rs`、`src/lib.rs::run` | 保留平台 subsystem 的薄 binary；GUI/daemon/agent-hook 分发与应用注册 |
| `src/ui/`、`src/terminal/` | 工作台、编辑器、设置、终端输入与渲染；`app.rs` / `view.rs` 等大文件按行为定位范围阅读 |
| `crates/lyrel-ui-base/` | 桌面共享配置、快捷键、主题、通知、工作区视图状态与 Host 适配；不依赖终端、文档引擎或窗口 shell |
| `crates/lyrel-protocol/` | 无 GPUI 的 wire、IPC 客户端、配置、会话值类型与 Host 契约；CLI 使用不带 SQLite 引擎的 LocalHost |
| `crates/lyrel-core/` | Session Daemon、持久化、PTY、SSH、RemoteHost 与注入 LocalHost 的只读 SQLite 引擎 |
| `src/daemon.rs`、`src/core/` | core 重导出与 GUI/系统适配，不复制共享实现 |
| `crates/lyrel-cli/src/cli.rs`、`crates/lyrel-cli/src/backend.rs` | CLI 参数契约；依赖替换沿用 `Backend` trait 分层 |
| `crates/lyrel-document/`、`crates/lyrel-markdown-editor/` | GUI-free 文本 codec 与 `formats` 引擎；Markdown editor 只启用 codec，不反向依赖工作台 |
| `crates/lyrel-insights/` | 经 Host 执行 skills 和统计，拥有 GitHub 与用量数据；GPUI 设置呈现和系统凭据留在根包 |
| `crates/lyrel-updater/` | Windows／macOS 更新程序、共享布局与安装计划；行为与打包契约见 `docs/reference/updates.mdx` |
| `crates/lyrel-protocol/src/core/config.rs` | 配置解析与进程覆盖；GPUI 包装和测试写盘守卫由 `lyrel-ui-base` 持有 |
| `crates/lyrel-test-support/` | 跨包 PTY、协议与 SSH 测试夹具及 `lyrel-graphics-fixture` 进程；测试以 dev-dependency 复用 |
| `Cargo.toml`、`Cargo.lock`、`.cargo/config.toml` | workspace、统一版本、features、锁定依赖与开发别名 |
| `scripts/`、`.github/`、`docs/docs.json` | 开发与平台脚本、CI/发布工作流、文档导航 |

## 工程底线

- 工作区路径可能属于远端。文件、Git、监视等经 `Host` / `SharedHost` 注入，GUI 通过 `lyrel-ui-base` 的 `HostOps` 在线程池执行；不能用本地文件系统解释远端路径或在 UI 线程执行阻塞 Host 调用。
- `lyrel-protocol` 和 `lyrel-core` 不依赖 GPUI；CLI 生产路径只依赖前者，不链接 SSH、Tokio、SQLite 或 PTY 引擎。`lyrel-server` 唯一生产依赖为 core。系统凭据留在 GUI；保留 Windows/macOS 到 Linux 的 SSH/SFTP、Host/PTY 和 WSL 接线。
- daemon 持有并持久化机器/工作区/标签页/pane 树；`src/ui/machine_mirror.rs` 的拉取期间本地写重放必须保留，避免旧快照覆盖新操作。
- GPUI 沿用 `Entity`、`Global`、`cx.spawn`、`background_executor`；GUI 只经 `gpui-kit` 官方 Fast 类型。daemon 的 std 线程/channel 与 SSH 引擎的 Tokio runtime 不混作同一个异步环境。
- Host/文件错误用 `io::Result`，二进制边界用 `anyhow` 上下文；GUI 经 `HostOps::run_in` 和 `HostOps::notify_err` / error notification 呈现失败，保留 CLI 错误链与子进程退出码。
- 修改终端输出热路径保留 `src/terminal/view.rs` 的隐藏 pane 通知门控，不因不可见输出触发整窗重绘。
- UI 颜色取 `lyrel-ui-base` 的语义角色；文案经 `L10nKey` 同步 `en`、`zh-CN`。`system` 是配置模式，不是第三种翻译语言；OS 界面语言解析不能用 PTY locale 替代。
- IPC 修改前读 `crates/lyrel-protocol/src/daemon/control.rs` 和 `crates/lyrel-protocol/src/daemon/protocol.rs`；增删 control 变体提升 `CONTROL_VERSION`，可忽略字段按 feature 协商，pane 协议版本另行处理。
- 第三方 Rust 依赖默认只用正式 registry，不引入 Git、自定义第三方 path、`[patch]` 或修改下载源码。例外仅限 #236 的 Zorite editor/bidi、#241 的 ratex GUI，以及 #279 在正式架构文档「上游源码复用与适配」清单批准的固定 Glim MIT / Zorite MIT `gpui-pdf`；清单内优先复用真实实现，逐文件记录来源、原始摘要、许可和本地适配，不扩展到 GPL 应用代码或清单外资源。
- Windows ConPTY 与 Windows server 仅支持 x86_64，远端 ARM64 Windows 明确拒绝；固定 ConPTY 文件对、许可、架构、版本和摘要校验 fail-closed，不静默回退系统实现，构建脚本不隐式下载。
- 默认配置为 Unix `~/.config/lyrel`、Windows `%APPDATA%\lyrel`；覆盖顺序为 `--config-dir`、`LYREL_CONFIG_DIR`、便携模式、默认目录。测试/调试必须隔离，不能影响安装实例。产品 `TERM_PROGRAM=lyrel`，标准环境变量键保持原样；#248 兼容身份只限已知 coding agent 的命令作用域，显式值优先，结束恢复 pane 基线。

## 开发与验收

构建、测试或运行验收先读 `.agent/skills/lyrel-development/SKILL.md`。Windows 常用入口为 `pwsh scripts/cargo-windows.ps1 build --locked`，macOS 为 `cargo build --locked`；`cargo dev` 使用 `.lyrel-dev`，Windows 启动前完成同配置 wrapper build。

- Rust 使用 edition 2024、stable 和现有 lockfile，不把本机版本设为 MSRV；沿用 rustfmt，保留 Clippy 对 `dbg!` / `todo!` 的禁止。
- Cargo 检查、构建和测试以当前检出的 zero-warning 为交付标准，遗留 warning 逐条说明理由。按改动选择最小检查，行为变更还需真实运行；纯规则/文档改动不触发无关产品构建。
- 普通桌面单元测试归 library，不以 binary 的零用例代替；配置、socket 和子进程隔离，native 字形测试保留 OS 主线程约束。缺运行时、平台或外部工具不能跳过后宣称完整通过。
- 报告实际命令、退出码和关键日志，区分已运行、跳过和未验证。Windows 通过不替代 Unix；CI/headless/截图不替代原生 UI、SSH、中文 IME、安装和在线更新验收；桌面交互优先后台自动化。

## 按任务读取

| 触发条件 | 入口 |
| --- | --- |
| 探索产品代码或领域概念 | `docs/agents/domain.md` 及任务涉及的正式文档 |
| UI、布局、交互或文案 | `gpui-kit`、`gpui-kit-design-guides`、`docs/design-system.md`；语言解析见 `docs/customization/settings.mdx` |
| 编辑 Markdown editor | `crates/lyrel-markdown-editor/HOST_CAPABILITIES.md` |
| 构建、测试、ConPTY、平台与运行验收 | `.agent/skills/lyrel-development/SKILL.md` |
| issue/PR、分诊、依赖、分支同步或交付清理 | `.agent/skills/lyrel-github/SKILL.md` |
| 获准实施多工单规格 | `.agent/skills/lyrel-implement-spec/SKILL.md` |
| 版本提升、打包、发布或资产核验 | `.agent/skills/lyrel-release/SKILL.md` |
| 用 CLI 操作 Lyrel 工作区/pane 或启动可见外部智能体 | `skills/lyrel/SKILL.md`；外部委派再读其 `references/delegation.md` |

项目专用 Skill 只存放在本仓库的通用目录 `.agent/skills/`，不放入 omp 专用目录或全局目录；`skills/lyrel/` 是对外分发的产品技能，不搬迁或复制。支持发现该目录的工具可用对应 Skill 入口，其他环境按文件路径读取；omp 可用 `skill://lyrel-*`。内置委派遵守当前工具契约，用实际可用的 agent 与隔离机制，不把模型 role 当作 agent 名；显式传递相关工程约束。

## 文案与交付权限

- `AGENTS.md`、Skill 和 `docs/` 使用简体中文；GitHub issue/PR 的标题、正文、评论及 Git 提交信息使用英文。GitHub Release 标题只用版本 tag，中文变更说明写入正文；路径、命令、代码和 API 保留原文。
- 工单与规格使用 GitHub Issues，只有明确要求才新建规格、工单或文档目录。提供编号、调研、实现或发布规划均不自动授权额外 GitHub 写操作。
- 交付方式以任务授权为准：本地任务完成并验证即可；提交、推送、合并、公开发布、历史改写和资源删除不能从完成条件反推授权。分支策略与清理边界见 GitHub Skill；保留用户改动和必需构建资产。
