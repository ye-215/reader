use regex::Regex;
use std::collections::HashMap;
use std::sync::Mutex;

/// 书源规则里的正则 pattern 是有限集合（书源数量 × 规则数），但正文/字段解析
/// 会在逐字段、逐元素、逐章节的最内层循环里反复编译同一个 pattern。这里按
/// pattern 字符串全局缓存编译结果，`None` 表示编译失败（同样缓存，避免非法
/// 正则反复尝试）。容量上限防止异常书源不断喂新 pattern 导致无界增长。
const REGEX_CACHE_CAP: usize = 512;

fn regex_cache() -> &'static Mutex<HashMap<String, Option<Regex>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Option<Regex>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取一个已编译的正则；`None` 表示 pattern 非法。
/// 缓存命中只加一次短锁并 clone `Arc` 级别的引用计数——`Regex` 内部本身是
/// `Arc`，clone 很廉价。
pub fn cached_regex(pattern: &str) -> Option<Regex> {
    {
        let cache = regex_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = cache.get(pattern) {
            return hit.clone();
        }
    }
    // 编译放在锁外：慢操作不阻塞其它 pattern 的查询。
    let compiled = Regex::new(pattern).ok();
    let mut cache = regex_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= REGEX_CACHE_CAP && !cache.contains_key(pattern) {
        cache.clear();
    }
    cache.insert(pattern.to_string(), compiled.clone());
    compiled
}

pub fn strip_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn apply_regex_replace(input: &str, pattern: &str, replace: &str) -> String {
    if let Some(re) = cached_regex(pattern) {
        re.replace_all(input, replace).to_string()
    } else {
        input.to_string()
    }
}

pub fn normalize_source_url(input: &str) -> String {
    input
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// 统计正文的"读者可见字数"。
///
/// 正文可能带 HTML（书源正文规则常返回 `<p>…</p>` 或含段评 img），直接取字符串长度
/// 会把标签字符也算进去 —— 例如 `<p>你好</p>` 是 10 个字符，读者实际只看到 2 个字。
/// 这里先去掉标签、把常见实体折回字符、再忽略所有空白，得到与阅读观感一致的字数。
pub fn visible_char_count(content: &str) -> usize {
    // 先去掉 <script>/<style> 整段: 它们的内容不是正文, 但同样会被标签正则漏掉。
    // regex crate 不支持反向引用, 所以两种标签各写一条, 不做开闭标签配对检查。
    let without_scripts = SCRIPT_RE.replace_all(content, " ");
    let without_styles = STYLE_RE.replace_all(&without_scripts, " ");
    // 段评等标记是 `<img ...>` 这类自闭合标签, 去掉标签即不再计入。
    let without_tags = TAG_RE.replace_all(&without_styles, " ");
    // 书源正文里的 `&nbsp;`/`&amp;` 是转义文字而非可见字符, 不折回会虚增字数。
    let decoded = ENTITY_RE.replace_all(&without_tags, |captures: &regex::Captures| {
        decode_entity(&captures[0])
    });
    decoded.chars().filter(|ch| !ch.is_whitespace()).count()
}

static SCRIPT_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>").expect("script 正则固定可用")
});

static STYLE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?is)<style\b[^>]*>.*?</style\s*>").expect("style 正则固定可用")
});

static TAG_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("标签正则固定可用"));

/// 常见 HTML 实体: 命中后折成对应字符再计数(`&nbsp;` 折成空格, 随后被空白过滤掉)。
static ENTITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"&(#\d+|#[xX][0-9a-fA-F]+|[a-zA-Z]+);").expect("实体正则固定可用")
});

