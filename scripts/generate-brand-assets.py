# /// script
# requires-python = ">=3.13"
# dependencies = ["pillow==13.0.0", "resvg-py==0.5.0"]
# ///
"""从已批准的 SVG 定稿重新生成正式品牌资源。运行：uv run scripts/generate-brand-assets.py。"""

from io import BytesIO
from pathlib import Path
import xml.etree.ElementTree as ET

from PIL import Image
from resvg_py import svg_to_bytes


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
DOCS = ROOT / "docs"
SVG_NS = "http://www.w3.org/2000/svg"
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICNS_SIZES = (32, 64, 128, 256, 512, 1024)


def render(svg: str, size: int) -> Image.Image:
    png = svg_to_bytes(
        svg_string=svg,
        width=size,
        height=size,
        skip_system_fonts=True,
    )
    with Image.open(BytesIO(png)) as image:
        return image.convert("RGBA")


def write_svg(path: Path, source: str) -> None:
    path.write_text(source, encoding="utf-8", newline="\n")
    print(f"SVG {path.relative_to(ROOT)}")


def save_image(image: Image.Image, path: Path, **options) -> None:
    image.save(path, **options)
    print(f"{path.suffix[1:].upper()} {path.relative_to(ROOT)} {image.width}x{image.height}")


def main() -> None:
    icon = (DOCS / "brand/lyrel-icon.svg").read_text(encoding="utf-8")
    mark = (DOCS / "brand/lyrel-mark.svg").read_text(encoding="utf-8")

    # 保留既有消费路径；所有彩色表面都直接使用定稿，不另存一份手工几何。
    for path in (ASSETS / "app-icon.svg", ASSETS / "logo.svg", DOCS / "logo/logo.svg"):
        write_svg(path, icon)
    # macOS 将透明单色图形作为 template image，最终前景由系统菜单栏提供。
    write_svg(ASSETS / "tray.svg", mark)

    color_images = {size: render(icon, size) for size in set(ICO_SIZES + ICNS_SIZES)}
    for name, size in (("app-icon.png", 1024), ("logo.png", 1024), ("logo@256.png", 256)):
        save_image(color_images[size], ASSETS / name)

    ico = BytesIO()
    color_images[256].save(
        ico,
        format="ICO",
        sizes=[(size, size) for size in ICO_SIZES],
        append_images=[color_images[size] for size in ICO_SIZES[:-1]],
    )
    for path in (ASSETS / "favicon.ico", DOCS / "favicon.ico"):
        path.write_bytes(ico.getvalue())
        print(f"ICO {path.relative_to(ROOT)} frames={ICO_SIZES}")

    # 文件名属于打包输入契约，不是产品或系统注册身份。
    save_image(
        color_images[1024],
        ASSETS / "lyrel.icns",
        append_images=[color_images[size] for size in ICNS_SIZES[:-1]],
    )

    # 保留既有社交分享资源的尺寸；背景从定稿读取，不引入第二套品牌色常量。
    tile = ET.fromstring(icon).find(f"{{{SVG_NS}}}rect")
    if tile is None:
        raise ValueError("彩色定稿缺少图标底色矩形")
    social = Image.new("RGBA", (2560, 1280), tile.attrib["fill"])
    social.alpha_composite(color_images[1024], (768, 128))
    save_image(social.convert("RGB"), ASSETS / "social-preview.png")


if __name__ == "__main__":
    main()
