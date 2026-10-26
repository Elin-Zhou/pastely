//! 片段内容变量解析与展开（纯函数）。
//!
//! ## 变量语法
//!
//! | 源文本 | 展开结果 |
//! |--------|---------|
//! | `{{clipboard}}` | 剪切板文本（无值时填空串） |
//! | `{{date:YYYY-MM-DD}}` | 本地日期，如 `2026-10-26` |
//! | `{{date:YYYY-MM-DD HH:mm:ss}}` | 本地日期时间，如 `2026-10-26 21:30:05` |
//! | `{{date:ts_s}}` | unix 秒十进制 |
//! | `{{date:ts_ms}}` | unix 毫秒十进制 |
//! | `{{cursor}}` | 从输出移除并记录光标 char 偏移 |
//!
//! ## 解析规则
//!
//! - 扫描 `{{`，找下一个 `}}` 取内容：命中变量则替换；否则该 `{{`
//!   按普通文本处理并从其后一个字符继续扫描（避免死循环）
//! - `{{` 与 `}}` 之间再出现 `{` 或 `}` → 视为普通文本
//! - 未闭合的 `{{` → 普通文本
//! - 多个 `{{cursor}}`：第一个定偏移，其余同样移除；偏移按 char 计（非字节）
//! - 变量替换结果（如剪切板内容）不参与二次展开

/// 变量名与日期格式常量（规约：禁止魔法字符串；设置页「插入变量」菜单亦复用）。
pub mod vars {
    /// 剪切板内容变量。
    pub const CLIPBOARD: &str = "clipboard";
    /// 光标占位变量。
    pub const CURSOR: &str = "cursor";
    /// 日期变量前缀（`{{date:<格式>}}`）。
    pub const DATE_PREFIX: &str = "date:";
    /// 本地日期格式。
    pub const DATE_FMT_YMD: &str = "YYYY-MM-DD";
    /// 本地日期时间格式。
    pub const DATE_FMT_YMD_HMS: &str = "YYYY-MM-DD HH:mm:ss";
    /// unix 秒格式。
    pub const DATE_FMT_TS_S: &str = "ts_s";
    /// unix 毫秒格式。
    pub const DATE_FMT_TS_MS: &str = "ts_ms";
}

/// 展开结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    /// 展开后文本中光标应落点的 char 偏移；内容无 {{cursor}} 时为 None
    pub cursor_char: Option<usize>,
}

/// 展开 `content` 中的变量；`now_ms` 为当前 unix epoch 毫秒，
/// `clipboard_text` 为剪切板文本（`None` 时 `{{clipboard}}` 填空串）。
pub fn expand(content: &str, now_ms: i64, clipboard_text: Option<&str>) -> Expansion {
    let chars: Vec<char> = content.chars().collect();
    let mut out = String::new();
    let mut cursor_char: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        if !starts_with_open(&chars, i) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let close = match find_close(&chars, i + 2) {
            Some(close) => close,
            // 未闭合的 `{{` → 普通文本，从其后一个字符继续扫描
            None => {
                out.push('{');
                i += 1;
                continue;
            }
        };
        let inner: String = chars[i + 2..close].iter().collect();
        // `{{` 与 `}}` 之间再出现 `{` 或 `}` → 视为普通文本
        if inner.contains('{') || inner.contains('}') {
            out.push('{');
            i += 1;
            continue;
        }
        match resolve(&inner, now_ms, clipboard_text) {
            Some(Replacement::Text(text)) => {
                // 替换结果直接落盘，不参与二次展开
                out.push_str(&text);
                i = close + 2;
            }
            Some(Replacement::Cursor) => {
                // 多个 {{cursor}}：第一个定偏移，其余同样移除
                if cursor_char.is_none() {
                    cursor_char = Some(out.chars().count());
                }
                i = close + 2;
            }
            // 未知变量 → 该 `{{` 按普通文本处理，从其后一个字符继续扫描
            None => {
                out.push('{');
                i += 1;
            }
        }
    }
    Expansion {
        text: out,
        cursor_char,
    }
}

/// 单个变量的替换结果。
enum Replacement {
    /// 替换为文本。
    Text(String),
    /// 移除并记录光标 char 偏移。
    Cursor,
}

/// 变量内容 → 替换结果；未知内容返回 `None`（调用方按普通文本处理）。
fn resolve(inner: &str, now_ms: i64, clipboard_text: Option<&str>) -> Option<Replacement> {
    if inner == vars::CURSOR {
        return Some(Replacement::Cursor);
    }
    if inner == vars::CLIPBOARD {
        return Some(Replacement::Text(
            clipboard_text.unwrap_or_default().to_string(),
        ));
    }
    inner
        .strip_prefix(vars::DATE_PREFIX)
        .and_then(|fmt| resolve_date(fmt, now_ms))
}

