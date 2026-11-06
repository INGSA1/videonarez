//! Источники захвата: мониторы (DXGI), окна приложений, видеокарта, микрофоны.

use serde::Serialize;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, MB_ICONASTERISK, WS_EX_TOOLWINDOW,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    /// Индекс выхода DXGI на основном адаптере (ddagrab output_idx / vsrc_amf monitor_index).
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    /// HMONITOR для gfxcapture.
    #[serde(skip)]
    pub hmonitor: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub vendor_id: u32,
    /// nvenc | amf | qsv | other
    pub vendor: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
    pub exe: String,
    pub width: i32,
    pub height: i32,
}

fn wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Видеокарта и её мониторы (адаптер 0 — тот, на котором работают ddagrab / AMF).
pub fn gpu_and_monitors() -> (Option<GpuInfo>, Vec<MonitorInfo>) {
    let mut monitors = Vec::new();
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(_) => return (None, monitors),
    };
    let Ok(adapter) = (unsafe { factory.EnumAdapters1(0) }) else {
        return (None, monitors);
    };
    let gpu = unsafe { adapter.GetDesc1() }.ok().map(|d| GpuInfo {
        name: wide(&d.Description),
        vendor_id: d.VendorId,
        vendor: match d.VendorId {
            0x10DE => "nvenc",
            0x1002 => "amf",
            0x8086 => "qsv",
            _ => "other",
        }
        .into(),
    });
    let mut j = 0u32;
    while let Ok(out) = unsafe { adapter.EnumOutputs(j) } {
        if let Ok(d) = unsafe { out.GetDesc() } {
            let r: RECT = d.DesktopCoordinates;
            monitors.push(MonitorInfo {
                index: j,
                name: wide(&d.DeviceName).trim_start_matches("\\\\.\\").to_string(),
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                x: r.left,
                y: r.top,
                primary: r.left == 0 && r.top == 0,
                hmonitor: d.Monitor.0 as usize as u64,
            });
        }
        j += 1;
    }
    (gpu, monitors)
}

fn process_exe(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or("").to_string()
    }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        // Свёрнутые окна оставляем: игра может быть в alt-tab в момент выбора источника.
        let minimized = IsIconic(hwnd).as_bool();
        if GetWindow(hwnd, GW_OWNER).map(|h| !h.0.is_null()).unwrap_or(false) {
            return BOOL(1);
        }
        if (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        // У свёрнутого окна размер неизвестен (0 — захват в исходном размере).
        let (w, h) = if minimized { (0, 0) } else { (r.right - r.left, r.bottom - r.top) };
        if !minimized && (w < 160 || h < 120) {
            return BOOL(1);
        }
        let exe = process_exe(pid);
        if pid == std::process::id() || exe.eq_ignore_ascii_case("explorer.exe") && title == "Program Manager" {
            return BOOL(1);
        }
        list.push(WindowInfo { hwnd: hwnd.0 as usize as u64, pid, title, exe, width: w, height: h });
    }
    BOOL(1)
}

/// Видимые окна верхнего уровня (кандидаты для захвата окна игры).
pub fn windows() -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut list as *mut Vec<WindowInfo> as isize));
    }
    list
}

/// «2026-11-06_14-03-27» по местному времени.
pub fn local_stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

pub fn beep() {
    unsafe {
        let _ = MessageBeep(MB_ICONASTERISK);
    }
}
