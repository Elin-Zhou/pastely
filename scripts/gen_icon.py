#!/usr/bin/env python3
"""GlassClip 应用图标生成（纯标准库：zlib/struct 手写 PNG，无第三方依赖）。

绘制 1024×1024：圆角方块底（#4A7DFF 纯色，圆角近似 macOS squircle）
+ 中央白色剪贴板轮廓（圆角矩形描边）+ 顶部实心夹子矩形。
输出单张 1024 PNG；各尺寸由 scripts/bundle_app.sh 用 sips 缩放生成 iconset。

用法：python3 scripts/gen_icon.py <输出.png 路径>
"""

import struct
import sys
import zlib

SIZE = 1024
RADIUS = 180            # 应用圆角（macOS squircle 的近似）
BG = (74, 125, 255)     # #4A7DFF
WHITE = (255, 255, 255)

# 剪贴板主体（描边）与顶部夹子（实心）；坐标为 (x0, y0, x1, y1)，y 向下
CLIP = (300, 290, 724, 850)
CLIP_R = 56
STROKE = 30
TAB = (432, 230, 592, 320)
TAB_R = 24


def in_rounded_rect(x: float, y: float, x0: float, y0: float, x1: float, y1: float, r: float) -> bool:
    """点是否在圆角矩形内。"""
    if not (x0 <= x <= x1 and y0 <= y <= y1):
        return False
    dx = max(x0 + r - x, 0.0, x - (x1 - r))
    dy = max(y0 + r - y, 0.0, y - (y1 - r))
    return dx * dx + dy * dy <= r * r


def in_rounded_border(x: float, y: float, x0: float, y0: float, x1: float, y1: float, r: float, w: float) -> bool:
    """点是否落在圆角矩形的描边带内（外沿内、内沿外）。"""
    outer = in_rounded_rect(x, y, x0, y0, x1, y1, r)
    inner = in_rounded_rect(x, y, x0 + w, y0 + w, x1 - w, y1 - w, max(r - w, 0.0))
    return outer and not inner


def render(size: int) -> list[bytes]:
    rows: list[bytes] = []
    for py in range(size):
        # PNG 每行前置 filter 字节（0 = None）
        row = bytearray([0])
        for px in range(size):
            # 采样点归一化到 1024 设计空间
            x = (px + 0.5) * SIZE / size
            y = (py + 0.5) * SIZE / size
            if not in_rounded_rect(x, y, 0, 0, SIZE, SIZE, RADIUS):
                row += b"\x00\x00\x00\x00"  # 圆角外透明
            elif in_rounded_border(x, y, *CLIP, CLIP_R, STROKE) or in_rounded_rect(x, y, *TAB, TAB_R):
                row += bytes((*WHITE, 255))
            else:
                row += bytes((*BG, 255))
        rows.append(bytes(row))
    return rows


def write_png(path: str, size: int, rows: list[bytes]) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    # IHDR: 宽、高、位深 8、颜色类型 6 (RGBA)、压缩 0、滤波 0、隔行 0
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(b"".join(rows), 9)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", idat))
        f.write(chunk(b"IEND", b""))


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "icon_1024x1024.png"
    write_png(out, SIZE, render(SIZE))
    print(f"icon written: {out} ({SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