/// 日期变量格式 → 展开文本；未知格式返回 `None`。
fn resolve_date(fmt: &str, now_ms: i64) -> Option<Replacement> {
    let epoch_secs = now_ms.div_euclid(1_000);
    let text = match fmt {
        vars::DATE_FMT_YMD => fmt_date(local_parts(epoch_secs)),
        vars::DATE_FMT_YMD_HMS => fmt_datetime(local_parts(epoch_secs)),
        vars::DATE_FMT_TS_S => epoch_secs.to_string(),
        vars::DATE_FMT_TS_MS => now_ms.to_string(),
        _ => return None,
    };
    Some(Replacement::Text(text))
}

/// `chars[i..]` 是否以 `{{` 开头（`i` 必须在界内）。
fn starts_with_open(chars: &[char], i: usize) -> bool {
    chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '{'
}

/// 从 `from` 起第一个 `}}` 的起始 char 下标。
fn find_close(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&j| chars[j] == '}' && chars[j + 1] == '}')
}

/// 本地时间的六个分量：(年, 月, 日, 时, 分, 秒)。
type DateParts = (i64, u32, u32, u32, u32, u32);

/// unix 秒 → 本地时间分量。`libc::localtime_r` 线程安全、无版本门槛。
fn local_parts(epoch_secs: i64) -> DateParts {
    let secs = epoch_secs as libc::time_t;
    // SAFETY: 两个指针均指向合法内存；localtime_r 只写 result 指向的 tm
    // 并返回其指针（POSIX），失败时不产生 UB（字段保持零值）
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (
        i64::from(tm.tm_year) + 1_900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
        tm.tm_sec as u32,
    )
}

/// 分量 → `YYYY-MM-DD`（月/日两位零填充）。
fn fmt_date(p: DateParts) -> String {
    let (year, month, day, _, _, _) = p;
    format!("{year:04}-{month:02}-{day:02}")
}

