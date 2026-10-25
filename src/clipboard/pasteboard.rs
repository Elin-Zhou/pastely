//! NSPasteboard 封装：changeCount 轮询原语 + 文本/图片读写。
//!
//! # Safety
//!
//! 全部 AppKit 调用限定主线程（watcher NSTimer 与确认流程均在主线程）。

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeString,
};
use objc2_foundation::{NSData, NSDictionary, NSString};

/// 当前系统剪切板的 changeCount（O(1) 整数，轮询变化的原生原语）。
pub fn change_count() -> i64 {
    NSPasteboard::generalPasteboard().changeCount() as i64
}

/// 读文本；无文本返回 `None`。
pub fn read_text() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let ty: &'static objc2_app_kit::NSPasteboardType = unsafe { NSPasteboardTypeString };
    let s = pb.stringForType(ty)?;
    Some(s.to_string())
}

/// 读图片：优先 PNG 原始数据；否则 NSImage 读入后经 TIFF→PNG 转换。
/// 返回 `(png 字节, 宽 px, 高 px)`。
pub fn read_image() -> Option<(Vec<u8>, u32, u32)> {
    {
        let pb = NSPasteboard::generalPasteboard();
        let png_type: &'static objc2_app_kit::NSPasteboardType = unsafe { NSPasteboardTypePNG };
        if let Some(data) = pb.dataForType(png_type) {
            if let Some(rep) = NSBitmapImageRep::imageRepWithData(&data) {
                return Some((
                    data.to_vec(),
                    rep.pixelsWide() as u32,
                    rep.pixelsHigh() as u32,
                ));
            }
        }
        // 退化路径：剪切板内容当 NSImage 读，TIFF rep 转 PNG
        let image: Retained<NSImage> =
            NSImage::initWithPasteboard(<NSImage as AnyThread>::alloc(), &pb)?;
        let tiff = image.TIFFRepresentation()?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
        // SAFETY: 纯数据转换，无空指针/线程约束
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some((
            png.to_vec(),
            rep.pixelsWide() as u32,
            rep.pixelsHigh() as u32,
        ))
    }
}

/// 写文本（清空其余类型）。
pub fn write_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let ns = NSString::from_str(text);
    let string_type: &'static objc2_app_kit::NSPasteboardType = unsafe { NSPasteboardTypeString };
    pb.setString_forType(&ns, string_type);
}

/// 写 PNG 图片（清空其余类型）。
pub fn write_image_png(png: &[u8]) {
    {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: 指针与长度来自切片，区间有效
        let data = unsafe { NSData::dataWithBytes_length(png.as_ptr().cast(), png.len()) };
        let png_type: &'static objc2_app_kit::NSPasteboardType = unsafe { NSPasteboardTypePNG };
        if !pb.setData_forType(Some(&data), png_type) {
            tracing::warn!(
                "write_image_png: setData_forType 失败（{} 字节）",
                png.len()
            );
        }
    }
}
