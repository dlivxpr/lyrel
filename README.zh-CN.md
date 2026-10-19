<div align="center">

<img src="assets/app-icon.svg" alt="lyrel" width="88" height="88" />

<a id="lyrel"></a>
### lyrel

**终端工作台：会话常驻、远程开发、原生支持智能体。**

<sub>纯 Rust · GPUI Kit（GPUI）GPU 渲染 · 正式 Rio VT 内核</sub>

<br />

[![版本](https://img.shields.io/badge/version-0.1.0-3FDD8C)](https://github.com/dlivxpr/lyrel)
[![桌面平台](https://img.shields.io/badge/desktop-macOS%20%C2%B7%20Windows-blue)](https://github.com/dlivxpr/lyrel)
[![许可证](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

<sub>[项目说明](README.md) · 简体中文</sub>

</div>

## 为什么

shell 和窗格由后台常驻的服务器持有，而不是窗口。下面这些特性几乎都源自这一设计。

- **会话常驻**：退出应用，shell 照常运行；重启机器后，布局、屏幕内容和已支持的智能体对话都会回来。不需要 tmux
- **智能体感知**：omp、Kimi Code、pi、OpenCode 2.x、Hermes 的原生界面与状态集成；各项能力和正式运行验收分别记录，多个仓库一屏查看
- **可被智能体驱动**：一个智能体能给另一个开窗格、派活、等它跑完、读走结果，GUI 开不开都行
- **编辑器级输入**：建议、补全、高亮、历史搜索，不用装任何插件
- **远程开发**：文件、仓库、窗格和 git 信息都留在远端机器上，走自带的 SSH 栈

## 安装

lyrel 的独立初版版本为 **0.1.0**，目前尚无实际发行资产，也不宣称安装包已发布或签名已完成。[**Releases**](https://github.com/dlivxpr/lyrel/releases) 是未来稳定版的发行入口。缺少 lyrel 安装包时，不会回退下载其他项目的发行资产。

未来发行包的目标形式如下，具体以届时实际发布的资产为准：

| | | |
|---|---|---|
| **macOS**（Apple 芯片） | `…-macos-arm64.dmg` | 拖进「应用程序」 |
| **Windows** | `…-setup.exe` · 免安装 `….zip` | |

桌面端仅支持 macOS、Windows。Linux 保留 `lyrel-core`、`lyrel-server` 和 CLI；Windows／macOS 桌面连接 Linux 的远程开发与 WSL 仍受支持，不提供 Linux GUI、AppImage 或桌面 updater。


## 有什么

| | |
|---|---|
| **智能体感知** | 逐窗格识别五种内置产品 · 状态点 · 通知 · 分支 + 差异视图 · 需要输入时托盘图标提醒 · 重启后续上会话 · 在随处搜索里找回历史会话 · 一键启动 · 侧边栏按仓库分组，可置顶；各项能力分别验收 |
| **CLI + 技能** | 提供 `lyrel` CLI · [智能体技能](skills/lyrel/SKILL.md) · `run` 转发命令输出并原样返回退出码 · `split` · `send` · `wait --until free` · `capture` |
| **编辑器级输入** | 基于历史的内联补全建议 · Tab 补全附带说明 · 语法高亮 · 多行编辑 · 点击定位光标 · <kbd>⌃ R</kbd> 模糊搜索历史 |
| **窗口** | 标签页与分屏 · <kbd>⌘ P</kbd> 随处搜索 · <kbd>⌘ F</kbd> 回滚搜索 · 休眠标签页以释放内存 · 输入法 |
| **侧边面板** | <kbd>⌘ J</kbd> · 进程树与监听端口 · 文件树 · 改动与差异视图 · 在文件中查找 · GitHub 议题与拉取请求（只读，通过 `gh` 登录） |
| **Shell 集成** | 窗格启动时自动注入，不用你装什么 · 提示符边界 · 工作目录 · 退出码 · 命令跑完发通知 · 覆盖 zsh、bash、fish、PowerShell、WSL 和远程窗格 |
| **远程工作区** | 远端的文件、仓库、改动、差异视图、工作树、标签页和窗格 · 从任意客户端重连，接着离开时的位置继续 |
| **SSH** | 自带 russh 实现，不依赖外部 ssh：连接配置的凭据存入钥匙串 · SFTP 面板 · 端口转发 · 跳板机 · `lyrel-server` 只需安装一次，无需 root |

## 支持的智能体

内置身份只保留以下五种产品。命令识别、状态、历史、恢复、分叉及首条任务是不同能力，
不能仅凭识别名称或安装文件推定全部可用，也不能把静态实现当作正式 GUI／daemon／PTY 验收。

| 智能体 | 正式命令 | 实施基线 |
|---|---|---|
| omp（Oh My Pi） | `omp` | 18.12.1 |
| Kimi Code | `kimi` | 2.2.0 |
| pi | `pi` | 1.3.0 |
| OpenCode 2.x | `opencode`、`opencode2` | 2.0.32 |
| Hermes | `hermes` | 0.22.2 |

omp 与 pi 是独立产品。Kimi Code 不接管归档产品数据；OpenCode 只维护 2.x。
完整能力、限制和验收边界见[智能体概览](docs/agents/overview.mdx)及[会话操作](docs/agents/sessions.mdx)。
退役产品的旧配置不映射为其他产品；已有进程不会因名单缩减被终止，用户旧 hooks、历史和账号不被自动清理。

lyrel 不包装、不代理其中任何一个——你启动的就是那个智能体本身，运行在普通 PTY 中，界面仍然是它自己的。
如果你通过包装脚本启动智能体，在 `config.json` 的 `agent_commands` 里把脚本名映射到对应智能体即可。
PATH 上受支持的智能体也能在随处搜索里找到（**智能体：omp** 等），会在新标签页中启动；新建标签页菜单列出真正使用过的产品。`agent_launch` 可以设置启动命令行。

## 文档

完整文档在 [**`docs/`**](docs/)：
[快捷键](docs/reference/keyboard-shortcuts.mdx) ·
[config.json](docs/reference/configuration.mdx) ·
[CLI 参考](docs/cli/reference.mdx)。
智能体如何调用这套 CLI，另见 [skills/lyrel/SKILL.md](skills/lyrel/SKILL.md)。

安装技能：

```sh
npx skills add dlivxpr/lyrel    # 安装
npx skills update lyrel         # 后续更新
```

## 基准测试

测试工具与复现方法见 [`scripts/bench/`](scripts/bench/README.md)。性能结果须注明 lyrel 的实际构建、平台和测量条件；本页不提供未经本项目实测的比较成绩。

## 来源与许可证

lyrel 衍生自 l0ng-ai 的开源工作，感谢原作者与贡献者；保留 Copyright 2026 l0ng-ai，不将衍生成果宣称为原创。

本项目遵循 [Apache-2.0](LICENSE) 许可证，保留上游原始版权声明、许可证及适用的归属声明；身份迁移不改变或取代上游原有版权。

---

<div align="center">
<sub>

基于 [GPUI Kit](https://gpui-kit.com/) 和 [`rio-vt`](https://crates.io/crates/rio-vt/0.5.28) 构建 · [Apache-2.0](LICENSE) · [问题反馈](https://github.com/dlivxpr/lyrel/issues) · [更新日志](CHANGELOG.md)

</sub>
</div>