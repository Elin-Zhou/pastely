//! 开机自启动分派：13+ SMAppService（现代标准）；10.13–12.x LSSharedFileList。
//!
//! 仅打包 .app 生效（以 bundle 定位主应用）；`cargo run` 直跑时注册会失败，
//! 调用方仅记录日志不影响运行。

use objc2::msg_send;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// 开启/关闭开机自启动（按系统版本自动分派实现）。
pub fn set_launch_at_login(enable: bool) -> anyhow::Result<()> {
    if crate::compat::sm_app_service_available() {
        set_via_sm_app_service(enable)
    } else {
        crate::autostart_legacy::set_launch_at_login(enable)
    }
}

/// 当前是否已注册开机自启动。
pub fn launch_at_login_enabled() -> bool {
    if crate::compat::sm_app_service_available() {
        enabled_via_sm_app_service()
    } else {
        crate::autostart_legacy::launch_at_login_enabled()
    }
}

fn set_via_sm_app_service(enable: bool) -> anyhow::Result<()> {
    let service = unsafe { SMAppService::mainAppService() };
    if enable {
        // SAFETY: register 对打包主应用安全；失败返回 NSError
        unsafe { service.registerAndReturnError() }
            .map_err(|e| anyhow::anyhow!("register login item: {}", e.localizedDescription()))?;
    } else {
        // SAFETY: 同上
        unsafe { service.unregisterAndReturnError() }
            .map_err(|e| anyhow::anyhow!("unregister login item: {}", e.localizedDescription()))?;
    }
    Ok(())
}

fn enabled_via_sm_app_service() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    // SAFETY: status 属性未生成绑定，直接 msg_send（返回 NSInteger 包装枚举）
    let status: SMAppServiceStatus = unsafe { msg_send![&service, status] };
    status == SMAppServiceStatus::Enabled
}
