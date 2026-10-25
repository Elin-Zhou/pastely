//! DMG 安装器背景图渲染（AppKit 离屏位图，逻辑 880×560 @2x = 1760×1120 PNG）。
//!
//! 左区（37.5% 宽）：紫罗兰垂直渐变 + 白色剪贴板水印 + 右缘 2pt 分隔线；
//! 右区：#1A1A1E 底 + 标题 / 强调线 / 特性行 + 拖装指引（箭头 + 图标名标签）+ 底部提示。
//!
//! 逻辑坐标系 880×560（底左原点）。
//! Finder 的 DMG 背景图按「1 像素 = 1 点」原样铺放、不支持缩放（平台限制），
//! 因此 SCALE 必须保持 1.0：图像像素尺寸严格等于窗口点尺寸，否则会被裁切/错位。
//! 输出 PNG：`cargo run --bin gen_dmg_bg -- <输出路径>`（默认 dmg_bg.png）。
//! 无 NSApp 也可运行（离屏位图上下文，主线程 mtm）。

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGradient, NSGraphicsContext, NSStringDrawing,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSAttributedStringKey, NSCopying, NSDictionary, NSString};

/// 逻辑宽（pt）。
const W: f64 = 880.0;
/// 逻辑高（pt）。
const H: f64 = 560.0;
/// 渲染倍率：2x Retina，Finder 缩到逻辑窗口显示。
static mut SCALE: f64 = 1.0;

fn scale() -> f64 {
    unsafe { SCALE }
}
/// 左区宽（逻辑，37.5%）。
const LEFT_W: f64 = 330.0;

fn s(v: f64) -> f64 {
    v * scale()
}

fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r / 255.0, g / 255.0, b / 255.0, a)
}

/// 逻辑坐标 → 渲染位图矩形（统一 SCALE 入口）。
fn srect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(s(x), s(y)), CGSize::new(s(w), s(h)))
}

fn fill_rect(x: f64, y: f64, w: f64, h: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRect(srect(x, y, w, h)).fill();
}

fn stroke_rounded(x: f64, y: f64, w: f64, h: f64, radius: f64, width: f64, color: &NSColor) {
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        srect(x, y, w, h),
        s(radius),
        s(radius),
    );
    path.setLineWidth(s(width));
    color.setStroke();
    path.stroke();
}

fn fill_rounded(x: f64, y: f64, w: f64, h: f64, radius: f64, color: &NSColor) {
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        srect(x, y, w, h),
        s(radius),
        s(radius),
    );
    color.setFill();
    path.fill();
}

fn stroke_line(x0: f64, y0: f64, x1: f64, y1: f64, width: f64, color: &NSColor) {
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(s(width));
    path.moveToPoint(CGPoint::new(s(x0), s(y0)));
    path.lineToPoint(CGPoint::new(s(x1), s(y1)));
    color.setStroke();
    path.stroke();
}

fn stroke_circle(cx: f64, cy: f64, r: f64, width: f64, color: &NSColor) {
    stroke_rounded(cx - r, cy - r, 2.0 * r, 2.0 * r, r, width, color);
}

