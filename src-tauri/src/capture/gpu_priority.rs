//! Приоритет процесса захвата на видеокарте.
//!
//! Если игра работает без ограничения кадров (например, CS2 на 250 FPS) и загружает GPU на 100%,
//! планировщик Windows отдаёт захвату и кодированию лишь остатки времени видеокарты — в записи
//! остаются 2–3 кадра в секунду. OBS решает это так же: поднимает приоритет своего процесса
//! в планировщике GPU через D3DKMTSetProcessSchedulingPriorityClass (gdi32.dll).
//! REALTIME требует прав администратора, поэтому пробуем по убыванию: HIGH, затем ABOVE_NORMAL.

use windows::core::{s, w};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION};

/// D3DKMT_SCHEDULINGPRIORITYCLASS
const ABOVE_NORMAL: i32 = 3;
const HIGH: i32 = 4;

type SetPriorityFn = unsafe extern "system" fn(HANDLE, i32) -> i32;

/// Поднять приоритет процесса `pid` в планировщике GPU. Возвращает установленный уровень.
pub fn raise(pid: u32) -> Option<&'static str> {
    unsafe {
        let lib = LoadLibraryW(w!("gdi32.dll")).ok()?;
        let f = GetProcAddress(lib, s!("D3DKMTSetProcessSchedulingPriorityClass"))?;
        let set: SetPriorityFn = std::mem::transmute(f);
        let h = OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut result = None;
        for (level, name) in [(HIGH, "высокий"), (ABOVE_NORMAL, "выше среднего")] {
            // NTSTATUS: 0 — успех
            if set(h, level) == 0 {
                result = Some(name);
                break;
            }
        }
        let _ = CloseHandle(h);
        result
    }
}
