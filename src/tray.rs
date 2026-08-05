//! System tray icon and console window control.
//!
//! **Windows (primary):** the binary is a Windows GUI subsystem app so a
//! double-click / Start Menu launch creates **no console** and lands in the
//! tray. Right-click offers **Show Console** and **Exit**; minimizing the
//! console hides it again; closing the console exits the server.
//!
//! When launched from an existing terminal, the parent console is attached so
//! CLI flags (`--help`, `--set-data-dir`, logs) still work, and that shared
//! console is never hidden.
//!
//! **Other platforms:** no-op stubs so the rest of the binary stays portable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Signals from the tray / console that the server should shut down.
#[derive(Debug, Clone)]
pub struct ShutdownFlag(Arc<AtomicBool>);

impl ShutdownFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn clone_inner(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

impl Default for ShutdownFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// How console + tray should behave for this process.
#[derive(Debug, Clone)]
pub struct TrayConfig {
    /// Listen address shown in the tray tooltip (e.g. `127.0.0.1:17843`).
    pub bind: String,
    /// Keep the console visible; do not auto-hide or minimize-to-tray.
    pub foreground: bool,
    /// Skip creating the tray icon entirely.
    pub no_tray: bool,
}

/// Background tray handle. Dropping it requests the tray thread to stop after
/// the server has already begun shutting down.
pub struct TrayGuard {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for TrayGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Wait until [`ShutdownFlag::request`] has been called (polled from async code).
pub async fn wait_for_shutdown(flag: ShutdownFlag) {
    while !flag.is_requested() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Attach to a parent terminal (if any) so CLI output works for a
/// `windows_subsystem = "windows"` binary. Safe to call first thing in `main`.
pub fn prepare_stdio() {
    #[cfg(windows)]
    prepare_stdio_windows();
}

/// Start tray + console management for a normal server run.
///
/// Returns `None` when tray is disabled or unsupported on this platform.
pub fn start(config: TrayConfig, shutdown: ShutdownFlag) -> Option<TrayGuard> {
    start_impl(config, shutdown)
}

// --- Windows implementation -------------------------------------------------

#[cfg(windows)]
fn prepare_stdio_windows() {
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS,
    };

    // Already have a console (tests / odd hosts) — nothing to do.
    if !unsafe { GetConsoleWindow() }.is_null() {
        return;
    }

    // Inherit the caller's terminal when launched from cmd / PowerShell / cargo.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        redirect_stdio_to_console();
    }
}

#[cfg(windows)]
fn start_impl(config: TrayConfig, shutdown: ShutdownFlag) -> Option<TrayGuard> {
    install_console_ctrl_handler(shutdown.clone_inner());

    // Hide only when we exclusively own the console (never hide a parent shell).
    let allow_manage = !config.foreground;
    if allow_manage && owns_console_exclusively() {
        hide_console();
    } else if config.foreground {
        // Ensure there is a visible console for logs when the user asked for one.
        show_console();
    }

    if config.no_tray {
        return None;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let bind = config.bind.clone();

    let join = thread::Builder::new()
        .name("bigshot-tray".into())
        .spawn(move || {
            if let Err(e) = run_tray_loop(&bind, allow_manage, stop_thread, shutdown) {
                tracing::warn!(error = %e, "system tray failed to start");
            }
        })
        .ok()?;

    Some(TrayGuard {
        stop,
        join: Some(join),
    })
}

#[cfg(windows)]
fn run_tray_loop(
    bind: &str,
    allow_manage: bool,
    stop: Arc<AtomicBool>,
    shutdown: ShutdownFlag,
) -> Result<(), String> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
    };

    let menu = Menu::new();
    let show_item = MenuItem::new("Show Console", true, None);
    let exit_item = MenuItem::new("Exit", true, None);
    menu.append(&show_item)
        .map_err(|e| format!("tray menu: {e}"))?;
    menu.append(&exit_item)
        .map_err(|e| format!("tray menu: {e}"))?;

    let tooltip = format!("Big-Shot Voting — http://{bind}");
    let icon = app_icon().map_err(|e| format!("tray icon: {e}"))?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip)
        .with_icon(icon)
        .build()
        .map_err(|e| format!("create tray: {e}"))?;

    let menu_rx = MenuEvent::receiver();
    let tray_rx = TrayIconEvent::receiver();

    tracing::info!("system tray icon ready (right-click for Show Console / Exit)");

    loop {
        if stop.load(Ordering::SeqCst) || shutdown.is_requested() {
            break;
        }

        // Win32 message pump so the tray icon receives clicks/menus.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    shutdown.request();
                    return Ok(());
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(event) = menu_rx.try_recv() {
            if event.id == show_item.id() {
                show_console();
            } else if event.id == exit_item.id() {
                tracing::info!("exit requested from tray menu");
                shutdown.request();
                return Ok(());
            }
        }

        while let Ok(event) = tray_rx.try_recv() {
            // Left-click or double-click restores the console.
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
            | TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_console();
            }
        }

        if allow_manage && owns_console_exclusively() {
            // Minimize → hide completely (taskbar icon goes away; tray remains).
            if console_is_minimized() {
                hide_console();
            }
        }

        thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}

