#!/usr/bin/env python3
"""Pastely 应用图标生成（纯标准库：zlib/struct 手写 PNG，无第三方依赖）。

设计：柔和紫罗兰垂直渐变（#6366F1 → #A855F7）+ 白色剪贴板简形
（板身描边 + 顶部夹子实心 + 三条内容线 + 描边外发光）。

实现：4× 超采样（4096²）逐像素判定后盒式滤波降采样到 1024²（平滑边缘）。
背景全幅方形——macOS 自动套圆角遮罩，无需自绘圆角。
输出单张 1024 PNG；各尺寸由 scripts/bundle_app.sh 用 sips 缩放生成 iconset。

用法：python3 scripts/gen_icon.py <输出.png 路径>
"""

import struct
import sys
import zlib

SS = 4                     # 超采样倍数
OUT = 1024                 # 输出边长
SIZE = OUT * SS            # 绘制边长（4096）

GRAD_TOP = (99, 102, 241)   # #6366F1 indigo-500
GRAD_BOTTOM = (168, 85, 247)  # #A855F7 purple-500
WHITE = (255, 255, 255)

# 板身：宽 46% 高 58%，圆角 10%，白色描边
BOARD_W, BOARD_H, BOARD_R = 0.46, 0.58, 0.10
STROKE = 56.0               # 线宽（4096 尺度）
GLOW_EXTRA = 40.0           # 发光带额外宽度
# 夹子：宽 18% 高 7%，板身上边缘中央，实心
TAB_W, TAB_H, TAB_R = 0.18, 0.07, 0.04
# 内容线：3 条，宽 26%，线宽 24，板身内三等分
LINE_W, LINE_STROKE = 0.26, 24.0
LINE_ALPHA = 0.85
GLOW_ALPHA = 0.25


def in_rounded_rect(x: float, y: float, x0: float, y0: float, x1: float, y1: float, r: float) -> bool:
    """点是否在圆角矩形内（几何坐标 0..1，y 向下）。"""
    if not (x0 <= x <= x1 and y0 <= y <= y1):
        return False
    dx = max(x0 + r - x, 0.0, x - (x1 - r))
    dy = max(y0 + r - y, 0.0, y - (y1 - r))
    return dx * dx + dy * dy <= r * r


def in_rounded_border(x, y, x0, y0, x1, y1, r, w):
    """点是否落在圆角矩形描边带内（外沿内、内沿外）。"""
    outer = in_rounded_rect(x, y, x0, y0, x1, y1, r)
    inner = in_rounded_rect(x, y, x0 + w, y0 + w, x1 - w, y1 - w, max(r - w, 0.0))
    return outer and not inner


def geometry():
    """剪贴板几何（0..1 归一化，y 向下）。"""
    cx = 0.5
    top = (1.0 - BOARD_H) / 2.0
    board = (cx - BOARD_W / 2, top, cx + BOARD_W / 2, top + BOARD_H)
    tab = (
        cx - TAB_W / 2,
        top - TAB_H / 2,  # 骑在板身上边缘
        cx + TAB_W / 2,
        top + TAB_H / 2,
    )
    # 内容线：板身内三等分（避开描边带，留边距）
    pad = 0.08
    y0 = board[1] + pad
    y1 = board[3] - pad
    lines = [y0 + (y1 - y0) * k / 4.0 for k in (1, 2, 3)]
    return board, tab, lines


def render() -> list[bytes]:
    """4× 超采样逐像素判定；返回 1024² RGBA 行列表（已盒式降采样）。"""
    board, tab, lines = geometry()
    buf = [[(0, 0, 0, 0) for _ in range(OUT)] for _ in range(OUT)]

    for py in range(SIZE):
        gy = (py + 0.5) / SIZE  # 归一化几何坐标（y 向下）
        # 垂直渐变插值（逐绘制行预计算）
        t = gy
        grad_row = tuple(
            round(a + (b - a) * t) for a, b in zip(GRAD_TOP, GRAD_BOTTOM)
        )
        srow = buf[py // SS]
        for px in range(SIZE):
            gx = (px + 0.5) / SIZE
            # 优先级：夹子实心 > 板身描边 > 内容线 > 发光带 > 渐变底
            if in_rounded_rect(gx, gy, *tab, TAB_R):
                color, alpha = WHITE, 255
            elif in_rounded_border(gx, gy, *board, BOARD_R, STROKE / SIZE):
                color, alpha = WHITE, 255
            else:
                hit_line = False
                for ly in lines:
                    half = LINE_STROKE / SIZE / 2.0
                    if (
                        0.5 - LINE_W / 2 <= gx <= 0.5 + LINE_W / 2
                        and ly - half <= gy <= ly + half
                        and in_rounded_rect(gx, gy, *board, BOARD_R)
                    ):
                        hit_line = True
                        break
                if hit_line:
                    color, alpha = WHITE, round(255 * LINE_ALPHA)
                elif in_rounded_border(
                    gx, gy, *board, BOARD_R, (STROKE + GLOW_EXTRA) / SIZE
                ):
                    # 描边外发光：柔和白圈
                    color, alpha = WHITE, round(255 * GLOW_ALPHA)
                else:
                    color, alpha = grad_row, 255
            # 盒式滤波累加：直接写入所属输出像素（同超采样格内等权覆盖即可——
            # 每输出像素恰好 SS×SS 个源像素，最后一个写前做平均成本高；
            # 改为累加再除：用整型累加）
            out_px = px // SS
            r_, g_, b_, a_ = srow[out_px]
            srow[out_px] = (r_ + color[0], g_ + color[1], b_ + color[2], a_ + alpha)

    # 平均化
    rows: list[bytes] = []
    n = SS * SS
    for row in buf:
        out_row = bytearray([0])  # PNG filter: None
        for r_, g_, b_, a_ in row:
            out_row += bytes((r_ // n, g_ // n, b_ // n, a_ // n))
        rows.append(bytes(out_row))
    return rows


def write_png(path: str, size: int, rows: list[bytes]) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(b"".join(rows), 9)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", idat))
        f.write(chunk(b"IEND", b""))


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "icon_1024x1024.png"
    write_png(out, OUT, render())
    print(f"icon written: {out} ({OUT}x{OUT})")


if __name__ == "__main__":
    main()
