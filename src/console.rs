//! 窓を出さない（GUI サブシステム）ための補助。タスクスケジューラーからは窓なしで動き、
//! コンソールから起動したときだけ親のコンソールへ標準出力・標準エラーをつなぐ。

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};

fn is_unset(h: *mut c_void) -> bool {
    h.is_null() || h == INVALID_HANDLE_VALUE
}

/// 標準出力が未接続（リダイレクトもされていない）なら、親のコンソールへつなぐ。
pub fn attach_parent_console() {
    // SAFETY: Win32 のコンソール API を、有効な引数で呼ぶだけ。
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if !is_unset(out) {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let con = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if con != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, con);
            SetStdHandle(STD_ERROR_HANDLE, con);
        }
    }
}
