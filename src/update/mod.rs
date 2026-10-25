//! 自动更新：GitHub Releases 检测、版本比较与状态机。
//!
//! 线程模型：[`UpdateManager`] 仅主线程访问；检测/下载在后台线程执行，
//! 结果经 `std::sync::mpsc` 回报，主线程 `tickUpdate:` 定时器消费（见 panel）。
//!
//! 检测源：`https://api.github.com/repos/{repo}/releases/latest`（未认证限额
//! 60 次/时，30 分钟节奏余量充足）。检测失败静默保留状态（自动模式下不打扰），
//! 下个周期自动重试。

pub mod installer;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::config::UpdateConfig;

/// 当前应用版本（Cargo.toml 单一来源）。
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// 检测结果 notes 摘要截断（char 数）。
const NOTES_MAX_CHARS: usize = 600;

/// 语义化版本（major.minor.patch），`Ord` 按字段数值比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// 解析 `"v0.1.0"` / `"V1.2"` / `"0.3"` / `"2.0.0-beta"`（后缀忽略）。
    pub fn parse(s: &str) -> Option<Self> {
        let core = s.trim().trim_start_matches(['v', 'V']);
        let core = core.split(['-', '+']).next()?;
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next().unwrap_or("0").parse().ok()?;
        let patch = it.next().unwrap_or("0").parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Version(major, minor, patch))
    }

    pub fn as_str_compact(&self) -> String {
        format!("{}.{}.{}", self.0, self.1, self.2)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.as_str_compact())
    }
}

/// 一个可安装的新版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    /// 原始 tag（如 `v0.2.0`）。
    pub tag: String,
    /// Release 标题。
    pub name: String,
    /// Release notes 摘要（截断至 [`NOTES_MAX_CHARS`]）。
    pub notes: String,
    /// Release 页面链接。
    pub html_url: String,
    /// 选定 DMG 资产的下载地址。
    pub dmg_url: String,
}

/// 更新状态机（仅主线程读写）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available(ReleaseInfo),
    UpToDate,
    Failed(String),
    /// 下载/安装进行中（info 为目标版本）。
    Downloading(ReleaseInfo),
}

/// 后台线程 → 主线程事件。
#[derive(Debug)]
pub enum UpdateEvent {
    Checked {
        manual: bool,
        result: Result<Option<ReleaseInfo>, String>,
    },
    Installed {
        /// 成功 = 新 bundle 路径（主线程 `open -n` 重启）。
        result: Result<PathBuf, String>,
    },
}

/// 从 assets 列表按优先级挑选 DMG：
/// `Pastely-{ver}-{arch}.dmg` → `Pastely-{ver}-universal.dmg` → `Pastely-{ver}.dmg`
/// → 首个任意 `*.dmg`（兼容现网非版本化命名）。
pub fn select_asset(assets: &[(String, String)], version: &str, arch: &str) -> Option<String> {
    let candidates = [
        format!("Pastely-{version}-{arch}.dmg"),
        format!("Pastely-{version}-universal.dmg"),
        format!("Pastely-{version}.dmg"),
    ];
    for name in &candidates {
        if let Some((_, url)) = assets.iter().find(|(n, _)| n == name) {
            return Some(url.clone());
        }
    }
    assets
        .iter()
        .find(|(n, _)| n.to_ascii_lowercase().ends_with(".dmg"))
        .map(|(_, url)| url.clone())
}

/// notes 摘要截断（按 char，不切 UTF-8）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// HTTP 客户端（全局限时；rustls，纯 Rust 无系统依赖）。
pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .new_agent()
}

/// 查询 `releases/latest`；`Ok(None)` = 已是最新。
pub fn check_latest(repo: &str) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let ua = format!("Pastely/{CURRENT_VERSION}");
    let agent = http_agent();
    let mut resp = agent
        .get(&url)
        .header("User-Agent", &ua)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("网络请求失败: {e}"))?;
    if resp.status().as_u16() != 200 {
        return Err(format!("GitHub API HTTP {}", resp.status().as_u16()));
    }
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("读取响应失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("解析响应失败: {e}"))?;

    let tag = v["tag_name"]
        .as_str()
        .ok_or("响应缺少 tag_name")?
        .to_string();
    let latest = Version::parse(&tag).ok_or(format!("无法解析版本号: {tag}"))?;
    let current =
        Version::parse(CURRENT_VERSION).ok_or(format!("无法解析当前版本号: {CURRENT_VERSION}"))?;
    if latest <= current {
        return Ok(None);
    }

    let assets_json = v["assets"].as_array().ok_or("响应缺少 assets")?;
    let assets: Vec<(String, String)> = assets_json
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?.to_string();
            let url = a["browser_download_url"].as_str()?.to_string();
            Some((name, url))
        })
        .collect();
    let dmg_url = select_asset(&assets, &latest.as_str_compact(), std::env::consts::ARCH)
        .ok_or("Release 中没有 DMG 资产")?;

    Ok(Some(ReleaseInfo {
        version: latest,
        tag,
        name: v["name"].as_str().unwrap_or("").to_string(),
        notes: truncate_chars(v["body"].as_str().unwrap_or(""), NOTES_MAX_CHARS),
        html_url: v["html_url"].as_str().unwrap_or("").to_string(),
        dmg_url,
    }))
}

