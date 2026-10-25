//! 老系统（macOS 10.13–12.x）开机自启动：LSSharedFileList 会话登录项。
//!
//! 13+ 走 [`crate::autostart`] 的 SMAppService；本模块仅在那之下被分派。
//! API 自 10.11 deprecated 但在 12.x 及以前完全可用；为规避 dyld 启动期
//! 符号解析风险，`kLSSharedFileListSessionLoginItems` 等常量一律运行时构造
//! 字符串（值即其文档常量字符串），不引用 extern static。

use std::ffi::{c_int, c_long, c_void};
use std::path::Path;

use anyhow::anyhow;
use core_foundation::base::{CFRelease, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;

// CF 类型句柄（不透明指针）
type LSSharedFileListRef = *mut c_void;
type LSSharedFileListItemRef = *mut c_void;
type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;

/// `kLSSharedFileListSessionLoginItems` 的文档常量值（运行时构造，免 extern static）。
const SESSION_LOGIN_ITEMS: &str = "com.apple.LSSharedFileList.SessionLoginItems";
/// `kLSSharedFileListNoUserInteraction | kLSSharedFileListDoNotMountVolumes`。
const RESOLVE_FLAGS: u32 = 0b11;
/// CFURL path style：POSIX。
const K_CF_URL_POSIX_PATH_STYLE: c_long = 0;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSSharedFileListCreate(
        allocator: CFAllocatorRef,
        list_type: CFStringRef,
        list_options: CFDictionaryRef,
    ) -> LSSharedFileListRef;
    fn LSSharedFileListCopySnapshot(in_list: LSSharedFileListRef, out_seed: *mut u32)
        -> CFArrayRef;
    fn LSSharedFileListInsertItemURL(
        in_list: LSSharedFileListRef,
        insert_after_this_item: LSSharedFileListItemRef,
        in_display_name: CFStringRef,
        in_icon_ref: *const c_void,
        in_url: *const c_void,
        properties_to_set: CFDictionaryRef,
        properties_to_clear: CFDictionaryRef,
    ) -> LSSharedFileListItemRef;
    fn LSSharedFileListItemRemove(
        in_list: LSSharedFileListRef,
        in_item: LSSharedFileListItemRef,
    ) -> c_int;
    fn LSSharedFileListItemCopyResolvedURL(
        in_item: LSSharedFileListItemRef,
        in_flags: u32,
        out_error: *mut *mut c_void,
    ) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: CFArrayRef) -> c_long;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: c_long) -> *const c_void;
    fn CFURLCopyFileSystemPath(url: *const c_void, path_style: c_long) -> CFStringRef;
}

/// `kLSSharedFileListItemLast`（头文件 #define 指针哨兵值 1）。
/// `dangling_mut::<c_void>()` 的位模式恰为地址 1，与哨兵定义一致。
fn item_last() -> LSSharedFileListItemRef {
    std::ptr::dangling_mut::<c_void>()
}

/// 路径等价比较：去尾斜杠 + 大小写不敏感（macOS 默认文件系统）。
fn item_path_matches(item: &str, bundle: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
    norm(item) == norm(bundle)
}

/// 当前 bundle 路径字符串（无 bundle 返回 `None`）。
fn bundle_path() -> Option<String> {
    crate::update::installer::current_bundle().map(|p| p.to_string_lossy().into_owned())
}

/// 快照中匹配当前 bundle 的 item（找到即停，返还裸指针，调用方勿释放）。
unsafe fn find_self_item(list: LSSharedFileListRef) -> Option<LSSharedFileListItemRef> {
    let bundle = bundle_path()?;
    let mut seed: u32 = 0;
    let snapshot = LSSharedFileListCopySnapshot(list, &mut seed);
    if snapshot.is_null() {
        return None;
    }
    let mut found: Option<LSSharedFileListItemRef> = None;
    let count = CFArrayGetCount(snapshot);
    for i in 0..count {
        let item = CFArrayGetValueAtIndex(snapshot, i) as LSSharedFileListItemRef;
        if item.is_null() {
            continue;
        }
        let url = LSSharedFileListItemCopyResolvedURL(item, RESOLVE_FLAGS, std::ptr::null_mut());
        if url.is_null() {
            continue;
        }
        let path_ref = CFURLCopyFileSystemPath(url, K_CF_URL_POSIX_PATH_STYLE);
        if !path_ref.is_null() {
            let path = CFString::wrap_under_create_rule(path_ref).to_string();
            if item_path_matches(&path, &bundle) {
                found = Some(item);
            }
        }
        CFRelease(url);
        if found.is_some() {
            break;
        }
    }
    CFRelease(snapshot);
    found
}

/// 当前是否已注册登录项。
pub fn launch_at_login_enabled() -> bool {
    if bundle_path().is_none() {
        return false;
    }
    unsafe {
        let type_str = CFString::new(SESSION_LOGIN_ITEMS);
        let list = LSSharedFileListCreate(
            std::ptr::null(),
            type_str.as_concrete_TypeRef(),
            std::ptr::null(),
        );
        if list.is_null() {
            return false;
        }
        let found = find_self_item(list);
        CFRelease(list);
        found.is_some()
    }
}

/// 开启/关闭登录项（幂等；已存在/不存在时 no-op）。
pub fn set_launch_at_login(enable: bool) -> anyhow::Result<()> {
    unsafe {
        let type_str = CFString::new(SESSION_LOGIN_ITEMS);
        let list = LSSharedFileListCreate(
            std::ptr::null(),
            type_str.as_concrete_TypeRef(),
            std::ptr::null(),
        );
        if list.is_null() {
            return Err(anyhow!("LSSharedFileListCreate 失败"));
        }
        let result = (|| {
            let found = find_self_item(list);
            if enable {
                if found.is_some() {
                    return Ok(());
                }
                let Some(bundle) = bundle_path() else {
                    return Err(anyhow!("非打包运行（无 .app bundle）"));
                };
                let url: CFURL = CFURL::from_path(Path::new(&bundle), true)
                    .ok_or_else(|| anyhow!("bundle 路径转 URL 失败: {bundle}"))?;
                let item = LSSharedFileListInsertItemURL(
                    list,
                    item_last(),
                    std::ptr::null(),
                    std::ptr::null(),
                    url.as_concrete_TypeRef() as *const c_void,
                    std::ptr::null(),
                    std::ptr::null(),
                );
                if item.is_null() {
                    return Err(anyhow!("LSSharedFileListInsertItemURL 失败"));
                }
                CFRelease(item);
                Ok(())
            } else {
                match found {
                    None => Ok(()),
                    Some(item) => {
                        let status = LSSharedFileListItemRemove(list, item);
                        if status == 0 {
                            Ok(())
                        } else {
                            Err(anyhow!("LSSharedFileListItemRemove 失败: {status}"))
                        }
                    }
                }
            }
        })();
        CFRelease(list);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_match_is_case_and_slash_insensitive() {
        assert!(item_path_matches(
            "/Applications/Pastely.app",
            "/Applications/Pastely.app"
        ));
        assert!(item_path_matches(
            "/applications/pastely.app/",
            "/Applications/Pastely.app"
        ));
        assert!(!item_path_matches(
            "/Applications/Other.app",
            "/Applications/Pastely.app"
        ));
        assert!(!item_path_matches("", "/Applications/Pastely.app"));
    }
}
