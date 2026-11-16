# Windows WebView2 SDK 许可

draw.io 宿主使用正式 registry 的 `webview2-com 0.39.1`／`webview2-com-sys 0.39.1`。
在 Windows MSVC 上，后者通过 `WebView2LoaderStatic.lib` 静态链接 SDK loader；
不需要旁加载 `WebView2Loader.dll`，也不应让 CLI、server 或普通测试依赖该 DLL。

SDK loader 仍受 Microsoft 的独立许可约束，不属于 Lyrel 的 Apache-2.0 许可。
此目录保留 SDK `Microsoft.Web.WebView2 1.0.3800.47` 的原始法律文本：

- 官方来源：<https://api.nuget.org/v3-flatcontainer/microsoft.web.webview2/1.0.3800.47/microsoft.web.webview2.1.0.3800.47.nupkg>。
- `LICENSE.txt` SHA-256：`0af8f1b807512aae39c2ac1aa4d0cae65cabecb6fd554b8439a5162a0d6eca55`。
- `NOTICE.txt` SHA-256：`106423785c5b7eba0a8e61d1837f2132e9c828e20ad530f565d981c1df60dd90`。

这两份原文同时收录到根 `LICENSE`，随既有 Windows ZIP／Inno 与 macOS
app／ZIP／DMG 的许可证链分发；无需新增 updater 管理路径或包根目录例外。

Windows 的 draw.io 预览仍需要系统已安装 WebView2 Runtime。应用不自动安装或
联网下载 Runtime；缺失时显示可恢复的预览错误，其他文件与终端功能不受影响。

