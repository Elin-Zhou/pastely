//! 列表行构建：文本高亮 / 图片缩略图 / 相对时间 / 选中态。

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSFont, NSImage, NSImageView,
    NSLineBreakMode, NSTableCellView, NSTextField,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSMutableAttributedString, NSString};

use crate::model::{ClipKind, SearchableItem};

/// 行高（pt）。
pub const ROW_HEIGHT: f64 = 32.0;
/// 图片缩略图边长（pt）。
pub const THUMB_SIZE: f64 = 28.0;
/// 时间列宽（pt）。
pub const META_WIDTH: f64 = 72.0;
/// 行首序号徽章宽度（pt）。
/// 内容区（文本/缩略图）起始 x（徽章 6 + 18 + 2 间隙）。
pub const CONTENT_X: f64 = 8.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 相对时间描述：刚刚 / N 分钟前 / N 小时前 / N 天前。
pub fn relative_time(created_ms: i64, now: i64) -> String {
    let delta = (now - created_ms).max(0) / 1000; // 秒
    if delta < 60 {
        "刚刚".to_string()
    } else if delta < 3_600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86_400 {
        format!("{} 小时前", delta / 3_600)
    } else {
        format!("{} 天前", delta / 86_400)
    }
}

/// 把 preview 的 **char** 偏移高亮集合转为 UTF-16 单位的连续区间
/// （NSAttributedString 的 range 语义是 UTF-16；增补平面字符占 2 单位）。
pub fn char_indices_to_utf16_ranges(s: &str, char_indices: &[usize]) -> Vec<(u64, u64)> {
    let mut hits: Vec<(usize, usize)> = Vec::with_capacity(char_indices.len()); // (char_idx, utf16_off)
    let mut u16_off = 0usize;
    for (ci, ch) in s.chars().enumerate() {
        if hits.len() < char_indices.len() && ci == char_indices[hits.len()] {
            hits.push((ci, u16_off));
            if hits.len() == char_indices.len() {
                break;
            }
        }
        u16_off += ch.len_utf16();
    }
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for &(ci, off) in &hits {
        let len = s.chars().nth(ci).map_or(1, |c| c.len_utf16());
        match ranges.last_mut() {
            Some(last) if last.0 + last.1 == off as u64 => last.1 += len as u64,
            _ => ranges.push((off as u64, len as u64)),
        }
    }
    ranges
}

/// 带高亮的预览富文本（命中字符以系统强调色着色）。
///
/// # Safety
///
/// 主线程调用（NSColor 类方法约束）。
pub unsafe fn highlighted_preview(
    preview: &str,
    char_indices: &[usize],
) -> Retained<NSMutableAttributedString> {
    unsafe {
        let s = NSMutableAttributedString::initWithString(
            <NSMutableAttributedString as AnyThread>::alloc(),
            &NSString::from_str(preview),
        );
        let color = NSColor::systemBlueColor();
        for (location, length) in char_indices_to_utf16_ranges(preview, char_indices) {
            s.addAttribute_value_range(
                objc2_app_kit::NSForegroundColorAttributeName,
                &color,
                objc2_foundation::NSRange::new(location as usize, length as usize),
            );
        }
        s
    }
}

