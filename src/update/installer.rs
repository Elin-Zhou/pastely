//! 自动安装链：下载 DMG → 挂载 → ditto 拷贝 → 三段换名原子替换 → 回报新 bundle 路径。
//!
//! 任一步失败向上返回 `Err`（UI 弹窗提示 + 兜底打开 DMG）；替换链严格回滚，
//! 旧应用永不丢失。仅打包的 .app 支持自动安装（`cargo run` 直跑时
//! [`current_bundle`] 为 `None`，UI 层据此引导）。

use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::anyhow;

use super::ReleaseInfo;

/// DMG 体积下限（防 GitHub 错误页 / 截断）。
const MIN_DMG_BYTES: u64 = 1024 * 1024;
/// UDIF 尾块大小（koly trailer）。
const KOLY_TRAILER_LEN: u64 = 512;

/// 定位当前应用 bundle（`.../Pastely.app`）；非打包运行返回 `None`。
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // exe → Contents/MacOS/<bin> → Contents → <App>.app
    let contents = exe.parent()?.parent()?;
    if contents.file_name()? != OsStr::new("Contents") {
        return None;
    }
    let app = contents.parent()?;
    (app.extension()? == OsStr::new("app")).then(|| app.to_path_buf())
}

fn user_agent() -> String {
    format!("Pastely/{}", super::CURRENT_VERSION)
}

/// 下载 DMG 到 `dest` 并做完整性校验（koly 尾魔数 + 体积下限）。
pub fn download_dmg(url: &str, dest: &Path) -> anyhow::Result<()> {
    let agent = super::http_agent();
    let resp = agent
        .get(url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|e| anyhow!("下载失败: {e}"))?;
    if resp.status().as_u16() != 200 {
        anyhow::bail!("下载 HTTP {}", resp.status().as_u16());
    }
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(dest)
        .map_err(|e| anyhow!("创建临时文件 {} 失败: {e}", dest.display()))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| anyhow!("下载写入失败: {e}"))?;
    drop(file);
    verify_dmg(dest)
}

/// DMG 完整性：体积下限 + 末 512 字节为 `koly` 魔数（UDIF trailer 起始）。
fn verify_dmg(path: &Path) -> anyhow::Result<()> {
    let len = std::fs::metadata(path)
        .map_err(|e| anyhow!("读取下载文件元数据失败: {e}"))?
        .len();
    if len < MIN_DMG_BYTES {
        anyhow::bail!("下载体积异常（{len} 字节），疑似错误页或截断");
    }
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::End(-(KOLY_TRAILER_LEN as i64)))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    if &magic != b"koly" {
        anyhow::bail!("DMG 尾部校验失败（koly 魔数缺失）");
    }
    Ok(())
}

/// 执行外部命令：非零退出/找不到命令统一报错（stderr 并入错误信息）。
fn run(cmd: &mut Command, what: &str) -> anyhow::Result<()> {
    let out = cmd.output().map_err(|e| anyhow!("{what} 执行失败: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("{what} 失败: {}", stderr.trim());
    }
    Ok(())
}

/// 下载并安装目标版本，返回新 bundle 路径（供主线程重启）。
pub fn install_release(info: &ReleaseInfo) -> anyhow::Result<PathBuf> {
    let target = current_bundle()
        .ok_or_else(|| anyhow!("无法定位当前应用包（仅打包的 Pastely.app 支持自动安装）"))?;

    // 下载到临时目录（同版本重复更新时覆盖旧临时文件）
    let dmg = std::env::temp_dir().join(format!("pastely-update-{}.dmg", info.version));
    tracing::info!("update: 下载 {}", info.dmg_url);
    download_dmg(&info.dmg_url, &dmg).inspect_err(|_| {
        let _ = std::fs::remove_file(&dmg);
    })?;

    match install_from_dmg(&dmg, &target) {
        Ok(new_bundle) => {
            let _ = std::fs::remove_file(&dmg);
            Ok(new_bundle)
        }
        Err(e) => {
            // 兜底材料保留：UI 层可引导用户手动打开 DMG
            tracing::error!(
                "update: 自动安装失败（DMG 保留于 {}）: {e:#}",
                dmg.display()
            );
            Err(e)
        }
    }
}

/// 挂载 DMG → 拷贝 → 替换 → 卸载（挂载点失败自动清理）。
fn install_from_dmg(dmg: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let mnt = std::env::temp_dir().join("Pastely-update-mnt");
    // 上次异常退出可能残留挂载/目录：先尝试强卸载再清空
    let _ = Command::new("hdiutil")
        .arg("detach")
        .arg(&mnt)
        .arg("-force")
        .output();
    let _ = std::fs::remove_dir_all(&mnt);
    std::fs::create_dir_all(&mnt).map_err(|e| anyhow!("创建挂载点失败: {e}"))?;

    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mnt)
        .arg(dmg)
        .output();
    let attached = matches!(&attach, Ok(o) if o.status.success());
    if attached {
        tracing::info!("update: DMG 已挂载于 {}", mnt.display());
    } else {
        let err = attach
            .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
            .unwrap_or_else(|e| e.to_string());
        let _ = std::fs::remove_dir_all(&mnt);
        anyhow::bail!("挂载 DMG 失败: {err}");
    }

    let result = copy_and_swap(&mnt, target);
    // 无论成败都卸载（短暂占用无碍）
    let _ = Command::new("hdiutil").arg("detach").arg(&mnt).output();
    let _ = std::fs::remove_dir_all(&mnt);
    result
}

