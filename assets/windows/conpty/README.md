# 随包分发的 ConPTY

Microsoft 的可再分发伪控制台，与 `lyrel-app.exe` 一起安装，避免 Windows 窗格使用系统内置的 `conhost.exe`。

| 项目 | 内容 |
| --- | --- |
| 版本 | `1.25.260930003`（文件版本 `1.25.2609.30003`） |
| 来源 | nuget.org 上的 `Microsoft.Windows.Console.ConPTY`，也作为 [microsoft/terminal](https://github.com/microsoft/terminal/releases) 的 release 资产提供 |
| 许可证 | MIT，见 `LICENSE.txt`；打包时以 `LICENSE-ConPTY.txt` 名称暂存 |
| 完整性 | `manifest.json` 固定 nupkg 的 SHA-512（取自 NuGet catalog 的 `packageHash`）与每个文件的 SHA-256 |

**本目录不再保存二进制副本。** `conpty.dll` 与 `OpenConsole.exe` 由显式准备步骤取得：`crates/lyrel-core/build.rs` 为 GUI、server、CLI 及测试程序暂存同一输入，`.github/scripts/bundle-windows.ps1` 使用同一准备目录。构建与打包校验固定摘要；daemon 首次创建 pane 前也核验程序旁的文件对与 MIT 通知，缺失、损坏、架构或版本不匹配均明确报错，不静默退回系统 conhost。

## 准备

在仓库根目录运行（需要 PowerShell 7+）：

```powershell
pwsh scripts/prepare-conpty.ps1
```

脚本下载 `manifest.json` 固定的 nupkg、按其中 SHA-512／SHA-256 校验包与每个文件、核对 Authenticode 签名与文件版本，然后输出到 `<repo>/.conpty/`：

```text
.conpty/x64/{conpty.dll, OpenConsole.exe}
.conpty/LICENSE-ConPTY.txt   # MIT 通知在包中的暂存名
.conpty/manifest.json        # 本次准备所核对的清单副本
```

`.conpty/` 与 nupkg 缓存 `.conpty-cache/` 都在 `.gitignore` 中。`LYREL_CONPTY_DIR` 可指向共享准备目录；请在仓库根运行准备与打包命令。`LYREL_CONPTY_NUPKG` 指向缓存的 nupkg 文件或目录，`scripts/prepare-conpty.ps1 -Offline` 使用缓存或已准备目录重新校验，不下载。构建脚本只消费已核验输入，不联网。

## 使用原因

系统内置的 `conhost.exe` 会吞掉窗格进程发出的 `OSC 11` 背景色查询：序列无法到达终端模拟器，也就收不到回复。因此，依据终端背景选择深浅界面的应用（例如 Codex、Neovim 的背景检测）会在浅色主题下显示深色界面。

可再分发版本会转发这些查询，并将回复送回客户端；lyrel 的 `terminal::view` 已能根据当前主题回答 `OSC 10/11/12`。原始历史观察与当前构建的实际验证须分别记录。

以下是上游在 Windows 11 26200 上记录的历史观察：使用同一二进制，只在旁边加入这一对文件；不代表本次 lyrel 构建已完成运行验证。

```text
in-box conhost:  terminal side sees <ESC>[?9001h<ESC>[?1004h   -> client: no reply
sideloaded:      terminal side sees ... <ESC>]11;?<BEL>        -> client: rgb:efef/f1f1/f5f5
```

`portable-pty` 优先旁加载 `conpty.dll`，而不是使用 `kernel32` 提供的系统实现（`src/win/psuedocon.rs`）。DLL 搜索路径从运行程序所在目录开始，因此文件对与 `LICENSE-ConPTY.txt` 放在 GUI／server 程序旁。`crates/lyrel-core/build.rs` 同时暂存到 Cargo 的 profile 目录与 `deps/`，独立 `-p lyrel-core`、`-p lyrel-server`、`-p lyrel-cli` 构建也保持这一契约。运行时仅在首次创建 pane 前校验一次；恢复文件后需重启对应 daemon。

便携发行包另带 `ConPTY-manifest.json`，内容为该发行版本的 `manifest.json`。旧版本 updater 在整个下载 ZIP 通过校验和验证后，以包内目标清单核验 ConPTY 摘要、PE 架构、实际配对版本和 MIT 通知，因此可以验证后续发行采用的新官方包，而不是只接受旧 helper 编译时的固定版本。不能改用现有安装目录旁的独立清单作为信任来源，也不执行目标程序来完成验证；构建与 daemon 的编译期固定 pin 不变。

## 更新

两个文件是一个整体：Microsoft 支持成对使用，不支持单独替换。更新时修改 `manifest.json` 中的包版本、文件版本与完整性摘要，重新运行 `pwsh scripts/prepare-conpty.ps1`。当前发行只覆盖 x64 目标；新增架构时同步准备脚本、清单、`crates/lyrel-core/conpty_files.rs` 的架构映射与打包脚本，不将未发布的 x86 目标计为已验证。