/// 构造一行 view-based 单元格（左栏 320pt 宽）。
///
/// # Safety
///
/// 主线程调用（mtm 保证）。
pub unsafe fn build_row(
    mtm: MainThreadMarker,
    view_width: f64,
    item: &SearchableItem,
    char_indices: &[usize],
    png_bytes: Option<&[u8]>,
    selected: bool,
) -> Retained<NSTableCellView> {
    unsafe {
        let cell = NSTableCellView::new(mtm);

        // 行级选中高亮（Text/Image 统一：cell layer 背景，图片行同样有反馈）
        if selected {
            cell.setWantsLayer(true);
            if let Some(layer) = cell.layer() {
                let soft = NSColor::controlAccentColor().colorWithAlphaComponent(0.18);
                layer.setBackgroundColor(Some(&soft.CGColor()));
                layer.setCornerRadius(4.0);
            }
        }

        let right = 8.0;
        match item.kind {
            ClipKind::Image => {
                let mut x = CONTENT_X;
                if let Some(bytes) = png_bytes {
                    let data = objc2_foundation::NSData::dataWithBytes_length(
                        bytes.as_ptr().cast(),
                        bytes.len(),
                    );
                    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
                        let iv = NSImageView::new(mtm);
                        iv.setImage(Some(&img));
                        iv.setFrame(rect(
                            x,
                            (ROW_HEIGHT - THUMB_SIZE) / 2.0,
                            THUMB_SIZE,
                            THUMB_SIZE,
                        ));
                        iv.setImageScaling(
                            objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown,
                        );
                        cell.addSubview(&iv);
                        x += THUMB_SIZE + 8.0;
                    }
                }
                let (w, h) = item.img_size.unwrap_or((0, 0));
                let label = NSTextField::labelWithString(
                    &NSString::from_str(&format!("图片 {w}×{h}")),
                    mtm,
                );
                label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                label.setFrame(rect(
                    x,
                    (ROW_HEIGHT - 18.0) / 2.0,
                    (view_width - x - META_WIDTH - right).max(40.0),
                    18.0,
                ));
                cell.addSubview(&label);
                cell.setTextField(Some(&label));
            }
            ClipKind::Text => {
                let label =
                    NSTextField::labelWithString(&NSString::from_str(&item.text_preview), mtm);
                label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                label.setAttributedStringValue(&highlighted_preview(
                    &item.text_preview,
                    char_indices,
                ));
                label.setFrame(rect(
                    CONTENT_X,
                    (ROW_HEIGHT - 18.0) / 2.0,
                    (view_width - CONTENT_X - META_WIDTH - right).max(40.0),
                    18.0,
                ));
                cell.addSubview(&label);
                cell.setTextField(Some(&label));
            }
        }
        let time = NSTextField::labelWithString(
            &NSString::from_str(&relative_time(item.created_at, crate::model::now_ms())),
            mtm,
        );
        time.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        time.setTextColor(Some(&NSColor::secondaryLabelColor()));
        time.setFrame(rect(
            view_width - META_WIDTH - right,
            (ROW_HEIGHT - 16.0) / 2.0,
            META_WIDTH,
            16.0,
        ));
        cell.addSubview(&time);
        cell
    }
}

/// 存储字节 → 展示 PNG（存储即 PNG 直通；旧数据经 NSBitmapImageRep 转换）。
pub fn png_for_display(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some(bytes.to_vec());
    }
    unsafe {
        let data =
            objc2_foundation::NSData::dataWithBytes_length(bytes.as_ptr().cast(), bytes.len());
        NSBitmapImageRep::imageRepWithData(&data).and_then(|rep| {
            rep.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &objc2_foundation::NSDictionary::new(),
            )
            .map(|d| d.to_vec())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_buckets() {
        let now = 1_000_000_000_000;
        assert_eq!(relative_time(now - 30_000, now), "刚刚");
        assert_eq!(relative_time(now - 5 * 60_000, now), "5 分钟前");
        assert_eq!(relative_time(now - 3 * 3_600_000, now), "3 小时前");
        assert_eq!(relative_time(now - 2 * 86_400_000, now), "2 天前");
        assert_eq!(relative_time(now + 9_000, now), "刚刚", "未来时间按 0 处理");
    }

    #[test]
    fn utf16_ranges_ascii_contiguous() {
        let r = char_indices_to_utf16_ranges("hello", &[0, 1, 2]);
        assert_eq!(r, vec![(0, 3)]);
    }

    #[test]
    fn utf16_ranges_split_when_discontiguous() {
        let r = char_indices_to_utf16_ranges("hello", &[0, 2, 4]);
        assert_eq!(r, vec![(0, 1), (2, 1), (4, 1)]);
    }

    #[test]
    fn utf16_ranges_astral_emoji_counts_two_units() {
        // 😀 占 2 个 UTF-16 单元：高亮第 2 个 char（'A'）应在 location 2
        let r = char_indices_to_utf16_ranges("😀A😀", &[1]);
        assert_eq!(r, vec![(2, 1)]);
    }

    #[test]
    fn utf16_ranges_out_of_bounds_ignored() {
        assert!(char_indices_to_utf16_ranges("ab", &[5]).is_empty());
    }

    #[test]
    fn png_passthrough() {
        let png_header = [0x89, b'P', b'N', b'G', 1, 2];
        assert_eq!(png_for_display(&png_header), Some(png_header.to_vec()));
        // 非法数据走 NSBitmapImageRep 转换路径返回 None，不 panic
        assert!(png_for_display(&[0, 1, 2]).is_none());
    }
}
