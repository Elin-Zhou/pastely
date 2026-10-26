//! 模糊搜索：对 [`SearchableItem`] 列表按查询串过滤、打分、排序并给出高亮索引。

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use crate::model::{ClipKind, SearchableItem, SnippetItem};

/// 过滤 + 排序。
///
/// - `query.trim()` 为空 → 按 `items` 原序全量返回（高亮为空，含 Image 条目）
/// - 非空 → 仅 `kind=Text` 条目参与 `SkimMatcherV2` 匹配 `full_text`，按分数降序；
///   高亮索引 = 匹配索引中落在 `text_preview` char 长度内的子集
///   （preview 为 full_text 前缀截断，索引可直接沿用）
/// - 无匹配（分数 `None`）的条目丢弃
pub fn fuzzy_filter(items: &[SearchableItem], query: &str) -> Vec<(SearchableItem, Vec<usize>)> {
    let query = query.trim();
    if query.is_empty() {
        return items
            .iter()
            .cloned()
            .map(|item| (item, Vec::new()))
            .collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<(i64, SearchableItem, Vec<usize>)> = items
        .iter()
        .filter(|item| item.kind == ClipKind::Text)
        .filter_map(|item| {
            let (score, indices) = matcher.fuzzy_indices(&item.full_text, query)?;
            let preview_chars = item.text_preview.chars().count();
            let highlights = indices
                .into_iter()
                .filter(|idx| *idx < preview_chars)
                .collect();
            Some((score, item.clone(), highlights))
        })
        .collect();
    scored.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .map(|(_, item, highlights)| (item, highlights))
        .collect()
}

/// 关键词模糊匹配（忽略大小写）。空查询返回空（片段不进空查询首屏）。
///
/// 匹配目标为 `keyword` 字段；返回项按分数降序，高亮索引相对 keyword 的 char 偏移。
pub fn snippet_filter(snippets: &[SnippetItem], query: &str) -> Vec<(SnippetItem, Vec<usize>)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    // 需求要求片段关键词忽略大小写（历史搜索的 Smart 行为保持不变）；
    // 0.3.7 无独立 builder，matcher 自身链式 `ignore_case()` 即 CaseMatching::Ignore
    let matcher = SkimMatcherV2::default().ignore_case();
    let mut scored: Vec<(i64, SnippetItem, Vec<usize>)> = snippets
        .iter()
        .filter_map(|snippet| {
            let (score, indices) = matcher.fuzzy_indices(&snippet.keyword, query)?;
            Some((score, snippet.clone(), indices))
        })
        .collect();
    scored.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .map(|(_, snippet, highlights)| (snippet, highlights))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_item(id: i64, full_text: &str) -> SearchableItem {
        SearchableItem {
            id,
            kind: ClipKind::Text,
            text_preview: crate::model::make_preview(full_text),
            full_text: full_text.to_string(),
            img_size: None,
            created_at: id,
            pinned: false,
        }
    }

    fn image_item(id: i64) -> SearchableItem {
        SearchableItem {
            id,
            kind: ClipKind::Image,
            text_preview: String::new(),
            full_text: String::new(),
            img_size: Some((640, 480)),
            created_at: id,
            pinned: false,
        }
    }

    fn ids(result: &[(SearchableItem, Vec<usize>)]) -> Vec<i64> {
        result.iter().map(|(item, _)| item.id).collect()
    }

    #[test]
    fn empty_query_returns_all_in_original_order_with_no_highlight() {
        let items = vec![text_item(1, "alpha"), image_item(2), text_item(3, "beta")];
        let result = fuzzy_filter(&items, "");
        assert_eq!(ids(&result), vec![1, 2, 3]);
        assert!(result.iter().all(|(_, hl)| hl.is_empty()));
    }

    #[test]
    fn whitespace_query_treated_as_empty() {
        let items = vec![text_item(1, "alpha"), image_item(2)];
        let result = fuzzy_filter(&items, "   \t ");
        assert_eq!(ids(&result), vec![1, 2]);
    }

    #[test]
    fn matches_sort_by_score_descending() {
        // "hello" 连续命中得分明显高于散落命中
        let items = vec![
            text_item(1, "hxxexxlxxlxxo scattered"),
            text_item(2, "hello contiguous"),
        ];
        let result = fuzzy_filter(&items, "hello");
        assert_eq!(ids(&result), vec![2, 1]);
    }

    #[test]
    fn no_match_items_are_dropped() {
        let items = vec![text_item(1, "alpha"), text_item(2, "beta")];
        assert!(fuzzy_filter(&items, "zzz").is_empty());
    }

    #[test]
    fn image_items_filtered_out_on_non_empty_query() {
        let items = vec![image_item(1), text_item(2, "alpha")];
        let result = fuzzy_filter(&items, "a");
        assert_eq!(ids(&result), vec![2]);
    }

    #[test]
    fn highlight_char_indices_are_correct() {
        // 连续尾部命中："hello world" 中 "world" → char 偏移 6..=10
        let result = fuzzy_filter(&[text_item(1, "hello world")], "world");
        assert_eq!(result[0].1, vec![6, 7, 8, 9, 10]);

        // 散落命中："hw" → 0 与 6
        let result = fuzzy_filter(&[text_item(1, "hello world")], "hw");
        assert_eq!(result[0].1, vec![0, 6]);
    }

    #[test]
    fn highlight_indices_beyond_preview_are_dropped() {
        // full_text = 'a' + 200*'x' + 'b'（202 char）；preview = 前 200 char + '…'（201 char）
        let full_text = format!("a{}b", "x".repeat(200));
        assert_eq!(full_text.chars().count(), 202);
        let item = text_item(1, &full_text);
        assert_eq!(item.text_preview.chars().count(), 201);

        let result = fuzzy_filter(std::slice::from_ref(&item), "ab");
        assert_eq!(result.len(), 1);
        // 'b' 的匹配索引 201 落在 preview 外（≥201）被丢弃，仅保留 0
        assert_eq!(result[0].1, vec![0]);
    }

    #[test]
    fn indices_count_chars_not_bytes() {
        // 多字节文本：char 偏移而非字节偏移
        let result = fuzzy_filter(&[text_item(1, "世界你好")], "世你");
        assert_eq!(result[0].1, vec![0, 2]);
    }

    fn snippet(id: i64, keyword: &str) -> SnippetItem {
        SnippetItem {
            id,
            name: id.to_string(),
            keyword: keyword.to_string(),
            content: String::new(),
            updated_at: id,
        }
    }

    fn snippet_ids(result: &[(SnippetItem, Vec<usize>)]) -> Vec<i64> {
        result.iter().map(|(s, _)| s.id).collect()
    }

    #[test]
    fn snippet_empty_query_returns_empty() {
        let snippets = vec![snippet(1, "sig"), snippet(2, "addr")];
        assert!(snippet_filter(&snippets, "").is_empty());
        assert!(snippet_filter(&snippets, "   \t ").is_empty());
    }

    #[test]
    fn snippet_case_is_ignored() {
        // 小写查询命中大写关键词
        let result = snippet_filter(&[snippet(1, "SIG")], "sig");
        assert_eq!(snippet_ids(&result), vec![1]);
        // 大写查询命中小写关键词
        let result = snippet_filter(&[snippet(1, "addr")], "ADDR");
        assert_eq!(snippet_ids(&result), vec![1]);
    }

    #[test]
    fn snippet_unmatched_are_dropped() {
        let snippets = vec![snippet(1, "sig"), snippet(2, "addr")];
        assert!(snippet_filter(&snippets, "zzz").is_empty());
        let result = snippet_filter(&snippets, "ad");
        assert_eq!(snippet_ids(&result), vec![2]);
    }

    #[test]
    fn snippet_matches_sort_by_score_descending() {
        // "sign" 连续命中得分明显高于散落命中
        let snippets = vec![snippet(1, "sxxixxgxxn scattered"), snippet(2, "sign")];
        let result = snippet_filter(&snippets, "sign");
        assert_eq!(snippet_ids(&result), vec![2, 1]);
    }

    #[test]
    fn snippet_highlight_indices_are_correct() {
        // 前缀命中："signature" 中 "sig" → char 偏移 0..=2
        let result = snippet_filter(&[snippet(1, "signature")], "sig");
        assert_eq!(result[0].1, vec![0, 1, 2]);
    }

    #[test]
    fn snippet_highlight_indices_count_chars_for_cjk() {
        // 中文关键词："我的签名" 中 "签名" → char 偏移 2、3（非字节 6、9）
        let result = snippet_filter(&[snippet(1, "我的签名")], "签名");
        assert_eq!(result[0].1, vec![2, 3]);
    }
}
