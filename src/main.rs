//! ClipHub 装配层：目录初始化、依赖注入、AppKit 事件循环。
//!
//! 所有可变状态集中在泄漏的 [`cliphub::panel::Globals`]，主线程单访
//! （watcher/hotkey/ttl 均为 NSApplication 主循环上的 NSTimer）。

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};

use cliphub::clipboard::WatcherState;
use cliphub::config::Config;
use cliphub::hotkey::HotkeyManager;
use cliphub::panel::{AppController, Globals, PanelController, PanelRuntime};
use cliphub::store::ClipStore;
use cliphub::ttl::reap_once;

fn data_dir() -> PathBuf {
    dirs::config_dir()
        .expect("无法定位用户配置目录")
        .join("Pastely")
}

/// 旧数据目录（`.../cliphub`）整体迁移到新目录；仅当新目录不存在且旧目录存在。
/// 返回是否发生迁移。迁移失败时调用方以空历史照常启动。
fn migrate_data_dir(old: &std::path::Path, new: &std::path::Path) -> std::io::Result<bool> {
    if new.exists() || !old.exists() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::rename(old, new)?;
    Ok(true)
}

fn init_logging(dir: &std::path::Path) {
    let file_appender = tracing_appender::rolling::daily(dir, "cliphub.log");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(file_appender)
        .with_ansi(false)
        .init();
}

/// 打开存储；库损坏则改名 .bak 后重建（可用性优先）。
fn open_store(db_path: &std::path::Path) -> ClipStore {
    match ClipStore::open(db_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::error!("打开数据库失败（{e:#}），备份后重建空库");
            let bak = db_path.with_extension("db.bak");
            let _ = std::fs::rename(db_path, &bak);
            for suffix in ["-wal", "-shm"] {
                let side = PathBuf::from(format!("{}{suffix}", db_path.display()));
                let _ = std::fs::rename(&side, PathBuf::from(format!("{}{suffix}", bak.display())));
            }
            ClipStore::open(db_path).expect("重建数据库失败")
        }
    }
}