/// 更新管理器：状态 + 后台任务编排。
pub struct UpdateManager {
    pub state: UpdateState,
    tx: Sender<UpdateEvent>,
    rx: Receiver<UpdateEvent>,
    /// 任一后台任务（检测/下载安装）进行中。
    busy: bool,
    last_check: Option<Instant>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            state: UpdateState::Idle,
            tx,
            rx,
            busy: false,
            last_check: None,
        }
    }

    /// 是否有可安装的新版本。
    pub fn available(&self) -> Option<&ReleaseInfo> {
        match &self.state {
            UpdateState::Available(info) | UpdateState::Downloading(info) => Some(info),
            _ => None,
        }
    }

    /// 后台任务是否进行中（检测或下载安装）。
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// 发起检测（`manual=true` 时结果由 UI 弹窗反馈）。
    pub fn check_now(&mut self, repo: &str, manual: bool) {
        if self.busy {
            if manual {
                tracing::warn!("检测更新已在进行中，忽略重复触发");
            }
            return;
        }
        self.busy = true;
        self.state = UpdateState::Checking;
        let tx = self.tx.clone();
        let repo = repo.to_string();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = check_latest(&repo);
                let _ = tx.send(UpdateEvent::Checked { manual, result });
            });
        if spawned.is_err() {
            self.busy = false;
            self.state = UpdateState::Failed("无法启动检测线程".to_string());
            return;
        }
        self.last_check = Some(Instant::now());
    }

    /// 确认安装当前 Available 版本（下载 → 挂载 → 替换 → 事件回报）。
    pub fn confirm_install(&mut self) {
        let UpdateState::Available(info) = self.state.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.state = UpdateState::Downloading(info.clone());
        let tx = self.tx.clone();
        let info_for_thread = info.clone();
        let spawned = std::thread::Builder::new()
            .name("update-install".into())
            .spawn(move || {
                let result =
                    installer::install_release(&info_for_thread).map_err(|e| e.to_string());
                let _ = tx.send(UpdateEvent::Installed { result });
            });
        if spawned.is_err() {
            self.busy = false;
            self.state = UpdateState::Available(info);
        }
    }

    /// 消费后台事件并推进状态机（主线程 tick 调用）。
    pub fn poll(&mut self) -> Vec<UpdateEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            match &ev {
                UpdateEvent::Checked { result, .. } => {
                    self.busy = false;
                    self.state = match result {
                        Ok(Some(info)) => UpdateState::Available(info.clone()),
                        Ok(None) => UpdateState::UpToDate,
                        Err(e) => UpdateState::Failed(e.clone()),
                    };
                }
                UpdateEvent::Installed { result } => {
                    self.busy = false;
                    if let Err(e) = result {
                        self.state = UpdateState::Failed(e.clone());
                    }
                    // 成功时状态保持 Downloading，主线程随即重启进程
                }
            }
            events.push(ev);
        }
        events
    }

    /// 自动检测是否到期（首次启动即检测一次）。
    pub fn should_auto_check(&self, cfg: &UpdateConfig) -> bool {
        if !cfg.auto_check || cfg.interval_minutes == 0 || self.busy {
            return false;
        }
        match &self.state {
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed(_) => {}
            _ => return false,
        }
        match self.last_check {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_secs(u64::from(cfg.interval_minutes) * 60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_order() {
        assert_eq!(Version::parse("v0.1.0"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("V1.2"), Some(Version(1, 2, 0)));
        assert_eq!(Version::parse("0.3"), Some(Version(0, 3, 0)));
        assert_eq!(
            Version::parse("2.0.0-beta.1"),
            Some(Version(2, 0, 0)),
            "预发布后缀忽略"
        );
        assert_eq!(Version::parse("1.2+build.5"), Some(Version(1, 2, 0)));
        assert_eq!(Version::parse("banana"), None);
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("1.2.3.4"), None, "超过三段拒绝");
        assert_eq!(Version::parse("v-1.0"), None);
    }

    #[test]
    fn version_ord_is_numeric_by_field() {
        assert!(Version(0, 2, 0) > Version(0, 1, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert!(Version(0, 1, 10) > Version(0, 1, 9));
        assert!(Version(0, 1, 0) <= Version(0, 1, 0));
    }

    #[test]
    fn current_version_parses() {
        let v = Version::parse(CURRENT_VERSION).expect("Cargo 版本必须可解析");
        assert_eq!(v.as_str_compact(), CURRENT_VERSION);
    }

    #[test]
    fn select_asset_prefers_arch_then_universal_then_plain() {
        let assets = vec![
            ("Pastely-0.2.0-x86_64.dmg".to_string(), "u1".to_string()),
            ("Pastely-0.2.0-aarch64.dmg".to_string(), "u2".to_string()),
            ("Pastely-0.2.0.dmg".to_string(), "u3".to_string()),
        ];
        assert_eq!(
            select_asset(&assets, "0.2.0", "aarch64").as_deref(),
            Some("u2")
        );
        assert_eq!(
            select_asset(&assets, "0.2.0", "x86_64").as_deref(),
            Some("u1")
        );
        // 缺架构版本 → 通用版
        assert_eq!(
            select_asset(&assets, "0.2.0", "riscv").as_deref(),
            Some("u3")
        );
    }

    #[test]
    fn select_asset_falls_back_to_any_dmg() {
        let assets = vec![
            ("checksums.txt".to_string(), "c".to_string()),
            ("Pastely.dmg".to_string(), "u9".to_string()),
        ];
        assert_eq!(
            select_asset(&assets, "0.2.0", "aarch64").as_deref(),
            Some("u9")
        );
        let none = vec![("checksums.txt".to_string(), "c".to_string())];
        assert_eq!(select_asset(&none, "0.2.0", "aarch64"), None);
    }

    #[test]
    fn truncate_keeps_char_boundary() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 5), "hello");
        assert_eq!(truncate_chars("hello", 4), "hell…");
        // 多字节按 char 截断不炸
        assert_eq!(truncate_chars("你好世界", 2), "你好…");
        assert_eq!(truncate_chars("", 3), "");
    }
}
