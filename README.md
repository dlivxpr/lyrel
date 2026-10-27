<div align="center">

<img src="assets/app-icon.svg" alt="lyrel" width="88" height="88" />

<a id="lyrel"></a>
### lyrel

**终端工作台：持久会话、远程工作、智能体。**

<sub>纯 Rust · GPUI Kit（GPUI）GPU 渲染 · 正式 Rio VT 内核</sub>

<br />

[![初始版本](https://img.shields.io/badge/version-0.1.0-3FDD8C)](https://github.com/dlivxpr/lyrel)
[![桌面平台](https://img.shields.io/badge/desktop-macOS%20%C2%B7%20Windows-blue)](https://github.com/dlivxpr/lyrel)
[![许可证](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

</div>

<a id="why"></a>
## 为什么选择 lyrel

Shell 和窗格由后台服务器管理，而不是窗口。以下所有特性都建立在这一点之上。

- **持久会话** — 退出应用后，Shell 继续运行；重启后，布局、屏幕内容和受支持的智能体对话都能恢复。无需 tmux
- **智能体感知** — 内置身份仅保留 omp、Kimi Code、pi、OpenCode 2.x、Hermes；其他程序仍可在普通 shell 中运行
- **可由智能体编排** — 一个智能体可以为另一个智能体打开窗格、交接任务、等待并读取结果，无论 GUI 是否运行
- **编辑器级输入** — 建议、补全、高亮、历史搜索，无需安装插件
- **远程开发** — 通过原生 SSH 协议栈工作，文件、仓库、窗格和 git 数据均保留在远程机器上

<a id="install"></a>
## 安装

lyrel 的独立初版版本为 **0.1.0**。从 [**Releases**](https://github.com/dlivxpr/lyrel/releases) 获取 stable 安装包及 `checksums.txt`，可用资产以对应版本页面为准。历史 macOS 包采用 ad-hoc 签名，未经 Developer ID 签名或 Apple 公证；今后的正式 macOS 桌面包要求 Developer ID Application 签名与 Apple 公证，未配置相应凭据时阻止发行，不表示历史资产已完成签名迁移。历史 ad-hoc 安装需手动迁移并重新授权，签名前提与限制见[更新文档](docs/reference/updates.mdx)。Windows 包未签名。桌面端仅支持 macOS、Windows；缺少 lyrel 安装包时，不应回退到上游下载安装包。Linux 保留 `lyrel-core`、`lyrel-server` 和 CLI，Windows／macOS 桌面连接 Linux 的远程开发与 WSL 不受此调整影响。

| | | |
|---|---|---|
| **macOS**（Apple 芯片） | `…-macos-arm64.dmg` | 拖入“应用程序” |
| **Windows** | `…-setup.exe` · 便携版 `….zip` | |

<a id="whats-inside"></a>
## 功能一览

| | |
|---|---|
| **智能体感知** | 按窗格识别五种内置产品 · 状态圆点 · 通知 · 分支与差异 · 需要输入时显示托盘图标 · 重启后恢复 · 从全局搜索重新打开历史会话 · 一键启动 · 标签页侧边栏按仓库分组，支持固定分组；各能力的正式运行验收分别记录 |
| **CLI + 技能** | 随附 `lyrel` CLI · [智能体技能](skills/lyrel/SKILL.md) · `run` 流式输出命令结果并以该命令的退出码退出 · `split` · `send` · `wait --until free` · `capture` |
| **编辑器级输入** | 根据历史记录显示幽灵建议 · 带说明的 Tab 补全 · 语法高亮 · 多行编辑 · 点击定位光标 · <kbd>⌃ R</kbd> 模糊搜索历史 |
| **窗口** | 标签页与分屏 · <kbd>⌘ P</kbd> 全局搜索 · <kbd>⌘ F</kbd> 回滚内容搜索 · 休眠标签页以释放内存 · 输入法支持 |
| **侧面板** | <kbd>⌘ J</kbd> · 进程树和监听端口 · 文件树 · 更改与差异 · 文件内容搜索 · 通过 `gh` 登录，以只读方式查看 GitHub 议题和拉取请求 |
| **Shell 集成** | 窗格启动时自动注入，无需安装 · 提示符标记 · 工作目录 · 退出码 · 命令完成通知 · zsh、bash、fish、PowerShell、WSL、远程窗格 |
| **远程工作区** | 远程文件、仓库、更改、差异、工作树、标签页和窗格 · 从任意客户端重新连接，接着上次的进度继续工作 |
| **SSH** | 原生 russh 协议栈：配置档案与钥匙串密钥存储 · SFTP 面板 · 端口转发 · 跳板机 · 一次性、无需特权的 `lyrel-server` 安装 |

<a id="supported-agents"></a>
## 支持的智能体

内置智能体保留各自的原生 TUI，不包装或代理程序。命令识别、状态报告、历史、恢复、分叉和首条任务是不同能力：正式版本、授权及真实运行证据须分别核对，不能由名称识别推导全部可用。

| 智能体 | 正式命令 | 实施基线 |
|---|---|---|
| omp（Oh My Pi） | `omp` | 18.12.1 |
| Kimi Code | `kimi` | 2.2.0 |
| pi | `pi` | 1.3.0 |
| OpenCode 2.x | `opencode`、`opencode2` | 2.0.32 |
| Hermes | `hermes` | 0.22.2 |

omp 与 pi 是独立产品。Kimi Code 不接管归档产品数据；OpenCode 只维护 2.x 接口，不新增每次启动前的强制版本拦截。
集成能力与验收边界见[智能体概览](docs/agents/overview.mdx)及[会话操作](docs/agents/sessions.mdx)。表格不是五种正式版本已经完成所有 GUI／daemon／PTY 验收的声明。

退役项的旧设置和快捷键不再生效，不映射为其他智能体；已有进程只失去专属身份，不因名单缩减而被终止。lyrel 不自动扫描或清理用户目录中的旧 hooks，不升级全局智能体、不删除账号或历史。[人工清理与升级边界](docs/agents/overview.mdx#retired-hooks)说明如何逐项核对 lyrel 归属。

这些智能体都不会被包装或代理 — 你启动的就是智能体本身，
它在普通 PTY 中运行，保留自身的界面。通过包装脚本启动的智能体，
可以使用 `config.json` 中的 `agent_commands` 按名称映射到对应智能体。
`PATH` 上的受支持智能体也会在全局搜索中显示为一项（**智能体：omp** 等），
选择后会在新标签页中打开；“新建标签页”菜单则列出你实际运行过的智能体。
`agent_launch` 用于设置启动时使用的命令行。

<a id="documentation"></a>
## 文档

完整文档位于 [**`docs/`**](docs/) —
[键盘快捷键](docs/reference/keyboard-shortcuts.mdx) ·
[config.json](docs/reference/configuration.mdx) ·
[CLI 参考](docs/cli/reference.mdx)。面向智能体的 CLI 接口也记录在
[skills/lyrel/SKILL.md](skills/lyrel/SKILL.md) 中。

使用以下命令安装技能：

```sh
npx skills add dlivxpr/lyrel    # 安装
npx skills update lyrel         # 后续更新
```

<a id="benchmarks"></a>
## 基准测试

测试工具与复现方法见 [`scripts/bench/`](scripts/bench/README.md)。性能结果须注明 lyrel 的实际构建、平台和测量条件；本页不提供未经本项目实测的比较成绩。

---

<div align="center">
<sub>

基于 [GPUI Kit](https://gpui-kit.com/) 和 [`rio-vt`](https://crates.io/crates/rio-vt/0.5.28) 构建 · [Apache-2.0](LICENSE) · [问题反馈](https://github.com/dlivxpr/lyrel/issues) · [更新日志](CHANGELOG.md)

来源致谢：本项目衍生自 l0ng-ai 的开源工作，保留 Copyright 2026 l0ng-ai 及 Apache-2.0 许可证声明；身份迁移不变更或移除上游及第三方的版权与许可。

</sub>
</div>