/// 白色剪贴板水印（板身描边 + 顶部夹子 + 三条内容线），居中于 (cx, cy)。
fn draw_clipboard(cx: f64, cy: f64, board_h: f64) {
    let white_solid = srgb(255.0, 255.0, 255.0, 0.92);
    let white_line = srgb(255.0, 255.0, 255.0, 0.85);
    let white_glow = srgb(255.0, 255.0, 255.0, 0.25);

    let board_w = board_h * 0.46 / 0.58; // 与应用图标同比例
    let x0 = cx - board_w / 2.0;
    let y0 = cy - board_h / 2.0;
    let y1 = cy + board_h / 2.0;
    let radius = board_h * 0.10;

    // 发光带（描边外柔和白圈）
    stroke_rounded(x0, y0, board_w, board_h, radius, 12.0 + 26.0, &white_glow);
    // 板身描边
    stroke_rounded(x0, y0, board_w, board_h, radius, 12.0, &white_solid);
    // 顶部夹子（骑在板身上边缘中央）
    let tab_w = board_w * 0.40;
    let tab_h = 52.0;
    fill_rounded(
        cx - tab_w / 2.0,
        y1 - tab_h / 2.0,
        tab_w,
        tab_h,
        18.0,
        &white_solid,
    );
    // 内容线：板身内三等分
    let line_w = board_w * 0.56;
    let line_x = cx - line_w / 2.0;
    let pad = 52.0;
    for k in 1..=3 {
        let ly = y0 + pad + (board_h - 2.0 * pad) * k as f64 / 4.0;
        stroke_line(line_x, ly, line_x + line_w, ly, 9.0, &white_line);
    }
}

/// 文本属性字典（字体 + 前景色）。
fn text_attrs(
    font: &NSFont,
    color: &NSColor,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    // SAFETY: 全局属性键常量，进程生命周期对象
    let font_key: &NSAttributedStringKey = unsafe { NSFontAttributeName };
    let color_key: &NSAttributedStringKey = unsafe { NSForegroundColorAttributeName };
    let font_any: &AnyObject = font;
    let color_any: &AnyObject = color;
    // SAFETY: 源对象均由调用方 Retained 持有，retain 在同帧内有效
    let retain_any = |o: &AnyObject| -> Retained<AnyObject> {
        unsafe { Retained::retain(o as *const AnyObject as *mut AnyObject) }.unwrap()
    };
    // SAFETY: 键常量为进程生命周期对象；NSAttributedStringKey=NSString 实现 NSCopying
    let retain_key = |k: &NSAttributedStringKey| -> Retained<ProtocolObject<dyn NSCopying>> {
        // SAFETY: 键常量为进程生命周期对象
        unsafe {
            Retained::retain(
                ProtocolObject::from_ref(k) as *const ProtocolObject<dyn NSCopying>
                    as *mut ProtocolObject<dyn NSCopying>,
            )
        }
        .unwrap()
    };

    let objects = NSArray::from_retained_slice(&[retain_any(font_any), retain_any(color_any)]);
    let keys = NSArray::from_retained_slice(&[retain_key(font_key), retain_key(color_key)]);
    // SAFETY: keys 与 objects 元素均为 NSCopying 的 NSString 常量，类型正确
    unsafe { NSDictionary::dictionaryWithObjects_forKeys(&objects, &keys) }
}

#[derive(Clone, Copy, PartialEq)]
enum TextKind {
    /// 系统加粗（标题）。
    Bold,
    /// Menlo 等宽（特性行 / 拖装提示）。
    Mono,
}

fn font_for(size: f64, kind: TextKind) -> Retained<NSFont> {
    match kind {
        TextKind::Bold => NSFont::boldSystemFontOfSize(s(size)),
        TextKind::Mono => NSFont::fontWithName_size(&NSString::from_str("Menlo-Regular"), s(size))
            .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(s(size), 0.0)),
    }
}

/// 在当前图形上下文绘制一行文本（逻辑坐标，底左原点）。
fn draw_text(text: &str, x: f64, y: f64, size: f64, kind: TextKind, color: &NSColor) {
    let attrs = text_attrs(&font_for(size, kind), color);
    let ns = NSString::from_str(text);
    // SAFETY: 属性字典类型正确（NSFont/NSColor 对应键）
    unsafe {
        NSStringDrawing::drawAtPoint_withAttributes(&*ns, CGPoint::new(s(x), s(y)), Some(&attrs))
    };
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "dmg_bg.png".to_string());
    let scale_factor: f64 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    unsafe {
        SCALE = scale_factor;
    }
    let mtm = MainThreadMarker::new().expect("必须主线程渲染");
    render(&out, mtm);
    println!("dmg background written: {out} (scale {scale_factor})");
}

