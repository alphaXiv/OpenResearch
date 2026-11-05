use orx_host_proto::{ByteRange, FileIdent, LogInfo, LogPage, MAX_PAGE_LEN};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// A run's log is written once at its run-unique path, so its inode names one generation.
fn open(path: &Path) -> io::Result<Option<(File, LogInfo)>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Reading the directory revalidates NFS negative lookup caches.
            let retry = path.parent().map(std::fs::read_dir).transpose();
            match retry.and_then(|_| File::open(path)) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log is not a regular file",
        ));
    }
    #[cfg(unix)]
    let (dev, ino) = (metadata.dev(), metadata.ino());
    #[cfg(windows)]
    let (dev, ino) = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the file owns a live handle and info has the required layout.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        (
            u64::from(info.dwVolumeSerialNumber),
            u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
        )
    };
    let info = LogInfo {
        ident: FileIdent { dev, ino },
        len: metadata.len(),
    };
    Ok(Some((file, info)))
}

/// `anchor`, when given, must be the 64 bytes just before `range.start`.
pub fn read(
    path: &Path,
    ident: Option<&FileIdent>,
    range: &ByteRange,
    anchor: Option<&[u8; 64]>,
) -> io::Result<LogPage> {
    if range.end < range.start
        || range.end - range.start > MAX_PAGE_LEN
        || (anchor.is_some() && range.start < 64)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid log range",
        ));
    }
    let Some((mut file, info)) = open(path)? else {
        return Ok(LogPage::Reset { info: None });
    };
    if ident.is_some_and(|ident| ident != &info.ident) || info.len < range.start {
        return Ok(LogPage::Reset { info: Some(info) });
    }
    if let Some(expected) = anchor {
        let mut actual = [0; 64];
        file.seek(SeekFrom::Start(range.start - 64))?;
        if file.read_exact(&mut actual).is_err() || &actual != expected {
            return Ok(LogPage::Reset { info: Some(info) });
        }
    }
    let end = range.end.min(info.len);
    file.seek(SeekFrom::Start(range.start))?;
    let mut bytes = vec![0; (end - range.start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(LogPage::Data {
        len: Some(info.len),
        ident: info.ident,
        start: range.start,
        bytes,
        eof: end >= info.len,
    })
}
