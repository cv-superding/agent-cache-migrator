//! 极简 glob 匹配 —— 只服务于 `agents.toml` 的 `skip` 字段。
//!
//! 语义（与字段文档一致）：`skip` 里的模式匹配**相对源根目录的路径**，
//! 命中的条目不参与「校验是否收敛」的判定（复制仍会带上 —— 多搬无害，
//! 缺的只是「这些文件不必逐字节一致」的约束）。
//!
//! 支持的语法（刻意保持最小）：
//! - `*`   —— 段内任意字符（不跨 `/`）
//! - `**`  —— 单独成段时，匹配零个或多个目录层
//! - 不支持 `?` / 字符类 / 中段 `**` —— 能力越小越好预测
//!
//! 示例：
//! - `logs/**`        → `logs/` 下的一切
//! - `**/*.log`       → 任意层级的 `.log` 文件
//! - `app/CodeCache/**` → 固定前缀目录下的一切

use std::path::Path;

/// 相对路径 `rel` 是否命中 `patterns` 里的任意一条。
pub fn matches_any(rel: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    let segs: Vec<&str> = rel_s.split('/').filter(|s| !s.is_empty()).collect();
    patterns.iter().any(|p| match_segs(&segs_of(p), &segs))
}

/// 模式切段。归一化分隔符、去掉空段。
///
/// 🔴 返回 `Vec<String>`（拥有所有权）：`segs_of` 内部的 `replace` 产生的
/// String 是临时值，若让切片借它出去会直接 E0515。
fn segs_of(pattern: &str) -> Vec<String> {
    pattern
        .trim()
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 段级递归匹配。`**` 只在单独成段时有跨段含义。
fn match_segs(pat: &[String], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((p, rest)) if p == "**" => {
            // `**` 匹配零层：剩余模式要匹配当前剩余路径
            if match_segs(rest, path) {
                return true;
            }
            // `**` 再吃掉一层
            match path.split_first() {
                Some((_, tail)) => match_segs(pat, tail),
                None => false,
            }
        }
        Some((p, rest)) => match path.split_first() {
            Some((s, tail)) => seg_match(p, s) && match_segs(rest, tail),
            None => false,
        },
    }
}

/// 单段匹配：支持 `*`（段内通配），大小写不敏感。
/// 策略与 `procs::matches` 一致：首段必须前缀、末段必须后缀、中间按序出现。
fn seg_match(p: &str, s: &str) -> bool {
    let p = p.to_ascii_lowercase();
    let s = s.to_ascii_lowercase();
    if !p.contains('*') {
        return p == s;
    }
    let parts: Vec<&str> = p.split('*').collect();
    let mut rest = s.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(idx) => {
                if i == 0 && idx != 0 {
                    return false;
                }
                rest = &rest[idx + part.len()..];
            }
            None => return false,
        }
    }
    if !p.ends_with('*') {
        if let Some(last) = parts.last() {
            return s.ends_with(last);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(pattern: &str, rel: &str) -> bool {
        matches_any(Path::new(rel), &[pattern.to_string()])
    }

    #[test]
    fn double_star_dir_prefix() {
        assert!(hit("logs/**", "logs/latest/app.log"));
        assert!(hit("logs/**", "logs/x.log"));
        assert!(!hit("logs/**", "other/x.log"));
    }

    #[test]
    fn double_star_file_suffix() {
        assert!(hit("**/*.log", "a/b/c.log"));
        assert!(hit("**/*.log", "root.log"));
        assert!(!hit("**/*.log", "a/b/c.txt"));
        assert!(!hit("**/*.log", "a/b.log.txt"), "后缀必须贴着段尾");
    }

    #[test]
    fn fixed_prefix_with_star_segment() {
        assert!(hit("app/CodeCache/**", "app/CodeCache/js/heap.bin"));
        assert!(!hit("app/CodeCache/**", "app/Other/js/heap.bin"));
        // 段内 `*`
        assert!(hit("Cache/*/data", "Cache/GPUCache/data"));
        assert!(!hit("Cache/*/data", "Cache/GPUCache/deep/data"));
    }

    #[test]
    fn case_and_separator_insensitive() {
        assert!(hit("logs/**", "LOGS/a.txt"));
        assert!(hit(r"logs\**", "logs/a.txt"), "反斜杠写法也要认");
    }

    #[test]
    fn empty_patterns_never_match() {
        assert!(!matches_any(Path::new("anything"), &[]));
    }

    #[test]
    fn real_registry_skip_patterns() {
        let skip = vec![
            "logs/**".to_string(),
            "**/*.log".to_string(),
            "app/CodeCache/**".to_string(),
            "app/session/**".to_string(),
        ];
        for p in ["logs/run.log", "app/session/cache.bin", "x/y/trace.log"] {
            assert!(matches_any(Path::new(p), &skip), "{p} 应被跳过");
        }
        for p in ["app/config.json", "data/store.db", "app/logs2/a.bin"] {
            assert!(!matches_any(Path::new(p), &skip), "{p} 不该被跳过");
        }
    }
}
