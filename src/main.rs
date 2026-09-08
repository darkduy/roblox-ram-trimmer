//! Roblox RAM Trimmer
//!
//! Ứng dụng nền chạy trên system tray, tự động trim 512MB working set
//! của tiến trình Roblox đang chiếm nhiều RAM nhất khi RAM hệ thống
//! vượt 85%. Việc trim được chia thành nhiều bước nhỏ (mặc định 32MB
//! mỗi bước, cách nhau 1.5s) để tránh gây spike I/O/CPU đột ngột làm
//! giật lag trong lúc chơi game.
//!
//! # Kiến trúc
//! - `config`: mọi hằng số điều chỉnh hành vi, tập trung một chỗ.
//! - `sysmem`: đọc % RAM toàn hệ thống qua WinAPI.
//! - `roblox_process`: tìm tiến trình Roblox và trim working set của nó.
//! - `app_state`: trạng thái runtime chia sẻ an toàn giữa các thread.
//! - `monitor`: vòng lặp giám sát chạy trên thread nền, quyết định khi
//!   nào trim và quản lý cooldown.
//! - `tray_ui`: giao diện system tray, chạy trên main thread (yêu cầu
//!   của Win32 message loop).
//!
//! # An toàn / giới hạn cần biết
//! `SetProcessWorkingSetSizeEx` chỉ ra lệnh cho Windows đẩy các trang bộ
//! nhớ ít dùng ra khỏi working set — đây là hành vi best-effort, không
//! phải một cam kết cứng. Roblox có thể chạm lại các trang đó ngay sau
//! nếu đang thực sự cần dùng, khiến RAM tăng trở lại. App cần chạy với
//! quyền đủ để mở tiến trình Roblox bằng `PROCESS_SET_QUOTA` — nếu
//! Roblox chạy dưới một tài khoản/session khác, thao tác trim sẽ báo lỗi
//! rõ ràng thay vì thất bại âm thầm.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Trên release build, dòng trên ẩn cửa sổ console (app chỉ hiện tray
// icon). Trên debug build, console vẫn hiện để tiện xem log trực tiếp.

mod app_state;
mod config;
mod monitor;
mod roblox_process;
mod sysmem;
mod tray_ui;

use app_state::AppStateHandle;

fn main() {
    init_logging();

    log::info!("=== Roblox RAM Trimmer khởi động ===");
    log::info!(
        "Cấu hình: ngưỡng RAM {}%, trim {}MB/đợt, bước {}MB, cooldown {}s",
        config::RAM_THRESHOLD_PERCENT,
        config::TRIM_TOTAL_TARGET_BYTES / 1024 / 1024,
        config::TRIM_STEP_BYTES / 1024 / 1024,
        config::COOLDOWN_AFTER_TRIM.as_secs()
    );

    let state = AppStateHandle::new();

    // Vòng lặp giám sát RAM chạy trên thread nền riêng biệt, độc lập
    // hoàn toàn với UI thread — đảm bảo việc trim (có thể mất vài giây
    // do chia nhỏ thành nhiều bước) không bao giờ làm treo tray icon.
    let monitor_state = state.clone();
    std::thread::spawn(move || {
        monitor::run_monitor_loop(monitor_state);
    });

    // Tray UI phải chạy trên main thread vì nó sở hữu Win32 message
    // loop — đây là yêu cầu bắt buộc của Shell_NotifyIcon trên Windows.
    tray_ui::run_tray_ui(state);
}

/// Thiết lập ghi log ra file trong thư mục `%LOCALAPPDATA%` của người
/// dùng, vì app chạy ẩn (không có console ở release build) nên đây là
/// cách duy nhất để chẩn đoán sự cố sau này.
fn init_logging() {
    let log_dir = std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());

    let log_path = log_dir.join(config::LOG_FILE_NAME);

    // Nếu không mở được file log (ví dụ do quyền), app vẫn tiếp tục
    // chạy bình thường — thiếu log không phải lỗi nghiêm trọng đủ để
    // ngăn cản chức năng chính của ứng dụng.
    if let Err(err) = simple_logging::log_to_file(&log_path, log::LevelFilter::Info) {
        eprintln!("Không thể khởi tạo file log tại {log_path:?}: {err}");
    }
}
