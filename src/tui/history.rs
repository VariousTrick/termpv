// 播放历史记录与断点续播管理模块
use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单条历史记录项
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub url_or_path: String,
    pub title: String,
    pub last_pts: f64,
    pub total_secs: f64,
    pub timestamp: u64,
}

impl HistoryItem {
    /// 计算播放百分比 (0.0 ~ 100.0)
    pub fn progress_pct(&self) -> f64 {
        if self.total_secs > 0.0 {
            ((self.last_pts / self.total_secs) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    }

    /// 格式化时间戳为 HH:MM:SS 或 MM:SS
    pub fn format_duration(secs: f64) -> String {
        let s = secs.max(0.0).round() as u64;
        let h = s / 3600;
        let m = (s % 3600) / 60;
        let sec = s % 60;
        if h > 0 {
            format!("{:02}:{:02}:{:02}", h, m, sec)
        } else {
            format!("{:02}:{:02}", m, sec)
        }
    }

    /// 格式化相对时间（如 “5 分钟前”、“昨天”）
    pub fn format_relative_time(&self) -> String {
        if self.timestamp == 0 {
            return "-".into();
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if now >= self.timestamp {
            let diff = now - self.timestamp;
            if diff < 60 {
                "刚刚".into()
            } else if diff < 3600 {
                format!("{} 分钟前", diff / 60)
            } else if diff < 86400 {
                format!("{} 小时前", diff / 3600)
            } else {
                format!("{} 天前", diff / 86400)
            }
        } else {
            "刚刚".into()
        }
    }
}

/// 历史记录持久化管理器
pub struct HistoryManager;

impl HistoryManager {
    /// 获取历史记录保存路径 (~/.config/termpv/history.json)
    pub fn history_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("termpv")
                .join("history.json"),
        )
    }

    /// 加载历史记录列表
    pub fn load() -> Vec<HistoryItem> {
        let Some(path) = Self::history_path() else {
            return Vec::new();
        };

        let Ok(content) = fs::read_to_string(&path) else {
            return Vec::new();
        };

        Self::parse_json(&content)
    }

    /// 保存历史记录列表
    pub fn save(items: &[HistoryItem]) -> Result<()> {
        let Some(path) = Self::history_path() else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let json = Self::serialize_json(items);
        let _ = fs::write(path, json);
        Ok(())
    }

    /// 记录或更新一条播放记录
    pub fn record(url_or_path: &str, last_pts: f64, total_secs: f64) {
        let trimmed = url_or_path.trim();
        if trimmed.is_empty() || total_secs <= 0.0 {
            return;
        }

        let mut items = Self::load();

        // 移除旧的同名记录
        items.retain(|it| it.url_or_path != trimmed);

        let title = Self::derive_title(trimmed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 如果播放到了最后 3 秒以内，断点归零（下次从头看）
        let effective_pts = if last_pts >= (total_secs - 3.0) {
            0.0
        } else {
            last_pts.max(0.0)
        };

        items.insert(
            0,
            HistoryItem {
                url_or_path: trimmed.to_string(),
                title,
                last_pts: effective_pts,
                total_secs,
                timestamp: now,
            },
        );

        // 最多保留 30 条历史
        if items.len() > 30 {
            items.truncate(30);
        }

        let _ = Self::save(&items);
    }

    /// 删除指定索引的历史项
    pub fn remove(index: usize) -> Result<()> {
        let mut items = Self::load();
        if index < items.len() {
            items.remove(index);
            Self::save(&items)?;
        }
        Ok(())
    }

    /// 清空所有历史
    pub fn clear() -> Result<()> {
        Self::save(&[])
    }

    /// 根据路径或 URL 推导易读的显示标题
    pub fn derive_title(url_or_path: &str) -> String {
        if url_or_path.starts_with("http://")
            || url_or_path.starts_with("https://")
            || url_or_path.starts_with("rtsp://")
        {
            // 尝试提取 URL 路径中的文件名
            if let Some(path_part) = url_or_path.split('?').next() {
                if let Some(file_name) = path_part.rsplit('/').next() {
                    if !file_name.is_empty() && file_name.contains('.') {
                        return file_name.to_string();
                    }
                }
            }
            // 否则截取域名与前缀
            let host_part = url_or_path
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("rtsp://");
            let len = host_part.chars().count();
            if len > 45 {
                format!("{}...", host_part.chars().take(42).collect::<String>())
            } else {
                host_part.to_string()
            }
        } else {
            let path = Path::new(url_or_path);
            if let Some(name) = path.file_name() {
                name.to_string_lossy().to_string()
            } else {
                url_or_path.to_string()
            }
        }
    }

    /// 简单的零依赖标准 JSON 序列化器
    fn serialize_json(items: &[HistoryItem]) -> String {
        let mut out = String::from("[\n");
        for (i, item) in items.iter().enumerate() {
            out.push_str("  {\n");
            out.push_str(&format!(
                "    \"url_or_path\": \"{}\",\n",
                escape_json_str(&item.url_or_path)
            ));
            out.push_str(&format!(
                "    \"title\": \"{}\",\n",
                escape_json_str(&item.title)
            ));
            out.push_str(&format!("    \"last_pts\": {:.2},\n", item.last_pts));
            out.push_str(&format!("    \"total_secs\": {:.2},\n", item.total_secs));
            out.push_str(&format!("    \"timestamp\": {}\n", item.timestamp));
            out.push_str("  }");
            if i + 1 < items.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("]\n");
        out
    }

    /// 简单的零依赖标准 JSON 解析器
    fn parse_json(content: &str) -> Vec<HistoryItem> {
        let mut items = Vec::new();
        let mut depth = 0usize;
        let mut block_start = 0usize;

        for (idx, b) in content.bytes().enumerate() {
            match b {
                b'{' => {
                    if depth == 0 {
                        block_start = idx;
                    }
                    depth += 1;
                }
                b'}' => {
                    if depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            let block = &content[block_start..=idx];
                            if let Some(item) = Self::parse_item(block) {
                                items.push(item);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        items
    }

    fn parse_item(block: &str) -> Option<HistoryItem> {
        let url_or_path = extract_string_field(block, "url_or_path")?;
        let title = extract_string_field(block, "title")
            .unwrap_or_else(|| Self::derive_title(&url_or_path));
        let last_pts = extract_num_field::<f64>(block, "last_pts").unwrap_or(0.0);
        let total_secs = extract_num_field::<f64>(block, "total_secs").unwrap_or(0.0);
        let timestamp = extract_num_field::<u64>(block, "timestamp").unwrap_or(0);

        Some(HistoryItem {
            url_or_path,
            title,
            last_pts,
            total_secs,
            timestamp,
        })
    }
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn extract_string_field(block: &str, field_name: &str) -> Option<String> {
    let pattern = format!("\"{}\":", field_name);
    let pos = block.find(&pattern)?;
    let rem = &block[pos + pattern.len()..];
    let trimmed = rem.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let inner = &trimmed[1..];
    let mut end_idx = 0;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end_idx = i;
            break;
        }
    }
    Some(unescape_json_str(&inner[..end_idx]))
}

fn extract_num_field<T: std::str::FromStr>(block: &str, field_name: &str) -> Option<T> {
    let pattern = format!("\"{}\":", field_name);
    let pos = block.find(&pattern)?;
    let rem = &block[pos + pattern.len()..];
    let trimmed = rem.trim_start();
    let end = trimmed
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(trimmed.len());
    trimmed[..end].parse::<T>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_serialization_roundtrip() {
        let items = vec![
            HistoryItem {
                url_or_path: "https://example.com/stream?token=abc&v=1".into(),
                title: "stream".into(),
                last_pts: 125.5,
                total_secs: 1800.0,
                timestamp: 1732450000,
            },
            HistoryItem {
                url_or_path: "/home/user/Videos/test \"video\".mp4".into(),
                title: "test \"video\".mp4".into(),
                last_pts: 60.0,
                total_secs: 120.0,
                timestamp: 1732451000,
            },
        ];

        let json = HistoryManager::serialize_json(&items);
        let parsed = HistoryManager::parse_json(&json);

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].url_or_path, items[0].url_or_path);
        assert_eq!(parsed[0].last_pts, 125.5);
        assert_eq!(parsed[1].title, "test \"video\".mp4");
        assert_eq!(parsed[1].progress_pct(), 50.0);
    }
}