fn decode_entity(entity: &str) -> String {
    let body = entity.trim_start_matches('&').trim_end_matches(';');
    let decoded = match body {
        "nbsp" | "ensp" | "emsp" | "thinsp" => Some(' '),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "hellip" => Some('…'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        _ => None,
    };
    if let Some(ch) = decoded {
        return ch.to_string();
    }
    // 数字实体(`&#8212;` / `&#x2014;`)按码位还原; 认不出就原样保留, 避免误吞内容。
    let codepoint = if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(decimal) = body.strip_prefix('#') {
        decimal.parse::<u32>().ok()
    } else {
        None
    };
    match codepoint.and_then(char::from_u32) {
        Some(ch) => ch.to_string(),
        None => entity.to_string(),
    }
}

pub fn repair_encoded_url(input: &str) -> String {
    let normalized = normalize_source_url(input);
    if !(normalized.contains("%3F")
        || normalized.contains("%3f")
        || normalized.contains("%26")
        || normalized.contains("%3D")
        || normalized.contains("%3d")
        // 守卫原先漏了 %23：只有 URL 里同时出现 %3F/%26/%3D 时，
        // 结尾那次 %23 → # 才会执行；单独的 `%23` 永远修不好。
        || normalized.contains("%23"))
    {
        return normalized;
    }

    normalized
        .replace("%3F", "?")
        .replace("%3f", "?")
        .replace("%26", "&")
        .replace("%3D", "=")
        .replace("%3d", "=")
        .replace("%23", "#")
}

#[cfg(test)]
mod tests {
    use super::{cached_regex, repair_encoded_url, visible_char_count};

    #[test]
    fn cached_regex_hits_and_caches_invalid_patterns() {
        let a = cached_regex("[A-Z]+").expect("合法正则");
        let b = cached_regex("[A-Z]+").expect("合法正则应命中缓存");
        assert_eq!(a.replace_all("ab CD", ""), b.replace_all("ab CD", ""));
        // 非法 pattern 也要返回 None，且第二次同样（走负结果缓存）
        assert!(cached_regex("([").is_none());
        assert!(cached_regex("([").is_none());
        assert_eq!(a.find("xxAByy").unwrap().as_str(), "AB");
    }

    #[test]
    fn repairs_hash_only_url() {
        // 单独出现 %23 时也必须还原：守卫曾漏掉这一项，导致锚点参数（如
        // /chapter/1%23footnote）一直是字面 %23，站点锚点定位与分流失效。
        assert_eq!(
            repair_encoded_url("https://example.test/book/1%23footnote"),
            "https://example.test/book/1#footnote"
        );
        // 混合编码仍要全部还原，且每个只替换一次。
        assert_eq!(
            repair_encoded_url("https://example.test/s%3Fx%3D1%26y%3D2%23top"),
            "https://example.test/s?x=1&y=2#top"
        );
        // 没有编码时不改动。
        assert_eq!(
            repair_encoded_url("https://example.test/book/1"),
            "https://example.test/book/1"
        );
    }

    #[test]
    fn counts_plain_text_without_whitespace() {
        // 换行与缩进是排版, 不是读者数出来的字。
        assert_eq!(visible_char_count("你好\n　　世界"), 4);
    }

    #[test]
    fn ignores_html_tags() {
        // 标签字符不能计入, 否则 `<p>你好</p>` 会数成 10。
        assert_eq!(visible_char_count("<p>你好</p>"), 2);
        assert_eq!(visible_char_count("<div class=\"con\"><p>一二三</p><p>四五六</p></div>"), 6);
    }

    #[test]
    fn ignores_comment_markers_and_scripts() {
        // 段评图标与内联脚本都不是正文。
        let content = "<p>正文</p><img src=\"x.svg\" click=\"showCmt()\"><script>var a=1;</script>";
        assert_eq!(visible_char_count(content), 2);
    }

    #[test]
    fn decodes_entities_before_counting() {
        // `&nbsp;` 折成空格后被空白过滤; `&amp;` 只算 1 个字。
        assert_eq!(visible_char_count("a&nbsp;b"), 2);
        assert_eq!(visible_char_count("A&amp;B"), 3);
        // 数字实体按码位还原。
        assert_eq!(visible_char_count("&#8212;"), 1);
        assert_eq!(visible_char_count("&#x4E00;"), 1);
    }

    #[test]
    fn keeps_unknown_entities_verbatim() {
        // 认不出的实体原样保留, 不能静默吞掉正文。
        assert!(visible_char_count("&unknownentity;") > 0);
    }
}
