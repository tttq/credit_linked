//! 系统托盘（Windows 右下角通知区域）图标。
//!
//! 行为（对齐微信）：
//! - 关闭窗口（标题栏 ✕ / Alt+F4 / 任务栏关闭）→ 弹框二选一：**后台运行** / **退出程序**；
//! - 选「后台运行」→ 主窗口隐藏，托盘小图标常驻通知区域，网关与自动签到继续跑；
//! - 单击 / 双击托盘图标 → 恢复主窗口并置前；
//! - 右键托盘图标 → 菜单「显示主窗口 / 退出程序」。
//!
//! 实现：独立 message-only 窗口 + `Shell_NotifyIconW`；消息泵复用 gpui 主线程的
//! `GetMessageW/DispatchMessageW`（gpui 的循环按线程分发，本隐藏窗口的回调消息同样会被派发）。

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_MODIFY, NOTIFYICONDATAW,
        Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// 托盘 → 本程序的回调消息（WM_APP 段）
    const WM_TRAY_MSG: u32 = WM_APP + 10;
    const TRAY_ID: u32 = 0x4652; // "FR"
    const CMD_SHOW: usize = 1;
    const CMD_QUIT: usize = 2;

    static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
    static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 失败诊断：写一行到工作目录 credit-ui-tray.log（只在失败时写）
    fn log_line(msg: &str) {
        use std::io::Write;
        let line = format!("[tray] {msg} (last_error={})\n", unsafe {
            windows::Win32::Foundation::GetLastError().0
        });
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("credit-ui-tray.log")
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }

    /// 托盘是否已就绪（图标注册成功）。未就绪时调用方应退化为「最小化到任务栏」，
    /// 否则窗口隐藏后用户找不到恢复入口。
    pub fn ready() -> bool {
        TRAY_HWND.load(Ordering::SeqCst) != 0
    }

    /// 启动托盘图标。必须在 UI 线程调用（gpui 的消息循环在该线程泵消息）。
    pub fn init(main_hwnd: isize, tooltip: &str) {
        if MAIN_HWND.swap(main_hwnd, Ordering::SeqCst) != 0 {
            return; // 已初始化
        }
        unsafe {
            let Ok(hinst) = GetModuleHandleW(None) else { return };
            let class = w!("CreditRsTrayWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: HINSTANCE::from(hinst),
                lpszClassName: class,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("credit-rs-tray"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                HINSTANCE::from(hinst),
                None,
            )
            .unwrap_or_default();
            if hwnd.0.is_null() {
                log_line("CreateWindowExW failed");
                MAIN_HWND.store(0, Ordering::SeqCst);
                return;
            }

            // exe 里嵌入的应用图标（build.rs：winresource set_icon → 资源 id = 1）；
            // 万一资源缺失，退回系统默认应用图标，保证托盘仍有可见图标。
            let icon: HICON = match LoadImageW(hinst, PCWSTR(1 as _), IMAGE_ICON, 0, 0, LR_SHARED | LR_DEFAULTSIZE) {
                Ok(handle) => HICON(handle.0),
                Err(_) => LoadIconW(HINSTANCE::default(), IDI_APPLICATION).unwrap_or_default(),
            };

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ID,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY_MSG,
                hIcon: icon,
                ..Default::default()
            };
            let tip = wide(tooltip);
            let n = tip.len().min(127);
            nid.szTip[..n].copy_from_slice(&tip[..n]);

            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                log_line("Shell_NotifyIconW(NIM_ADD) failed");
                let _ = DestroyWindow(hwnd);
                MAIN_HWND.store(0, Ordering::SeqCst);
                return;
            }
            TRAY_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        }
    }

    /// 主窗口隐藏到托盘（右下角小图标常驻）。
    pub fn hide_main() {
        let h = MAIN_HWND.load(Ordering::SeqCst);
        if h == 0 {
            return;
        }
        unsafe {
            let _ = ShowWindowAsync(HWND(h as *mut _), SW_HIDE);
        }
    }

    /// 托盘气泡提示：隐藏后告诉用户图标在右下角、怎么恢复。
    pub fn balloon(title: &str, text: &str) {
        let h = TRAY_HWND.load(Ordering::SeqCst);
        if h == 0 {
            return;
        }
        unsafe {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: HWND(h as *mut _),
                uID: TRAY_ID,
                uFlags: NIF_INFO,
                ..Default::default()
            };
            let t = wide(text);
            let t = &t[..t.len().min(255)];
            nid.szInfo[..t.len()].copy_from_slice(t);
            let ti = wide(title);
            let ti = &ti[..ti.len().min(63)];
            nid.szInfoTitle[..ti.len()].copy_from_slice(ti);
            nid.dwInfoFlags = NIIF_INFO;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// 恢复主窗口并置前（托盘单击 / 菜单「显示主窗口」）。
    pub fn restore_main() {
        let h = MAIN_HWND.load(Ordering::SeqCst);
        if h == 0 {
            return;
        }
        unsafe {
            let hwnd = HWND(h as *mut _);
            let _ = ShowWindowAsync(hwnd, SW_SHOW);
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    unsafe fn show_menu(hwnd: HWND) {
        let Ok(menu) = CreatePopupMenu() else { return };
        let show = wide(crate::i18n::t("tray_show"));
        let quit = wide(crate::i18n::t("tray_quit"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW, PCWSTR(show.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // 不置前的话菜单会「点外面关不掉」
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            CMD_SHOW => restore_main(),
            // 走平台标准退出：主线程 GetMessage 收到 WM_QUIT 后 gpui 循环结束
            CMD_QUIT => {
                PostQuitMessage(0);
            }
            _ => {}
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_TRAY_MSG {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => restore_main(),
                // 经典通知版本下右键是 WM_RBUTTONUP；v4 版本是 WM_CONTEXTMENU
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod imp {
    /// 非 Windows：无托盘实现，调用方退化为最小化到任务栏。
    #[allow(dead_code)]
    pub fn ready() -> bool {
        false
    }
    #[allow(dead_code)]
    pub fn init(_main_hwnd: isize, _tooltip: &str) {}
    #[allow(dead_code)]
    pub fn hide_main() {}
    #[allow(dead_code)]
    pub fn balloon(_title: &str, _text: &str) {}
    #[allow(dead_code)]
    pub fn restore_main() {}
}

#[cfg(not(windows))]
pub use imp::*;