/// True when this process is the only one attached to the console — safe to hide.
#[cfg(windows)]
fn owns_console_exclusively() -> bool {
    use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};

    if unsafe { GetConsoleWindow() }.is_null() {
        return false;
    }

    // Two slots is enough: count > 1 means a parent shell shares the console.
    let mut list: [u32; 2] = [0, 0];
    let count = unsafe { GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32) };
    count == 1
}

#[cfg(windows)]
fn hide_console() {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    let hwnd = unsafe { GetConsoleWindow() };
    if !hwnd.is_null() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(windows)]
fn show_console() {
    use windows_sys::Win32::System::Console::{AllocConsole, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let mut hwnd = unsafe { GetConsoleWindow() };
    if hwnd.is_null() {
        if unsafe { AllocConsole() } == 0 {
            tracing::warn!("AllocConsole failed; cannot show console window");
            return;
        }
        redirect_stdio_to_console();
        hwnd = unsafe { GetConsoleWindow() };
    }

    if hwnd.is_null() {
        return;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Point OS stdin/stdout/stderr at the current console after Attach/AllocConsole.
#[cfg(windows)]
fn redirect_stdio_to_console() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    fn open_con(path: &str, access: u32) -> Option<HANDLE> {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: path is a valid NUL-terminated wide string; CONIN$/CONOUT$ are
        // opened with standard share modes for console rebinding.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(handle)
        }
    }

    unsafe {
        if let Some(h) = open_con("CONIN$", GENERIC_READ) {
            let _ = SetStdHandle(STD_INPUT_HANDLE, h);
        }
        if let Some(h_out) = open_con("CONOUT$", GENERIC_WRITE) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, h_out);
            if let Some(h_err) = open_con("CONOUT$", GENERIC_WRITE) {
                let _ = SetStdHandle(STD_ERROR_HANDLE, h_err);
            } else {
                let _ = SetStdHandle(STD_ERROR_HANDLE, h_out);
            }
        }
    }
}

#[cfg(windows)]
fn console_is_minimized() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};

    let hwnd = unsafe { GetConsoleWindow() };
    if hwnd.is_null() {
        return false;
    }
    // Only treat as "user minimized" when the window is still visible in the taskbar.
    unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) != 0 }
}

#[cfg(windows)]
fn install_console_ctrl_handler(shutdown: Arc<AtomicBool>) {
    use std::sync::OnceLock;

    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT,
        CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };

    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    let _ = FLAG.set(shutdown);

    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                if let Some(flag) = FLAG.get() {
                    flag.store(true, Ordering::SeqCst);
                }
                // TRUE = handled. For CLOSE, Windows still terminates after a
                // short grace period; signalling our flag lets axum shut down
                // cleanly within that window.
                1
            }
            _ => 0,
        }
    }

    let ok = unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
    if ok == 0 {
        tracing::warn!("failed to install console control handler");
    }
}

/// Simple 32×32 RGBA icon (ballot-box blue with a white check).
#[cfg(windows)]
fn app_icon() -> Result<tray_icon::Icon, String> {
    const S: u32 = 32;
    let mut rgba = vec![0u8; (S * S * 4) as usize];

    let put = |rgba: &mut [u8], x: i32, y: i32, r: u8, g: u8, b: u8, a: u8| {
        if x < 0 || y < 0 || x >= S as i32 || y >= S as i32 {
            return;
        }
        let i = ((y as u32 * S + x as u32) * 4) as usize;
        rgba[i] = r;
        rgba[i + 1] = g;
        rgba[i + 2] = b;
        rgba[i + 3] = a;
    };

    // Rounded-ish filled square background.
    let bg = (30u8, 90u8, 180u8);
    let margin = 2i32;
    for y in margin..(S as i32 - margin) {
        for x in margin..(S as i32 - margin) {
            // Soft corners: skip far corners.
            let edge_x = (x - margin).min(S as i32 - 1 - margin - x);
            let edge_y = (y - margin).min(S as i32 - 1 - margin - y);
            if edge_x + edge_y < 2 {
                continue;
            }
            put(&mut rgba, x, y, bg.0, bg.1, bg.2, 255);
        }
    }

    // White checkmark (two thick segments).
    let white = (255u8, 255u8, 255u8);
    // Short leg
    for t in 0..7 {
        let x = 9 + t;
        let y = 16 + t;
        for w in 0..3 {
            put(&mut rgba, x, y + w, white.0, white.1, white.2, 255);
            put(&mut rgba, x + w, y, white.0, white.1, white.2, 255);
        }
    }
    // Long leg
    for t in 0..12 {
        let x = 15 + t;
        let y = 22 - t;
        for w in 0..3 {
            put(&mut rgba, x, y + w, white.0, white.1, white.2, 255);
            put(&mut rgba, x + w, y, white.0, white.1, white.2, 255);
        }
    }

    tray_icon::Icon::from_rgba(rgba, S, S).map_err(|e| e.to_string())
}

// --- Non-Windows stubs ------------------------------------------------------

#[cfg(not(windows))]
fn start_impl(_config: TrayConfig, _shutdown: ShutdownFlag) -> Option<TrayGuard> {
    // Console hide/show and a native tray need platform-specific event loops
    // (gtk on Linux, main-thread runloop on macOS). The server still runs as a
    // normal foreground process here.
    None
}
