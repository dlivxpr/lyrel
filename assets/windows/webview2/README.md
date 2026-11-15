# WebView2 loader

`WebView2Loader.dll` is the Microsoft WebView2 loader used by the draw.io
preview's direct WebView2 host (`webview2-com 0.39.1` /
`webview2-com-sys 0.39.1`).

- Source: the `webview2-com-sys 0.39.1` crate, `x64/WebView2Loader.dll`
  redistributed with the WebView2 SDK.
- Bytes: 160,880; SHA-256
  `86545b66cdb0603bc26b626fb9ad610cb6e71f28d468f5ea66df23b03dda96d5`.
- Terms: Microsoft WebView2 SDK license and redistributable terms,
  <https://aka.ms/webview2license>. The loader is not covered by Lyrel's
  Apache-2.0 license.
- Placement: `scripts/cargo-windows.ps1` copies this file beside every built
  executable, as with the pinned ConPTY files. Without it the WebView2 import
  library cannot find `CreateCoreWebView2EnvironmentWithOptions` at launch.

The dll is intentionally byte-pinned and ignored by Git's text conversion.
