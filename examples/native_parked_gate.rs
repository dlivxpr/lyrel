// #23 独立原生能力探针；不链接应用入口，不读写用户配置，不启动终端。
// 设计唯一来源：docs/research/main-ui-draft.html:705-915,2160-2196。
// 操作：Tab 到牌堆，←/→、Home/End 选牌，Enter 翻面/恢复，Escape 关闭并回牌堆。
// F6 回牌堆；M reduced motion；P Park；T 明暗；-/+ zoom；V 原型/未获批可读候选；D 诊断。
// 受限高度：PageUp/PageDown 或滚轮；更多卡：增加至20，←/→/Home/End 或上一/下一组。
// 启动参数：--theme=light --zoom=1.5 --size=600x720 --reduced-motion --park --readable-preview
// Windows/macOS：cargo run --locked --example native_parked_gate
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "native_parked_gate/probe.rs"]
mod probe;

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn main() {
    probe::run();
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn main() {
    eprintln!("native_parked_gate requires Windows or macOS; Linux is not a native acceptance target");
    std::process::exit(1);
}
