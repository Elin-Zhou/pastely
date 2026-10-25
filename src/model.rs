//! 剪贴板条目数据模型：类型枚举、写入载荷、列表查询视图与预览规则。

/// 条目类型。`as_str` 是 SQL 存储值与所有条件判断的唯一来源（禁止魔法字符串）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Image,
}

impl ClipKind {
    /// SQL 存储值与所有条件判断的唯一来源（规约：禁止魔法字符串）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }

    /// `as_str` 的逆映射：DB 读取 / 外部输入 → 枚举。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "image" => Some(Self::Image),
            _ => None,
        }
    }
}

/// 写入载荷（clipboard → store）。`hash` 为内容 SHA-256 前 16 字节 hex（32 字符）。
#[derive(Debug, Clone)]
pub struct NewClipItem {
    pub kind: ClipKind,
    /// kind=Text 时必有
    pub text: Option<String>,
    /// kind=Image 时必有（PNG 字节）
    pub image_png: Option<Vec<u8>>,
    /// 图片像素尺寸
    pub img_size: Option<(u32, u32)>,
    /// SHA-256 前 16 字节 hex（32 字符）
    pub hash: String,
    /// unix epoch 毫秒
    pub created_at: i64,
}

/// 列表查询视图（store → UI）。不含图片 BLOB，图片字节按需 `ClipStore::image_bytes`。
#[derive(Debug, Clone)]
pub struct SearchableItem {
    pub id: i64,
    pub kind: ClipKind,
    /// 见 [`text_preview`]
    pub text_preview: String,
    /// Text=全文；Image=""
    pub full_text: String,
    pub img_size: Option<(u32, u32)>,
    pub created_at: i64,
    pub pinned: bool,
}

/// text_preview 长度上限（按 char 计）。
const PREVIEW_MAX_CHARS: usize = 200;

/// 由 `full_text` 生成 `text_preview`：
/// 取前 200 个 char，其中 `'\n'`/`'\r'` 替换为 `' '`；
/// 超过 200 char 时截断后追加 `"…"`（追加不计入 200）。
pub fn text_preview(full_text: &str) -> String {
    let mut preview = String::new();
    let mut count = 0usize;
    let mut truncated = false;
    for ch in full_text.chars() {
        if count == PREVIEW_MAX_CHARS {
            truncated = true;
            break;
        }
        if ch == '\n' || ch == '\r' {
            preview.push(' ');
        } else {
            preview.push(ch);
        }
        count += 1;
    }
    if truncated {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_as_str_roundtrip() {
        assert_eq!(ClipKind::Text.as_str(), "text");
        assert_eq!(ClipKind::Image.as_str(), "image");
        assert_eq!(ClipKind::parse("text"), Some(ClipKind::Text));
        assert_eq!(ClipKind::parse("image"), Some(ClipKind::Image));
        assert_eq!(ClipKind::parse("unknown"), None);
        assert_eq!(ClipKind::parse(""), None);
    }

    #[test]
    fn preview_short_text_unchanged() {
        assert_eq!(text_preview("hello world"), "hello world");
        assert_eq!(text_preview(""), "");
    }

    #[test]
    fn preview_replaces_newlines_with_spaces() {
        assert_eq!(text_preview("a\nb\rc\r\nd"), "a b c  d");
    }

    #[test]
    fn preview_exactly_200_chars_no_ellipsis() {
        let s = "a".repeat(200);
        let p = text_preview(&s);
        assert_eq!(p.chars().count(), 200);
        assert_eq!(p, s);
        assert!(!p.ends_with('…'));
    }

    #[test]
    fn preview_over_200_chars_truncates_with_ellipsis() {
        let s = "a".repeat(201);
        let p = text_preview(&s);
        assert_eq!(p.chars().count(), 201); // 200 + 省略号
        assert_eq!(p, format!("{}…", "a".repeat(200)));
    }

    #[test]
    fn preview_counts_chars_not_bytes() {
        // 201 个多字节字符：截断按 char 计而非字节
        let s = "中".repeat(201);
        let p = text_preview(&s);
        assert_eq!(p.chars().count(), 201);
        assert_eq!(p, format!("{}…", "中".repeat(200)));
    }

    #[test]
    fn preview_replaces_newline_before_truncation() {
        let mut s = String::new();
        for i in 0..201 {
            if i % 2 == 0 {
                s.push('\n');
            } else {
                s.push('x');
            }
        }
        let p = text_preview(&s);
        assert!(!p.contains('\n'));
        assert!(!p.contains('\r'));
        // 前 200 个 char：101 个空格 + 99 个 x，再追加省略号
        assert_eq!(p.chars().count(), 201);
        assert!(p.starts_with(' '));
    }
}
