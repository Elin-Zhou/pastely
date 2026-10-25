//! 诊断：真实调用 GitHub Releases 检测链路（HTTP + JSON 解析 + 版本比较）。
//!
//! 运行：`cargo run --example update_check`（可带参数指定 repo）。
//! 预期：当前版本 == 最新 Release 时输出 UpToDate；有新版时打印候选 DMG。

use cliphub::update::{check_latest, CURRENT_VERSION};

fn main() {
    let repo = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Elin-Zhou/pastely".into());
    println!("检测 {repo}（当前版本 {CURRENT_VERSION}）…");
    match check_latest(&repo) {
        Ok(None) => println!("已是最新版本（{CURRENT_VERSION}）"),
        Ok(Some(info)) => {
            println!("发现新版本: {} ({})", info.version, info.tag);
            println!("  标题: {}", info.name);
            println!("  DMG: {}", info.dmg_url);
            println!("  页面: {}", info.html_url);
        }
        Err(e) => {
            eprintln!("检测失败: {e}");
            std::process::exit(1);
        }
    }
}
