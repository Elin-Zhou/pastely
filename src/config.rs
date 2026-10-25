//! 应用配置：TOML 持久化、默认值与粘贴模式枚举。
//! 快捷键字符串的解析在 [`crate::hotkey::parse_hotkey`]（唯一实现处）。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 顶层配置（`[hotkey]` / `[storage]` / `[paste]` / `[ui]` 四节）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: HotkeyConfig,
    pub storage: StorageConfig,
    pub paste: PasteConfig,
    pub ui: UiConfig,
}

/// `[hotkey]` 节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// 主弹窗快捷键（`parse_hotkey` 可解析的格式，如 `"Cmd+Shift+V"`）。
    pub main: String,
}

/// `[storage]` 节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// 保留天数；0 = 永久。
    pub ttl_days: u32,
    /// 历史容量上限（条）。
    pub max_items: usize,
    /// 单条文本最大 char 数。
    pub max_text_chars: usize,
    /// 单条图片最大 MB 数。
    pub max_image_mb: u32,
}

/// `[paste]` 节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasteConfig {
    pub mode: PasteMode,
}

/// `[ui]` 节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// 弹窗列表最大行数。
    pub max_list_rows: usize,
    /// 主题：`"auto"` / `"light"` / `"dark"`（装配层解释）。
    pub theme: String,
}

/// 粘贴模式：自动写回剪贴板并粘贴，或仅复制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteMode {
    Auto,
    CopyOnly,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            main: "Cmd+Shift+V".to_string(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            ttl_days: 30,
            max_items: 1000,
            max_text_chars: 100_000,
            max_image_mb: 10,
        }
    }
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self {
            mode: PasteMode::Auto,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            max_list_rows: 12,
            theme: "auto".to_string(),
        }
    }
}

impl Config {
    /// 读取配置：文件不存在 → 写出默认文件并返回默认；TOML 损坏/字段非法 → `Err`。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => toml::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("invalid config file {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(e) => Err(anyhow::anyhow!(
                "failed to read config file {}: {e}",
                path.display()
            )),
        }
    }

    /// 写出配置（TOML）；父目录不存在时先创建。
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("create dir {}: {e}", parent.display()))?;
            }
        }
        let raw =
            toml::to_string_pretty(self).map_err(|e| anyhow::anyhow!("serialize config: {e}"))?;
        std::fs::write(path, raw)
            .map_err(|e| anyhow::anyhow!("write config {}: {e}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一临时文件路径（按测试名隔离，避免并发互踩）。
    fn tmp_config(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cliphub-config-{}-{name}.toml", std::process::id()))
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn defaults_match_spec_exactly() {
        let c = Config::default();
        assert_eq!(c.hotkey.main, "Cmd+Shift+V");
        assert_eq!(c.storage.ttl_days, 30);
        assert_eq!(c.storage.max_items, 1000);
        assert_eq!(c.storage.max_text_chars, 100_000);
        assert_eq!(c.storage.max_image_mb, 10);
        assert_eq!(c.paste.mode, PasteMode::Auto);
        assert_eq!(c.ui.max_list_rows, 12);
        assert_eq!(c.ui.theme, "auto");
    }

    #[test]
    fn save_then_load_roundtrips_modified_values() {
        let path = tmp_config("roundtrip");
        cleanup(&path);
        let mut c = Config::default();
        c.hotkey.main = "Ctrl+Alt+T".to_string();
        c.storage.ttl_days = 7;
        c.storage.max_items = 42;
        c.storage.max_text_chars = 5_000;
        c.storage.max_image_mb = 2;
        c.paste.mode = PasteMode::CopyOnly;
        c.ui.max_list_rows = 6;
        c.ui.theme = "dark".to_string();
        c.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, c);
        cleanup(&path);
    }

    #[test]
    fn saved_default_file_contains_spec_sections() {
        let path = tmp_config("content");
        cleanup(&path);
        Config::default().save(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("main = \"Cmd+Shift+V\""), "raw:\n{raw}");
        assert!(raw.contains("ttl_days = 30"));
        assert!(raw.contains("max_items = 1000"));
        assert!(raw.contains("max_text_chars = 100000"));
        assert!(raw.contains("max_image_mb = 10"));
        assert!(raw.contains("mode = \"auto\""));
        assert!(raw.contains("max_list_rows = 12"));
        assert!(raw.contains("theme = \"auto\""));
        cleanup(&path);
    }

    #[test]
    fn load_missing_file_writes_default_and_returns_default() {
        let path = tmp_config("missing");
        cleanup(&path);
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, Config::default());
        assert!(path.exists(), "default file should be written out");
        // 第二次加载：从磁盘读回同一份
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        cleanup(&path);
    }

    #[test]
    fn load_creates_parent_dirs_when_missing() {
        let dir = std::env::temp_dir().join(format!("cliphub-config-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("config.toml");
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, Config::default());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_toml_is_err() {
        let path = tmp_config("corrupt");
        cleanup(&path);
        std::fs::write(&path, "not [ valid toml {{{").unwrap();
        assert!(Config::load(&path).is_err());
        cleanup(&path);
    }

    #[test]
    fn load_invalid_paste_mode_is_err() {
        let path = tmp_config("bad-mode");
        cleanup(&path);
        std::fs::write(&path, "[paste]\nmode = \"banana\"\n").unwrap();
        assert!(Config::load(&path).is_err());
        cleanup(&path);
    }

    #[test]
    fn load_wrong_field_type_is_err() {
        let path = tmp_config("bad-type");
        cleanup(&path);
        std::fs::write(&path, "[storage]\nttl_days = \"thirty\"\n").unwrap();
        assert!(Config::load(&path).is_err());
        cleanup(&path);
    }

    #[test]
    fn paste_mode_serde_snake_case() {
        assert_eq!(
            toml::to_string(&PasteConfig::default()).unwrap(),
            "mode = \"auto\"\n"
        );
        let copy_only: PasteConfig = toml::from_str("mode = \"copy_only\"").unwrap();
        assert_eq!(copy_only.mode, PasteMode::CopyOnly);
    }

    #[test]
    fn load_partial_file_fills_defaults() {
        let path = tmp_config("partial");
        cleanup(&path);
        std::fs::write(&path, "[storage]\nttl_days = 1\n").unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.storage.ttl_days, 1);
        assert_eq!(c.hotkey.main, "Cmd+Shift+V");
        assert_eq!(c.storage.max_items, 1000);
        assert_eq!(c.paste.mode, PasteMode::Auto);
        cleanup(&path);
    }
}