fn render(out: &str, _mtm: MainThreadMarker) {
    // 离屏 RGBA 位图（逻辑 × scale()，bytesPerRow/bitsPerPixel 交由系统决定）
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            s(W) as isize,
            s(H) as isize,
            8,
            4,
            true,
            false,
            objc2_app_kit::NSDeviceRGBColorSpace,
            0,
            0,
        )
    }
    .expect("创建位图失败");
    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).expect("创建上下文失败");
    NSGraphicsContext::setCurrentContext(Some(&ctx));

    // —— 左区：垂直渐变（顶 indigo → 底 purple，angle -90 自上而下）——
    let gradient = NSGradient::initWithStartingColor_endingColor(
        NSGradient::alloc(),
        &srgb(99.0, 102.0, 241.0, 1.0),
        &srgb(168.0, 85.0, 247.0, 1.0),
    )
    .expect("创建渐变失败");
    gradient.drawInRect_angle(srect(0.0, 0.0, LEFT_W, H), -90.0);
    // 白色剪贴板水印
    draw_clipboard(LEFT_W / 2.0, H / 2.0, 300.0);
    // 右缘分隔线（白 alpha 0.25）
    fill_rect(LEFT_W - 2.0, 0.0, 2.0, H, &srgb(255.0, 255.0, 255.0, 0.25));

    // —— 右区：#1A1A1E 底 ——
    fill_rect(LEFT_W, 0.0, W - LEFT_W, H, &srgb(26.0, 26.0, 30.0, 1.0));

    // 标题：顶部 y=64（自上而下）→ 底左 y = H-64-68
    draw_text(
        "Pastely",
        LEFT_W + 80.0,
        H - 64.0 - 68.0,
        56.0,
        TextKind::Bold,
        &srgb(255.0, 255.0, 255.0, 1.0),
    );
    // 强调线：标题下 24pt，220×3，紫
    fill_rect(
        LEFT_W + 80.0,
        H - 156.0 - 3.0,
        220.0,
        3.0,
        &srgb(168.0, 85.0, 247.0, 1.0),
    );
    // 特性行：Menlo 15pt #98989F
    draw_text(
        "fuzzy search · ttl cleanup · auto paste",
        LEFT_W + 80.0,
        H - 175.0 - 20.0,
        18.0,
        TextKind::Mono,
        &srgb(178.0, 178.0, 186.0, 1.0),
    );

    // —— 拖装区中带：图标 96pt 中心 (480/680, 300)，箭头 (545..618, 300) ——
    // 名称标签不用背景图绘制：Finder 原生标签是矢量渲染（Retina 清晰），
    // 背景图 1px=1pt 必然柔化，中等字号文字最暴露——只保留大标题/大水印。
    let white_guide = srgb(255.0, 255.0, 255.0, 0.75);
    // 横线 → 空心圆圈 → 箭头头部
    stroke_line(545.0, 300.0, 585.0, 300.0, 3.0, &white_guide);
    stroke_circle(597.0, 300.0, 10.0, 3.0, &white_guide);
    stroke_line(611.0, 293.0, 619.0, 300.0, 3.0, &white_guide);
    stroke_line(611.0, 307.0, 619.0, 300.0, 3.0, &white_guide);

    // 底部拖装提示：y≈500（自上而下），底距 44pt 防裁切
    draw_text(
        "D R A G   T O   A P P L I C A T I O N S",
        LEFT_W + 80.0,
        H - 495.0 - 17.0,
        15.0,
        TextKind::Mono,
        &srgb(150.0, 150.0, 158.0, 1.0),
    );

    NSGraphicsContext::setCurrentContext(None);

    // 导出 PNG
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .expect("PNG 编码失败");
    let path = NSString::from_str(out);
    if !data.writeToFile_atomically(&path, true) {
        eprintln!("写入失败: {out}");
        std::process::exit(1);
    }
}
