//! DMG 安装器背景图渲染（AppKit 离屏位图，1280×800）。
//!
//! 左区：紫罗兰垂直渐变 + 白色剪贴板水印 + 右缘 2pt 分隔线；
//! 右区：#1A1A1E 底 + 标题 / 强调线 / 特性行 + 底部拖装提示。
//!
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

const W: f64 = 1280.0;
const H: f64 = 800.0;

fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r / 255.0, g / 255.0, b / 255.0, a)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

fn fill_rect(r: CGRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRect(r).fill();
}

fn stroke_rounded(r: CGRect, radius: f64, width: f64, color: &NSColor) {
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius);
    path.setLineWidth(width);
    color.setStroke();
    path.stroke();
}

fn fill_rounded(r: CGRect, radius: f64, color: &NSColor) {
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius);
    color.setFill();
    path.fill();
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
    stroke_rounded(
        rect(x0, y0, board_w, board_h),
        radius,
        12.0 + 26.0,
        &white_glow,
    );
    // 板身描边
    stroke_rounded(rect(x0, y0, board_w, board_h), radius, 12.0, &white_solid);
    // 顶部夹子（骑在板身上边缘中央）
    let tab_w = board_w * 0.40;
    let tab_h = 52.0;
    fill_rounded(
        rect(cx - tab_w / 2.0, y1 - tab_h / 2.0, tab_w, tab_h),
        18.0,
        &white_solid,
    );
    // 内容线：板身内三等分
    let line_w = board_w * 0.56;
    let line_x = cx - line_w / 2.0;
    let pad = 52.0;
    for k in 1..=3 {
        let ly = y0 + pad + (board_h - 2.0 * pad) * k as f64 / 4.0;
        let path = NSBezierPath::bezierPath();
        path.setLineWidth(9.0);
        path.moveToPoint(CGPoint::new(line_x, ly));
        path.lineToPoint(CGPoint::new(line_x + line_w, ly));
        white_line.setStroke();
        path.stroke();
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

/// 在当前图形上下文绘制一行文本（AppKit 底左坐标）。
fn draw_text(text: &str, x: f64, y: f64, font: &NSFont, color: &NSColor) {
    let attrs = text_attrs(font, color);
    let s = NSString::from_str(text);
    // SAFETY: 属性字典类型正确（NSFont/NSColor 对应键）
    unsafe { NSStringDrawing::drawAtPoint_withAttributes(&*s, CGPoint::new(x, y), Some(&attrs)) };
}

/// Menlo（系统缺失时退回等宽系统字体）。
fn menlo(size: f64) -> Retained<NSFont> {
    NSFont::fontWithName_size(&NSString::from_str("Menlo-Regular"), size)
        .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, 0.0))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "dmg_bg.png".to_string());
    let mtm = MainThreadMarker::new().expect("必须主线程渲染");
    render(&out, mtm);
    println!("dmg background written: {out}");
}

fn render(out: &str, _mtm: MainThreadMarker) {
    // 离屏 RGBA 位图（bytesPerRow/bitsPerPixel 交由系统决定）
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            1280,
            800,
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
    gradient.drawInRect_angle(rect(0.0, 0.0, 480.0, H), -90.0);
    // 白色剪贴板水印
    draw_clipboard(240.0, 400.0, 440.0);
    // 右缘分隔线（白 alpha 0.25）
    fill_rect(rect(478.0, 0.0, 2.0, H), &srgb(255.0, 255.0, 255.0, 0.25));

    // —— 右区：#1A1A1E 底 ——
    fill_rect(rect(480.0, 0.0, W - 480.0, H), &srgb(26.0, 26.0, 30.0, 1.0));

    // 标题 Pastely：顶部 y=80（自上而下）→ 底左坐标 800-80-68≈652
    let title_font = NSFont::boldSystemFontOfSize(56.0);
    draw_text(
        "Pastely",
        560.0,
        H - 80.0 - 68.0,
        &title_font,
        &srgb(255.0, 255.0, 255.0, 1.0),
    );
    // 强调线：标题下 24pt，220×3，紫
    fill_rect(
        rect(560.0, H - 174.0 - 3.0, 220.0, 3.0),
        &srgb(168.0, 85.0, 247.0, 1.0),
    );
    // 特性行：Menlo 15pt #98989F
    draw_text(
        "fuzzy search · ttl cleanup · auto paste",
        560.0,
        H - 193.0 - 18.0,
        &menlo(15.0),
        &srgb(152.0, 152.0, 159.0, 1.0),
    );
    // 底部拖装提示：Menlo 13pt #7C7C85
    draw_text(
        "D R A G   O N T O   A P P L I C A T I O N S   T O   I N S T A L L",
        560.0,
        H - 740.0 - 16.0,
        &menlo(13.0),
        &srgb(124.0, 124.0, 133.0, 1.0),
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
