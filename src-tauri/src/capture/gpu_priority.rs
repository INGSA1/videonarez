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

/// Запретить Windows энергосбережение (EcoQoS) для процесса `pid` (0 — текущий процесс).
///
/// Windows 11 считает фоновым процесс, чьё окно свёрнуто и который ничего не воспроизводит,
/// и переводит его на медленные ядра и грубые таймеры. Для захвата это проседание записи
/// после сворачивания окна. OBS отключает это так же, через SetProcessInformation.
pub fn no_power_throttling(pid: u32) -> bool {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        let (h, own) = if pid == 0 {
            (GetCurrentProcess(), false)
        } else {
            match OpenProcess(PROCESS_SET_INFORMATION, false, pid) {
                Ok(h) => (h, true),
                Err(_) => return false,
            }
        };
        // ControlMask: какими механизмами управляем сами; StateMask = 0: все они выключены.
        let st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            StateMask: 0,
        };
        let ok = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &st as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
        .is_ok();
        if own {
            let _ = CloseHandle(h);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn power_throttling_is_disabled() {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, GetProcessInformation, ProcessPowerThrottling, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_STATE,
        };
        assert!(super::no_power_throttling(0));
        let mut st = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: 0, StateMask: 0 };
        unsafe {
            GetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &mut st as *mut _ as *mut _,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
            .unwrap();
        }
        assert_eq!(st.ControlMask, 0x5, "управляем скоростью и таймерами");
        assert_eq!(st.StateMask, 0, "энергосбережение выключено");
    }
}
