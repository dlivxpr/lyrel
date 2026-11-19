---
name: lyrel-development
description: >-
  在 Lyrel 仓库构建、测试、启动开发实例或验证产品改动时使用。
  覆盖 Windows ConPTY 与 Cargo wrapper、桌面和 headless 测试、Linux Docker/WSL、
  native 字形消费者及运行验收；只读规则整理不触发产品构建。
---

# Lyrel 开发与验证

路径和命令均相对仓库根。先读根 `AGENTS.md`；平台前提以 `docs/getting-started/installation.mdx`、当前 Cargo 配置和 CI 为准。

## 选择验证范围

1. 根据改动选择最小相关检查和真实运行场景，不默认运行整个 workspace。只改文档、规则或 Skill 时检查其引用、加载和行为边界，不启动无关产品构建。
2. Rust 使用 edition 2024 与 stable；仓库未固定具体 toolchain/MSRV，不把本机版本写成要求。使用现有 lockfile 和 `--locked`。
3. 修改 UI 时先使用 `gpui-kit`、`gpui-kit-design-guides` 并读 `docs/design-system.md`；编辑 Markdown editor 前读 `crates/lyrel-markdown-editor/HOST_CAPABILITIES.md`。
4. 准备隔离配置、目标平台运行时和外部工具后再运行。前提缺失不能通过跳过用例伪装成完整通过。

## Windows 与 ConPTY

- 首次构建或准备目录失效时执行 `pwsh scripts/prepare-conpty.ps1`。固定包、完整性摘要、文件对和更新契约见 `assets/windows/conpty/README.md`；构建脚本只核验输入，不联网下载。
- 源码构建使用 `pwsh scripts/cargo-windows.ps1 build --locked`，测试使用 `pwsh scripts/cargo-windows.ps1 test --locked`。下表的 `cargo build` / `cargo test` 在 Windows 均替换为对应 wrapper，其余参数不变。
- wrapper 从 Cargo JSON 的 `compiler-artifact.executable` 取得实际产物，再暂存 `conpty.dll`、`OpenConsole.exe` 和 `LICENSE-ConPTY.txt`；不要依赖固定 `deps/` 或 `OUT_DIR` 层数。`cargo dev` / `cargo run` 前先完成同配置、同目标的 wrapper build。
- 默认输入为 `.conpty/`，`LYREL_CONPTY_DIR` 可覆盖；离线准备与缓存语义按上述 README。Windows MSVC 保留 `+crt-static`，不要用 `RUSTFLAGS` 意外覆盖。
- 当前 Windows ConPTY 与 Windows server 仅支持 x86_64；远端 ARM64 Windows 明确拒绝。安装器的 x64 仿真兼容不等于发布了 ARM64 目标。
- daemon 创建 pane 前对配对文件、许可、架构和版本 fail-closed；缺失或损坏不能静默回退到系统 ConPTY。发行包还需核对包内 `ConPTY-manifest.json`，不能用现有安装旁的独立清单替换信任来源。
- 终端真实 PTY 回归在非标准 Git Bash 安装（如 Scoop）下需指定 `LYREL_TEST_GIT_BASH`。自定义 Cargo 产物目录若无法自动识别，可先经 wrapper 构建 `--locked -p lyrel-test-support --bin lyrel-graphics-fixture`，再以 `LYREL_GRAPHICS_FIXTURE_BIN` 指定实际 exe；测试仍会验证源码身份。
- 保留必需的 `.conpty/` 构建资产，不把它当作本次可随手清理的临时目录。

## 常用命令

