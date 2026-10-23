//! 仅用于无副作用探针的启动参数，不读取 lyrel Config。

use super::policy::Material;
use super::tokens::ProbeTheme;

pub(crate) const HELP: &str = "native_material_gate — #23 原生材质探针（非生产工作台）
启动：cargo run --locked --example native_material_gate -- [参数]
参数采用 --key value：
  --theme light|dark|custom       默认 dark
  --material opaque|transparent|blurred|auto|mica|mica-alt|acrylic
                                 默认 opaque；Windows 之外仅前三项
  --width 840..3840              默认 1440（逻辑像素）
  --height 600..2160             默认 900（逻辑像素）
  --zoom 75..150                默认 100（Kit rem 与字号一起缩放）
  --opacity 0.35..1             默认 0.76；opaque 强制有效 alpha=1
  --legacy-blur true|false      默认 true；仅 Windows Auto 消费
  --help                        打印此说明，不打开窗口
快捷键：F1 下一材质；F2 下一主题；F3 下一缩放；F4 下一透明度；
F5 重放原生 policy；F6 宽/窄窗口；F7 切换 Auto blur；F8 全屏；
Escape 返回探针根焦点；Tab/Shift+Tab 遍历 Kit 控件；Ctrl+Q / Cmd+Q 退出。
拖动 Kit 标题栏，拖动窗口原生边缘；Alt+Tab（macOS Cmd+Tab）切换激活状态。
期望与 API 边界见窗口说明及 examples/native_material_gate.rs 的模块说明。
";

pub(crate) struct Options {
    pub(crate) theme: ProbeTheme,
    pub(crate) material: Material,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) zoom: f32,
    pub(crate) opacity: f32,
    pub(crate) legacy_blur: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            theme: ProbeTheme::Dark,
            material: Material::Opaque,
            width: 1440.,
            height: 900.,
            zoom: 100.,
            opacity: 0.76,
            legacy_blur: true,
        }
    }
}

impl Options {
    pub(crate) fn parse() -> Result<Option<Self>, String> {
        let mut options = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(key) = args.next() {
            if key == "--help" {
                return Ok(None);
            }
            let value = args.next().ok_or_else(|| format!("{key} 缺少值"))?;
            match key.as_str() {
                "--theme" => {
                    options.theme = ProbeTheme::ALL.into_iter().find(|theme| theme.id() == value)
                        .ok_or_else(|| format!("不支持的主题：{value}"))?;
                }
                "--material" => {
                    options.material = Material::ALL.iter().copied().find(|material| material.id() == value)
                        .ok_or_else(|| format!("当前平台不支持的材质：{value}"))?;
                }
                "--width" => options.width = number(&key, &value, 840., 3840.)?,
                "--height" => options.height = number(&key, &value, 600., 2160.)?,
                "--zoom" => options.zoom = number(&key, &value, 75., 150.)?,
                "--opacity" => options.opacity = number(&key, &value, 0.35, 1.)?,
                "--legacy-blur" => options.legacy_blur = value.parse()
                    .map_err(|_| "--legacy-blur 需要 true 或 false".to_owned())?,
                _ => return Err(format!("未知参数：{key}")),
            }
        }
        Ok(Some(options))
    }
}

fn number(key: &str, value: &str, min: f32, max: f32) -> Result<f32, String> {
    let number: f32 = value.parse().map_err(|_| format!("{key} 需要数字"))?;
    if !number.is_finite() || !(min..=max).contains(&number) {
        return Err(format!("{key} 范围为 {min}..={max}"));
    }
    Ok(number)
}
