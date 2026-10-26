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
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            v if v == Self::Text.as_str() => Some(Self::Text),
            v if v == Self::Image.as_str() => Some(Self::Image),
            _ => None,
        }
    }
}

/// 写入载荷（clipboard → store）。`hash` 为内容 SHA-256 前 16 字节 hex（32 字符）。
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// 见 [`make_preview`]
    pub text_preview: String,
    /// Text=全文；Image=""
    pub full_text: String,
    pub img_size: Option<(u32, u32)>,
    pub created_at: i64,
    pub pinned: bool,
}

/// 快捷片段（设置页 CRUD + 主面板快速输入）。
#[derive(Debug, Clone)]
pub struct SnippetItem {
    pub id: i64,
    pub name: String,
    pub keyword: String,
    pub content: String,
    /// unix epoch 毫秒
    pub updated_at: i64,
}

/// 主面板统一行模型：片段与剪贴板条目共用一个列表。
#[derive(Debug, Clone)]
pub enum ListItem {
    Clip(SearchableItem),
    Snippet(SnippetItem),
}

/// 当前时间的 unix epoch 毫秒。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// 计算去重哈希：SHA-256 摘要前 16 字节的 hex（32 个小写字符）。
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    hex(&digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX_DIGITS[usize::from(b >> 4)] as char);
        out.push(HEX_DIGITS[usize::from(b & 0x0f)] as char);
    }
    out
}

/// text_preview 长度上限（按 char 计）。
pub const PREVIEW_CHARS: usize = 200;

/// 由 `full_text` 生成 `text_preview`：
/// 取前 [`PREVIEW_CHARS`] 个 char，其中 `'\n'`/`'\r'` 替换为 `' '`；
/// 超过上限时截断后追加 `"…"`（追加不计入上限）。
pub fn make_preview(full_text: &str) -> String {
    let mut preview = String::new();
    let mut truncated = false;
    for (i, ch) in full_text.chars().enumerate() {
        if i == PREVIEW_CHARS {
            truncated = true;
            break;
        }
        preview.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
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
        assert_eq!(ClipKind::from_db("text"), Some(ClipKind::Text));
        assert_eq!(ClipKind::from_db("image"), Some(ClipKind::Image));
        assert_eq!(ClipKind::from_db("unknown"), None);
        assert_eq!(ClipKind::from_db(""), None);
    }

    #[test]
    fn content_hash_is_stable_32_lowercase_hex() {
        let h1 = content_hash(b"hello");
        assert_eq!(h1, content_hash(b"hello"));
        assert_ne!(h1, content_hash(b"world"));
        assert_eq!(h1.len(), 32);
        assert!(
            h1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {h1}"
        );
    }

    #[test]
    fn now_ms_is_plausible_epoch_millis() {
        // 2020-01-01 之后即可捕获时钟倒退类错误
        assert!(now_ms() > 1_577_836_800_000);
    }

    #[test]
    fn preview_short_text_unchanged() {
        assert_eq!(make_preview("hello world"), "hello world");
        assert_eq!(make_preview(""), "");
    }

    #[test]
    fn preview_replaces_newlines_with_spaces() {
        assert_eq!(make_preview("a\nb\rc\r\nd"), "a b c  d");
    }

    #[test]
    fn preview_exactly_200_chars_no_ellipsis() {
        let s = "a".repeat(200);
        let p = make_preview(&s);
        assert_eq!(p.chars().count(), 200);
        assert_eq!(p, s);
        assert!(!p.ends_with('…'));
    }

    #[test]
    fn preview_over_200_chars_truncates_with_ellipsis() {
        let s = "a".repeat(201);
        let p = make_preview(&s);
        assert_eq!(p.chars().count(), 201); // 200 + 省略号
        assert_eq!(p, format!("{}…", "a".repeat(200)));
    }

    #[test]
    fn preview_counts_chars_not_bytes() {
        // 201 个多字节字符：截断按 char 计而非字节
        let s = "中".repeat(201);
        let p = make_preview(&s);
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
        let p = make_preview(&s);
        assert!(!p.contains('\n'));
        assert!(!p.contains('\r'));
        // 前 200 个 char：101 个空格 + 99 个 x，再追加省略号
        assert_eq!(p.chars().count(), 201);
        assert!(p.starts_with(' '));
    }
}
