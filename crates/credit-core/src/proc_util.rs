//! 子进程启动工具。

use std::process::Command;

/// Windows 上给命令加 CREATE_NO_WINDOW：控制台子系统子进程（node / tasklist / powershell / cmd，
/// 以及 ELECTRON_RUN_AS_NODE 方式拉起的 GUI exe）从 GUI 应用启动时会弹出黑色控制台窗口，
/// 该标志让其完全隐藏、后台运行。非 Windows 平台原样返回。
pub fn hide_console(cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    cmd
}
