//! ClipHub 装配层：目录初始化、依赖注入、AppKit 事件循环。
//!
//! 所有可变状态集中在泄漏的 [`cliphub::panel::Globals`]，主线程单访
//! （watcher/hotkey/ttl 均为 NSApplication 主循环上的 NSTimer）。

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

use cliphub::clipboard::WatcherState;
use cliphub::config::Config;
use cliphub::hotkey::HotkeyManager;
use cliphub::panel::{AppController, Globals, PanelController, PanelRuntime};
use cliphub::store::ClipStore;
use cliphub::ttl::reap_once;

fn data_dir() -> PathBuf {
    dirs::config_dir()
        .expect("无法定位用户配置目录")
        .join("cliphub")
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

        // 定时器：watcher 300ms / hotkey 100ms / ttl 3600s
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

        // 状态栏
        cliphub::panel::build_status_bar();

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
