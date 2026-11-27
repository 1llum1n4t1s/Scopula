//! ごみ箱の設定の判定（M3）。Prunlet の削除はごみ箱が有効なら戻せるが、無効（NukeOnDelete=1）なら
//! 元に戻せない。Scopula は「ごみ箱が無効と確認できたボリューム」だけで削除し、有効・判定不能は見送る。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Storage::FileSystem::{
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
};

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const EXPLORER_POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
const BITBUCKET_VOLUME: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";

/// ボリュームのごみ箱の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trash {
    /// 無効（削除は元に戻せない）。この状態のボリュームでだけ削除する。
    Disabled,
    /// 有効（削除はごみ箱へ移る）。見送る。
    Enabled,
    /// 判定できない。見送る。
    Unknown(String),
}

impl Trash {
    /// 削除してよいか（ごみ箱が無効と確認できたときだけ）。
    pub fn may_delete(&self) -> bool {
        *self == Trash::Disabled
    }

    pub fn describe(&self) -> String {
        match self {
            Trash::Disabled => "ごみ箱は無効".to_string(),
            Trash::Enabled => "ごみ箱が有効".to_string(),
            Trash::Unknown(why) => format!("ごみ箱の設定を判定できません ({why})"),
        }
    }
}

/// ポリシー `NoRecycleFiles` とボリュームの `NukeOnDelete` から状態を決める。ポリシーを優先する。
/// キーが無いのは既定（ごみ箱が有効）。
pub fn decide(policy_no_recycle: Option<u32>, nuke_on_delete: Option<u32>) -> Trash {
    if policy_no_recycle == Some(1) || nuke_on_delete == Some(1) {
        Trash::Disabled
    } else {
        Trash::Enabled
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// レジストリの DWORD を読む。値が無ければ `Ok(None)`。
fn read_dword(root: HKEY, subkey: &str, value: &str) -> Result<Option<u32>, String> {
    let (key, name) = (wide(subkey), wide(value));
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 終端付きの UTF-16 と、4 バイトの出力先を渡している。
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    match rc {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        e => Err(format!("レジストリの読み取りに失敗 (エラー {e})")),
    }
}

/// `\\?\Volume{GUID}\` から `{GUID}` を取り出す。
pub fn guid_from_volume_name(name: &str) -> Option<String> {
    let start = name.find('{')?;
    let end = name[start..].find('}')? + start;
    Some(name[start..=end].to_string())
}

/// `path` があるボリュームの GUID（`{...}`）。ネットワークなど GUID を持たないボリュームではエラー。
fn volume_guid(path: &Path) -> Result<String, String> {
    let mut wpath: Vec<u16> = path.as_os_str().encode_wide().collect();
    wpath.push(0);
    let mut root = vec![0u16; 1024];
    // SAFETY: 終端付きの UTF-16 と、長さを渡したバッファを使う。
    let ok = unsafe { GetVolumePathNameW(wpath.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return Err("ボリュームのパスを取得できない".to_string());
    }
    let mut name = vec![0u16; 1024];
    // SAFETY: GetVolumePathNameW が終端付きで書いたバッファを渡している。
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    };
    if ok == 0 {
        return Err("ボリューム GUID を取得できない（ネットワークなど）".to_string());
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    guid_from_volume_name(&String::from_utf16_lossy(&name[..len]))
        .ok_or_else(|| "ボリューム名に GUID がない".to_string())
}

/// `path` があるボリュームのごみ箱の状態を調べる。
pub fn volume_trash(path: &Path) -> Trash {
    let query = || -> Result<Trash, String> {
        let policy = read_dword(HKEY_LOCAL_MACHINE, EXPLORER_POLICY, "NoRecycleFiles")?.or(
            read_dword(HKEY_CURRENT_USER, EXPLORER_POLICY, "NoRecycleFiles")?,
        );
        let guid = volume_guid(path)?;
        let nuke = read_dword(
            HKEY_CURRENT_USER,
            &format!(r"{BITBUCKET_VOLUME}\{guid}"),
            "NukeOnDelete",
        )?;
        Ok(decide(policy, nuke))
    };
    query().unwrap_or_else(Trash::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ごみ箱の判定はポリシーとnukeondeleteで決まり_キー無しは有効() {
        assert_eq!(decide(None, Some(1)), Trash::Disabled);
        assert_eq!(decide(Some(1), Some(0)), Trash::Disabled);
        assert_eq!(decide(None, Some(0)), Trash::Enabled);
        assert_eq!(decide(None, None), Trash::Enabled);
        assert!(Trash::Disabled.may_delete());
        assert!(!Trash::Enabled.may_delete());
        assert!(!Trash::Unknown("x".into()).may_delete());
    }

    #[test]
    fn ボリューム名からguidを取り出す() {
        let n = r"\\?\Volume{12345678-aaaa-bbbb-cccc-1234567890ab}\";
        assert_eq!(
            guid_from_volume_name(n).as_deref(),
            Some("{12345678-aaaa-bbbb-cccc-1234567890ab}")
        );
        assert_eq!(guid_from_volume_name(r"\\?\C:\"), None);
    }
}
