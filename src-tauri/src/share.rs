//! «Скопировать для Discord»: кладёт файл клипа в буфер обмена Windows как файл (CF_HDROP),
//! чтобы его можно было вставить через Ctrl+V в Discord, Telegram или проводник.

use crate::error::{AppError, AppResult};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, POINT};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::DROPFILES;

const CF_HDROP: u32 = 15;

/// Размер файла в байтах возвращается, чтобы интерфейс мог предупредить о лимите Discord.
#[tauri::command]
pub fn copy_file_to_clipboard(path: String) -> AppResult<u64> {
    let size = std::fs::metadata(&path)
        .map_err(|e| AppError::msg(format!("Файл не найден: {e}")))?
        .len();
    // DROPFILES + список путей в UTF-16, каждый завершён нулём, в конце ещё один ноль.
    let wide: Vec<u16> = path.replace('/', "\\").encode_utf16().chain([0, 0]).collect();
    let header = std::mem::size_of::<DROPFILES>();
    let bytes = header + wide.len() * 2;
    unsafe {
        let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| AppError::msg(e.to_string()))?;
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(AppError::msg("Не удалось выделить память для буфера обмена"));
        }
        let df = DROPFILES { pFiles: header as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
        std::ptr::write_unaligned(p as *mut DROPFILES, df);
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, p.add(header), wide.len() * 2);
        let _ = GlobalUnlock(mem);

        // Буфер обмена может быть на мгновение занят другой программой — несколько попыток.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if !opened {
            let _ = GlobalFree(Some(mem));
            return Err(AppError::msg("Буфер обмена занят другой программой, попробуйте ещё раз"));
        }
        let _ = EmptyClipboard();
        let res = SetClipboardData(CF_HDROP, Some(HANDLE(mem.0)));
        let _ = CloseClipboard();
        if res.is_err() {
            let _ = GlobalFree(Some(mem));
            return Err(AppError::msg("Не удалось записать файл в буфер обмена"));
        }
    }
    Ok(size)
}