| 目的 | 命令 |
| --- | --- |
| 桌面 Debug / Release 构建 | `cargo build --locked` / `cargo build --release --locked` |
| 隔离桌面实例 | `cargo dev` |
| CLI 用法 | `cargo run --locked -p lyrel-cli -- --help` |
| 隔离 headless daemon | `cargo run --locked -p lyrel-server -- --daemon --config-dir .lyrel-dev` |
| 默认成员 / 全成员测试 | `cargo test --locked` / `cargo test --workspace --locked` |
| 同步 wire／Host 协议 | `cargo t-protocol`（Windows：`pwsh scripts/cargo-windows.ps1 t-protocol`） |
| core / CLI E2E | `cargo test --locked -p lyrel-core` / `cargo test --locked -p lyrel-cli --test cli_e2e` |
| 文本 codec / 文档格式引擎 | `cargo test --locked -p lyrel-document --no-default-features` / `cargo test --locked -p lyrel-document --all-features` |
| 桌面 library 测试 | `cargo test --locked -p lyrel -p lyrel-ui-base -p lyrel-terminal -p lyrel-workbench-editor --lib` |
| 桌面 updater 测试 | `cargo test --locked -p lyrel-updater` |
| native Markdown / 终端 | `cargo test --locked -p lyrel --features native-markdown-tests --test native_markdown` / `cargo test --locked -p lyrel --features native-markdown-tests --test native_terminal` |
| 格式检查 / 写回 | `cargo fmt --check` / `cargo fmt` |
| Host 边界 | `bash .github/scripts/check-host-boundary.sh` |
| Workspace 生产依赖图 | `node .github/scripts/workspace-graph.cjs` |
| 补充 lint | `cargo clippy --workspace --all-targets --locked` |

Windows/macOS 默认构建包含桌面应用、独立 updater 和 Markdown editor；桌面应用由 `lyrel-ui-base`、`lyrel-terminal` 与 `lyrel-workbench-editor` 支撑。两个 native 字形消费者属于非默认 `native-markdown-tests`，不能由默认测试替代。CI 覆盖范围及超时以 `.github/workflows/ci.yml` 为准，补充 Clippy 不冒称 CI 必跑项；仓库未设覆盖率百分比门槛。

Host 边界脚本要求 LF；遇到 `$'\r': command not found` 或 `invalid option name` 时先核对行尾，不能扩大 allowlist 掩盖。补全生成读 `scripts/fig-convert/README.md`；性能验证读 `scripts/bench/README.md`，后者依赖 macOS/zsh。

## Linux

- 优先使用兼容目标 libc、依赖和运行时的轻量 Docker Linux 容器，参照 CI 准备完整环境。仅在 Docker 不可用或容器不能覆盖目标行为（如 WSL 专项）时回退 WSL，并记录原因。
- Linux headless 的 `cargo build/test --locked --workspace --all-features` 排除 `lyrel`、`lyrel-markdown-editor`、`lyrel-updater`、`lyrel-ui-base`、`lyrel-terminal` 与 `lyrel-workbench-editor`；完整命令见安装指南。文档格式引擎、insights 的 skills／统计／GitHub 查询及共享测试夹具参与 headless 验证；用量账户与浏览器读取器只在桌面构建。
- 完整 features 需要 MIT Kerberos 开发依赖及 `LIBGSSAPI_IMPL=mit`。Pi 桥契约需要当前平台 Node 22.19+ 在 `PATH`，不能用 Windows 的 `node.exe` 替代 Linux Node。
- 很多 server 集成用例受 `cfg(unix)` 限制，Windows 通过不能替代 Unix 验证；联网及真实 SSH/LSP 的 ignored 用例只在具备前提时显式执行。

## 测试与实际验收

- 普通桌面单元测试归 library，不以 `--bin lyrel-app` 的零用例结果代替。GUI 使用 `#[gpui_kit::test]` / `TestAppContext`，异步集成使用 `#[tokio::test]`。
- CLI E2E 和 native 字形测试使用 `harness = false`。native 消费者须在 OS 主线程初始化平台，不能移回 libtest worker；测试支持 feature 不扩大成默认产品 feature。
- Host 契约复用 `crates/lyrel-core/src/host/conformance.rs`，由 `cfg(test)` 或 `test-support` 启用。skills 和统计的本地与真实 stdio 消费者回归位于 `lyrel-server` 的 `insights_conformance` 测试目标；stdio 部分仅在 Unix 执行。集成用例用 `CARGO_BIN_EXE_*` 的真实二进制、`tempfile::TempDir`、隔离 socket/config 与子进程清理；GUI 写配置前调用 `lyrel-ui-base` 的 `pin_test_config_dir()`。
- 本次 Cargo 检查、测试和构建以 zero-warning 为交付标准，已有 warning 逐条说明理由。行为变更还需运行受影响路径，保留命令、退出码和关键日志。
- 终端、CLI、headless 优先用可复现脚本或后台进程；桌面验证优先后台窗口自动化，只有依赖前台焦点或后台无法完成时才申请所需授权。
- 按真实目标验收：截图或 headless 通过不证明原生交互、SSH、中文 IME、安装或在线更新通过。报告已运行、跳过、前提缺失和未验证项。