/// 在挂载卷内找 .app → ditto 拷到目标同卷暂存目录 → 三段换名替换。
fn copy_and_swap(mnt: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let mounted_app = find_app_in_dir(mnt).ok_or_else(|| anyhow!("DMG 中未找到 .app"))?;
    let staged = staging_path(target);
    let _ = std::fs::remove_dir_all(&staged);
    run(
        Command::new("ditto").arg(&mounted_app).arg(&staged),
        "拷贝应用（ditto）",
    )?;
    match swap_bundles(&staged, target) {
        Ok(()) => Ok(target.to_path_buf()),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staged);
            Err(e)
        }
    }
}

/// 目录（含子目录）内首个 `*.app`。
fn find_app_in_dir(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == OsStr::new("app")) && p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// 同卷暂存路径（隐藏目录，避免出现在 Finder；失败重启用新 pid 不冲突）。
fn staging_path(target: &Path) -> PathBuf {
    let name = format!(
        ".{}.new-{}",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    );
    target.parent().unwrap_or(Path::new("/tmp")).join(name)
}

/// 三段换名原子替换：target→.old → staged→target（失败回滚 .old→target）。
///
/// `staged` 与 `target` 必须在同一卷（rename 跨卷失败），由 [`staging_path`] 保证。
pub fn swap_bundles(staged: &Path, target: &Path) -> anyhow::Result<()> {
    let old = {
        let name = format!(
            ".{}.old-{}",
            target.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        );
        target.parent().unwrap_or(Path::new("/tmp")).join(name)
    };
    let _ = std::fs::remove_dir_all(&old);
    if target.exists() {
        std::fs::rename(target, &old).map_err(|e| anyhow!("移走旧应用失败: {e}"))?;
    }
    match std::fs::rename(staged, target) {
        Ok(()) => {
            // 新版就位；旧包清理失败不致命（隐藏目录，下次更新再清）
            let _ = std::fs::remove_dir_all(&old);
            Ok(())
        }
        Err(e) => {
            // 回滚：恢复旧应用
            if old.exists() {
                let _ = std::fs::rename(&old, target);
            }
            anyhow::bail!("换入新应用失败（已回滚）: {e}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_bundle_none_outside_app_bundle() {
        // 单测经 cargo test 直跑：可执行不在 .app 内
        assert!(current_bundle().is_none());
    }

    #[test]
    fn find_app_in_dir_finds_first_app() {
        let base = std::env::temp_dir().join(format!("pastely-findapp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let app = base.join("Pastely.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(base.join("not-an-app.txt")).unwrap();
        assert_eq!(find_app_in_dir(&base), Some(app));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn find_app_in_dir_empty_or_missing() {
        let base = std::env::temp_dir().join(format!("pastely-findapp2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert_eq!(find_app_in_dir(&base), None);
        assert_eq!(find_app_in_dir(&base.join("missing")), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn swap_bundles_installs_and_cleans_old() {
        let base = std::env::temp_dir().join(format!("pastely-swap1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("Pastely.app");
        std::fs::create_dir_all(target.join("Contents")).unwrap();
        std::fs::write(target.join("Contents/old"), "old").unwrap();
        let staged = base.join(".staged-1");
        std::fs::create_dir_all(staged.join("Contents")).unwrap();
        std::fs::write(staged.join("Contents/new"), "new").unwrap();

        swap_bundles(&staged, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/new")).unwrap(),
            "new"
        );
        assert!(!staged.exists(), "staged 已换名为 target");
        // 旧包已清理（隐藏 .old-*）
        let leftovers: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".old-"))
            .collect();
        assert!(leftovers.is_empty(), "old 残留: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn swap_bundles_first_install_without_target() {
        let base = std::env::temp_dir().join(format!("pastely-swap2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("Pastely.app");
        let staged = base.join("staged.app");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("new"), "n").unwrap();

        swap_bundles(&staged, &target).unwrap();
        assert!(target.join("new").exists());
        assert!(!staged.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn swap_bundles_rolls_back_on_failure() {
        let base = std::env::temp_dir().join(format!("pastely-swap3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("Pastely.app");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old"), "o").unwrap();
        // staged 不存在 → rename 必败 → 必须回滚 target
        let staged = base.join(".staged-missing");
        assert!(swap_bundles(&staged, &target).is_err());
        assert!(target.join("old").exists(), "旧应用已回滚");
        assert!(!target.join("new").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn verify_dmg_rejects_truncated_and_bad_magic() {
        let base = std::env::temp_dir().join(format!("pastely-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let p = base.join("tiny.dmg");
        std::fs::write(&p, b"not a dmg").unwrap();
        assert!(verify_dmg(&p).is_err(), "体积下限拦截");

        let p2 = base.join("badmagic.dmg");
        let mut big = vec![0u8; 2 * 1024 * 1024];
        let len = big.len();
        big[len - 512..len - 508].copy_from_slice(b"JUNK");
        std::fs::write(&p2, &big).unwrap();
        assert!(verify_dmg(&p2).is_err(), "koly 魔数拦截");

        big[len - 512..len - 508].copy_from_slice(b"koly");
        std::fs::write(&p2, &big).unwrap();
        assert!(verify_dmg(&p2).is_ok(), "合法 koly 尾块通过");
        let _ = std::fs::remove_dir_all(&base);
    }
}