/// 分量 → `YYYY-MM-DD HH:mm:ss`（各字段两位零填充）。
fn fmt_datetime(p: DateParts) -> String {
    let (year, month, day, hour, minute, second) = p;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定的「当前时间」（unix epoch 毫秒），仅 ts 类断言依赖确切值。
    const NOW_MS: i64 = 1_769_515_805_000;

    #[test]
    fn plain_text_without_variables_is_unchanged() {
        let e = expand("hello world", NOW_MS, None);
        assert_eq!(e.text, "hello world");
        assert_eq!(e.cursor_char, None);
        let e = expand("", NOW_MS, None);
        assert_eq!(e.text, "");
        assert_eq!(e.cursor_char, None);
    }

    #[test]
    fn clipboard_none_empty_and_some() {
        // None 填空串
        assert_eq!(expand("x{{clipboard}}y", NOW_MS, None).text, "xy");
        // 有值但为空串：同样是空串，但走的是有值路径
        assert_eq!(expand("x{{clipboard}}y", NOW_MS, Some("")).text, "xy");
        // 有值
        assert_eq!(
            expand("x{{clipboard}}y", NOW_MS, Some("剪贴板")).text,
            "x剪贴板y"
        );
        assert_eq!(expand("{{clipboard}}", NOW_MS, Some("hi")).text, "hi");
    }

    #[test]
    fn clipboard_content_is_not_reexpanded() {
        // 替换结果不参与二次展开：剪切板里的 {{cursor}} 原样保留
        let e = expand("{{clipboard}}", NOW_MS, Some("{{cursor}}"));
        assert_eq!(e.text, "{{cursor}}");
        assert_eq!(e.cursor_char, None);
    }

    #[test]
    fn date_variables_match_local_parts_formatting() {
        // 布线验证：expand 的日期路径与 local_parts + fmt_* 同源
        let secs = NOW_MS.div_euclid(1_000);
        assert_eq!(
            expand("{{date:YYYY-MM-DD}}", NOW_MS, None).text,
            fmt_date(local_parts(secs))
        );
        assert_eq!(
            expand("{{date:YYYY-MM-DD HH:mm:ss}}", NOW_MS, None).text,
            fmt_datetime(local_parts(secs))
        );
    }

    #[test]
    fn fmt_date_pads_month_and_day() {
        assert_eq!(fmt_date((2026, 10, 26, 0, 0, 0)), "2026-10-26");
        assert_eq!(fmt_date((2026, 1, 5, 0, 0, 0)), "2026-01-05");
    }

    #[test]
    fn fmt_datetime_pads_all_components() {
        assert_eq!(
            fmt_datetime((2026, 10, 26, 21, 30, 5)),
            "2026-10-26 21:30:05"
        );
        assert_eq!(fmt_datetime((2026, 1, 2, 3, 4, 5)), "2026-01-02 03:04:05");
    }

    #[test]
    fn ts_variables_expand_to_decimal_strings() {
        assert_eq!(expand("{{date:ts_s}}", NOW_MS, None).text, "1769515805");
        assert_eq!(expand("{{date:ts_ms}}", NOW_MS, None).text, "1769515805000");
    }

    #[test]
    fn cursor_at_start_middle_and_end() {
        let e = expand("{{cursor}}end", NOW_MS, None);
        assert_eq!(e.text, "end");
        assert_eq!(e.cursor_char, Some(0));

        let e = expand("ab{{cursor}}cd", NOW_MS, None);
        assert_eq!(e.text, "abcd");
        assert_eq!(e.cursor_char, Some(2));

        let e = expand("ab{{cursor}}", NOW_MS, None);
        assert_eq!(e.text, "ab");
        assert_eq!(e.cursor_char, Some(2));
    }

    #[test]
    fn first_cursor_wins_and_rest_removed() {
        let e = expand("a{{cursor}}b{{cursor}}c", NOW_MS, None);
        assert_eq!(e.text, "abc");
        assert_eq!(e.cursor_char, Some(1));
    }

    #[test]
    fn cursor_offset_counts_chars_not_bytes() {
        // 中文：字节偏移为 6，char 偏移必须为 2
        let e = expand("你好{{cursor}}🎉", NOW_MS, None);
        assert_eq!(e.text, "你好🎉");
        assert_eq!(e.cursor_char, Some(2));
        // emoji 前置：字节偏移为 8，char 偏移必须为 2
        let e = expand("🎉🎉{{cursor}}", NOW_MS, None);
        assert_eq!(e.text, "🎉🎉");
        assert_eq!(e.cursor_char, Some(2));
    }

    #[test]
    fn unknown_variable_kept_verbatim() {
        let e = expand("a{{foo}}b", NOW_MS, None);
        assert_eq!(e.text, "a{{foo}}b");
        assert_eq!(e.cursor_char, None);
        // 未知日期格式同样原样保留
        assert_eq!(expand("{{date:YYYY}}", NOW_MS, None).text, "{{date:YYYY}}");
        // 未知变量之后的合法变量仍能命中（从 {{ 后一个字符继续扫描）
        let e = expand("{{foo}}{{cursor}}x", NOW_MS, None);
        assert_eq!(e.text, "{{foo}}x");
        assert_eq!(e.cursor_char, Some(7));
    }

    #[test]
    fn unclosed_open_kept_verbatim() {
        assert_eq!(expand("pre{{abc", NOW_MS, None).text, "pre{{abc");
        assert_eq!(expand("{{", NOW_MS, None).text, "{{");
        assert_eq!(expand("{{}", NOW_MS, None).text, "{{}");
    }

    #[test]
    fn nested_braces_kept_verbatim() {
        // 内容含 { → 普通文本
        assert_eq!(expand("{{a{b}}}", NOW_MS, None).text, "{{a{b}}}");
        // 内容含 } → 普通文本
        assert_eq!(expand("{{a}b}}", NOW_MS, None).text, "{{a}b}}");
    }

    #[test]
    fn adjacent_variables_expand_in_place() {
        let e = expand("{{date:ts_s}}-{{date:ts_s}}", NOW_MS, None);
        assert_eq!(e.text, "1769515805-1769515805");
        let e = expand("{{clipboard}}{{cursor}}{{clipboard}}", NOW_MS, Some("v"));
        assert_eq!(e.text, "vv");
        assert_eq!(e.cursor_char, Some(1));
    }

    #[test]
    fn local_parts_produces_structurally_valid_components() {
        // 不依赖运行时 TZ 的不变量：分量取值范围与格式化长度
        let p = local_parts(1_767_225_600);
        assert_eq!(fmt_date(p).chars().count(), 10);
        assert_eq!(fmt_datetime(p).chars().count(), 19);
        let (_, month, day, hour, minute, second) = p;
        assert!((1..=12).contains(&month));
        assert!((1..=31).contains(&day));
        assert!(hour <= 23);
        assert!(minute <= 59);
        assert!(second <= 60);
    }
}
