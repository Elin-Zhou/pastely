//! 开机自启动：SMAppService main-app LoginItem（macOS 13+，Apple 现代标准）。
//!
//! 仅打包 .app 生效（mainAppService 以 bundle 定位主应用）；`cargo run` 直跑时
//! 注册会失败，调用方仅记录日志不影响运行。

use anyhow::anyhow;
use objc2::msg_send;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// 开启/关闭开机自启动（register / unregister 主应用 LoginItem）。
pub fn set_launch_at_login(enable: bool) -> anyhow::Result<()> {
    let service = unsafe { SMAppService::mainAppService() };
    if enable {
        // SAFETY: register 对打包主应用安全；失败返回 NSError
        unsafe { service.registerAndReturnError() }
            .map_err(|e| anyhow!("register login item: {}", e.localizedDescription()))?;
    } else {
        // SAFETY: 同上
        unsafe { service.unregisterAndReturnError() }
            .map_err(|e| anyhow!("unregister login item: {}", e.localizedDescription()))?;
    }
    Ok(())
}

/// 当前是否已注册开机自启动（registrationStatus == Enabled）。
pub fn launch_at_login_enabled() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    // SAFETY: status 属性未生成绑定，直接 msg_send（返回 NSInteger 包装枚举）
    let status: SMAppServiceStatus = unsafe { msg_send![&service, status] };
    status == SMAppServiceStatus::Enabled
}
