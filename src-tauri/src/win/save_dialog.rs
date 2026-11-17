//! The Win32 [`SaveDialog`] of `calls_export` (T-142; research R18: the native save dialog
//! in Rust, no dialog plugin). Each [`SaveDialog::pick`] runs the common item dialog
//! (`IFileSaveDialog`) on its own short-lived thread ("calls-save-dialog") in a COM
//! single-threaded apartment (the dialog needs an STA and pumps its own modal loop there),
//! never on the main thread or a tauri worker's apartment, and waits for it. The dialog
//! offers one file type (the format's), the suggested name, asks before overwriting and
//! returns only file-system paths. The user's cancel (`HRESULT_FROM_WIN32(ERROR_CANCELLED)`
//! from `Show`) is `Ok(None)` ([`shown`], the one mapping); any other failure is `Err` with
//! the OS code only (#45). The dialog has no owner window: the port's contract carries none.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use voicen_core::call::storage::Format;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileSaveDialog, IFileSaveDialog, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_OVERWRITEPROMPT,
    FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
};

use super::{os_code, wide};
use crate::call::SaveDialog;

/// The common item save dialog as a [`SaveDialog`].
pub struct WinSaveDialog;

impl SaveDialog for WinSaveDialog {
    fn pick(&self, suggested_name: &str, format: Format) -> io::Result<Option<PathBuf>> {
        let name = suggested_name.to_string();
        std::thread::Builder::new()
            .name("calls-save-dialog".into())
            .spawn(move || show(&name, format))?
            .join()
            .map_err(|_| io::Error::other("the save dialog thread panicked"))?
    }
}

/// What `IModalWindow::Show` answered: `Ok(true)` when the user picked a target,
/// `Ok(false)` when they cancelled (`HRESULT_FROM_WIN32(ERROR_CANCELLED)`), else `Err`
/// with the OS code.
pub fn shown(result: windows::core::Result<()>) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(err) if err.code() == ERROR_CANCELLED.to_hresult() => Ok(false),
        Err(err) => Err(io::Error::from_raw_os_error(os_code(&err))),
    }
}

/// The dialog on the current thread, inside its own STA.
fn show(suggested_name: &str, format: Format) -> io::Result<Option<PathBuf>> {
    let _com = Sta::init();
    let os = |err: windows::core::Error| io::Error::from_raw_os_error(os_code(&err));
    let (type_name, spec, extension) = match format {
        Format::Md => (w!("Markdown"), w!("*.md"), w!("md")),
        Format::Json => (w!("JSON"), w!("*.json"), w!("json")),
    };
    let name = wide(suggested_name);
    // SAFETY: COM is initialised on this thread (an STA unless it already had another
    // apartment); every PCWSTR points to a NUL-terminated UTF-16 string that outlives the
    // call it is passed to (`w!` literals are static, `name` lives to the end of this fn).
    let dialog: IFileSaveDialog =
        unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }.map_err(os)?;
    // SAFETY: as above: plain setters on the live dialog of this thread.
    unsafe {
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: type_name,
                pszSpec: spec,
            }])
            .map_err(os)?;
        dialog.SetDefaultExtension(extension).map_err(os)?;
        dialog.SetFileName(PCWSTR(name.as_ptr())).map_err(os)?;
        let options = dialog.GetOptions().map_err(os)?;
        dialog
            .SetOptions(
                options
                    | FOS_OVERWRITEPROMPT
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )
            .map_err(os)?;
    }
    // SAFETY: as above; `Show` runs the modal loop on this thread.
    if !shown(unsafe { dialog.Show(None) })? {
        return Ok(None);
    }
    // SAFETY: `Show` succeeded, so the dialog has a result item; the display name is a
    // CoTaskMemAlloc'd NUL-terminated string, copied once and freed once here.
    let path = unsafe {
        let item = dialog.GetResult().map_err(os)?;
        let text = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(os)?;
        let path = OsString::from_wide(text.as_wide());
        CoTaskMemFree(Some(text.as_ptr() as *const core::ffi::c_void));
        path
    };
    Ok(Some(PathBuf::from(path)))
}

/// An STA on the current thread for the dialog: `CoUninitialize` on drop only when
/// `CoInitializeEx` succeeded.
struct Sta {
    initialized: bool,
}

impl Sta {
    fn init() -> Sta {
        // SAFETY: no reserved pointer; balanced by `Drop` on this thread.
        let hr = unsafe {
            CoInitializeEx(
                None,
                COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0),
            )
        };
        Sta {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for Sta {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: balances the successful `CoInitializeEx` of this thread.
            unsafe { CoUninitialize() };
        }
    }
}