fn main() -> anyhow::Result<()> {
    let dir = data_dir();
    // 目录改名迁移（先于 create_dir_all：新目录存在即跳过，幂等）
    let legacy_dir = dirs::config_dir()
        .expect("无法定位用户配置目录")
        .join("cliphub");
    match migrate_data_dir(&legacy_dir, &dir) {
        Ok(true) => tracing::info!(
            "migrated data dir {} -> {}",
            legacy_dir.display(),
            dir.display()
        ),
        Ok(false) => {}
        Err(e) => tracing::error!(
            "数据目录迁移失败（{} -> {}）：{e}；将以空历史启动",
            legacy_dir.display(),
            dir.display()
        ),
    }
    std::fs::create_dir_all(&dir).expect("创建数据目录失败");
    init_logging(&dir);
    tracing::info!("ClipHub 启动，数据目录 {}", dir.display());

    // 配置：损坏回退默认（不写回，保护用户文件）
    let config_path = dir.join("config.toml");
    let config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("配置加载失败（{e:#}），回退默认值");
            Config::default()
        }
    };

    let db_path = dir.join("history.db");
    let store = Arc::new(Mutex::new(open_store(&db_path)));

    let mtm = MainThreadMarker::new().expect("NSApplication 必须在主线程");
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        cliphub::panel::set_accessory(&app);

        // 全局状态（泄漏，进程生命周期）
        let globals: &'static Globals = Box::leak(Box::new(Globals {
            store: store.clone(),
            config: RefCell::new(config.clone()),
            config_path: config_path.clone(),
            watcher: RefCell::new(WatcherState::new(
                config.storage.max_text_chars,
                config.storage.max_image_mb,
            )),
            hotkey: RefCell::new(HotkeyManager::new()?),
            panel: RefCell::new(None),
            runtime: RefCell::new(None),
            ax_hint_shown: RefCell::new(false),
            ax_prompt_shown: RefCell::new(false),
            settings: RefCell::new(None),
            update: RefCell::new(cliphub::update::UpdateManager::new()),
            status_item: RefCell::new(None),
            update_menu_item: RefCell::new(None),
            check_menu_item: RefCell::new(None),
        }));

        // 运行时选择状态（泄漏，field editor 裸指针指向它）
        let runtime: &'static mut PanelRuntime = Box::leak(Box::new(PanelRuntime {
            g: globals,
            filtered: Vec::new(),
            selected: 0,
        }));
        *globals.runtime.borrow_mut() = Some(runtime as *mut PanelRuntime);

        // AppController（ivar 指向 globals）
        let controller = AppController::create(globals);

        // 面板（field editor 在此挂入 controller ivars；列表容器持 globals 指针）
        let panel: &'static PanelController = Box::leak(Box::new(PanelController::new(
            mtm,
            config.ui.max_list_rows,
            runtime as *mut PanelRuntime,
            &controller,
            globals,
        )));
        *globals.panel.borrow_mut() = Some(panel);

        // 主题：auto 跟随系统（动态色自动切换）；light/dark 固定 appearance
        cliphub::panel::apply_panel_theme(&panel.panel, &config.ui.theme);

        // NSApp delegate（状态栏菜单 nil-targeted 动作经此分发）
        use objc2::runtime::ProtocolObject;
        app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));

        // 快捷键注册（失败仅告警，用户可改 config.toml [hotkey].main）
        if let Err(e) = globals.hotkey.borrow_mut().register(&config.hotkey.main) {
            tracing::error!(
                "注册快捷键 {:?} 失败: {e:#}（可修改配置文件 [hotkey].main）",
                config.hotkey.main
            );
        }

        // 定时器：watcher 300ms / hotkey 100ms / ttl 3600s / update 1s
        let timer = |interval: f64, sel: objc2::runtime::Sel| {
            objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                interval,
                &controller,
                sel,
                None,
                true,
            )
        };
        timer(0.3, objc2::sel!(tickWatch:));
        timer(0.1, objc2::sel!(tickHotkey:));
        timer(3600.0, objc2::sel!(tickTtl:));
        timer(1.0, objc2::sel!(tickUpdate:));

        // 状态栏（更新菜单项/图标句柄存入 globals）
        cliphub::panel::build_status_bar(globals);

        // 首次启动：一次性询问开机自启动（run 前同步弹，选完再进事件循环）
        if !config.general.first_run_done {
            let alert: Retained<NSAlert> = msg_send![NSAlert::alloc(mtm), init];
            alert.setMessageText(&objc2_foundation::NSString::from_str(
                "Pastely 需要在后台记录剪切板历史",
            ));
            alert.setInformativeText(&objc2_foundation::NSString::from_str(
                "Pastely 常驻菜单栏监听剪切板变化，开机自启动可确保重启后不漏记录。\n可随时在「设置…」中更改。",
            ));
            // 第一个按钮为默认高亮（NSAlertFirstButtonReturn=1000）
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("开启开机自启动"));
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("暂不"));
            let enable = alert.runModal() == NSAlertFirstButtonReturn;
            let mut registered = false;
            if enable {
                match cliphub::autostart::set_launch_at_login(true) {
                    Ok(()) => registered = true,
                    Err(e) => tracing::error!("设置开机自启动失败（可稍后在设置中重试）: {e:#}"),
                }
            }
            tracing::info!("首次启动选择：launch_at_login={enable} registered={registered}");
            {
                let mut cfg = globals.config.borrow_mut();
                cfg.general.first_run_done = true;
                cfg.general.launch_at_login = registered;
            }
            globals
                .config
                .borrow()
                .save(&config_path)
                .map_err(|e| anyhow::anyhow!("保存首次启动配置失败: {e}"))?;
        }

        // TTL 启动清理一次
        {
            let store = globals.store.lock().expect("store poisoned");
            match reap_once(&store, config.storage.ttl_days, config.storage.max_items) {
                Ok((expired, evicted)) if expired + evicted > 0 => {
                    tracing::info!("启动清理：TTL 过期 {expired} 条，容量淘汰 {evicted} 条");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("启动清理失败: {e:#}"),
            }
        }

        tracing::info!("ClipHub 就绪：快捷键 {:?} 唤起", config.hotkey.main);
        app.run();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_moves_legacy_dir() {
        let base = std::env::temp_dir().join(format!("pastely-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("cliphub");
        let new = base.join("Pastely");
        std::fs::create_dir_all(old.join("nested")).unwrap();
        std::fs::write(
            old.join("config.toml"),
            "[general]\nfirst_run_done = true\n",
        )
        .unwrap();
        assert!(migrate_data_dir(&old, &new).unwrap());
        assert!(!old.exists(), "旧目录整体改名");
        assert!(new.join("config.toml").exists(), "内容随目录迁移");
        assert!(!migrate_data_dir(&old, &new).unwrap(), "已迁移后幂等跳过");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn migration_skips_when_new_exists_or_old_missing() {
        let base = std::env::temp_dir().join(format!("pastely-mig2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("cliphub");
        let new = base.join("Pastely");
        assert!(!migrate_data_dir(&old, &new).unwrap(), "旧目录不存在：跳过");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(new.join("keep"), "x").unwrap();
        assert!(!migrate_data_dir(&old, &new).unwrap(), "新目录已存在：跳过");
        assert!(old.exists(), "旧目录保留不动");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
