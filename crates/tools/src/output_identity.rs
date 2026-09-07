//! Handle-based identity checks for generated output capabilities.

use std::fs::File;
use std::io;

#[cfg(unix)]
pub(crate) fn file_identity(file: &File) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
pub(crate) fn file_identity(file: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
    };
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the borrowed File keeps the handle valid; the API initializes
    // the output structure on success before it is read.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("output artifact is a reparse point"));
    }
    Ok(format!(
        "{}:{}:{}",
        information.dwVolumeSerialNumber, information.nFileIndexHigh, information.nFileIndexLow
    ))
}
