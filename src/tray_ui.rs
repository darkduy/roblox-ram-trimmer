//! Giao diện system tray: icon, tooltip trạng thái, và menu ngữ cảnh.
//!
//! `tray-icon` yêu cầu chạy trên cùng thread có một Win32 message loop
//! (`GetMessageW`/`DispatchMessageW`) đang hoạt động. Ta tự viết một
//! vòng lặp tối giản thay vì kéo thêm dependency winit/tao chỉ để phục
//! vụ mục đích này — giữ binary nhẹ và ít phụ thuộc.

use crate::app_state::{AppStateHandle, AppStatus, LastTrimInfo};
use crate::config::RAM_THRESHOLD_PERCENT;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

/// ID cố định cho menu item "Thoát", dùng để nhận diện khi xử lý sự kiện click.
const MENU_ID_QUIT: &str = "quit";

/// Sinh dữ liệu icon 16x16 pixel dạng RGBA đơn giản (hình tròn đặc màu
/// xanh) hoàn toàn bằng code — tránh phải đóng gói file .ico riêng, giúp
/// việc build/deploy đơn giản hơn (chỉ một file .exe duy nhất).
fn generate_fallback_icon_rgba(size: u32) -> Vec<u8> {
    let mut buffer = Vec::with_capacity((size * size * 4) as usize);
    let center = size as f32 / 2.0;
    let radius = center - 1.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let inside_circle = (dx * dx + dy * dy).sqrt() <= radius;

            if inside_circle {
                // Xanh dương nhẹ (giống biểu tượng "RAM"/hiệu năng).
                buffer.extend_from_slice(&[64, 156, 255, 255]);
            } else {
                buffer.extend_from_slice(&[0, 0, 0, 0]); // trong suốt
            }
        }
    }

    buffer
}

/// Xây dựng icon tray từ dữ liệu RGBA sinh động ở trên.
fn build_tray_icon() -> Icon {
    const SIZE: u32 = 32;
    let rgba = generate_fallback_icon_rgba(SIZE);
    Icon::from_rgba(rgba, SIZE, SIZE).expect("Dữ liệu RGBA icon không hợp lệ")
}

/// Định dạng dòng hiển thị % RAM hệ thống cho menu item (không tương tác được).
fn format_ram_line(percent: Option<f32>) -> String {
    match percent {
        Some(p) => format!("RAM hệ thống: {p:.1}% (ngưỡng {RAM_THRESHOLD_PERCENT:.0}%)"),
        None => "RAM hệ thống: đang đo...".to_string(),
    }
}

/// Định dạng dòng hiển thị lần trim gần nhất cho menu item.
fn format_last_trim_line(last_trim: Option<&LastTrimInfo>) -> String {
    match last_trim {
        Some(info) => {
            let secs_ago = info.finished_at.elapsed().as_secs();
            format!(
                "Lần trim gần nhất: {:.0}MB từ {} ({}s trước)",
                info.bytes_trimmed as f64 / 1024.0 / 1024.0,
                info.process_name,
                secs_ago
            )
        }
        None => "Chưa có lần trim nào".to_string(),
    }
}

/// Khởi tạo tray icon + menu, sau đó chạy vòng lặp UI vô hạn xử lý sự
/// kiện click chuột và cập nhật tooltip định kỳ.
///
/// Hàm này **block** thread hiện tại — được thiết kế để chạy trên thread
/// chính (main thread) của ứng dụng, trong khi việc giám sát RAM chạy
/// trên một thread nền riêng (xem `monitor::run_monitor_loop`).
pub fn run_tray_ui(state: AppStateHandle) -> ! {
    // Các menu item hiển thị thông tin (status/ram/last-trim) được đánh
    // dấu disabled vì chúng chỉ đóng vai trò label, không phải hành động
    // có thể click — đây là cách làm chuẩn cho "info rows" trong tray menu.
    let status_item = MenuItem::new("Đang khởi động...", false, None);
    let ram_item = MenuItem::new(format_ram_line(None), false, None);
    let last_trim_item = MenuItem::new(format_last_trim_line(None), false, None);
    let quit_item = MenuItem::with_id(MENU_ID_QUIT, "Thoát", true, None);

    let menu = Menu::new();
    menu.append(&status_item).expect("Không thể thêm menu item status");
    menu.append(&ram_item).expect("Không thể thêm menu item RAM");
    menu.append(&last_trim_item)
        .expect("Không thể thêm menu item last-trim");
    menu.append(&PredefinedMenuItem::separator())
        .expect("Không thể thêm separator");
    menu.append(&quit_item).expect("Không thể thêm menu item Thoát");

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Roblox RAM Trimmer — đang khởi động...")
        .with_icon(build_tray_icon())
        .build()
        .expect("Không thể tạo tray icon — kiểm tra hệ thống có hỗ trợ system tray không");

    // Giữ tray_icon sống suốt vòng đời app; nếu bị drop, icon sẽ biến mất
    // khỏi tray. `_tray_icon` chỉ để giữ ownership, không cần đọc lại.
    let _tray_icon = tray_icon;

    let menu_event_receiver = MenuEvent::receiver();
    let tray_event_receiver = TrayIconEvent::receiver();

    let mut msg = MSG::default();
    let mut last_ui_refresh = std::time::Instant::now();

    // Chu kỳ "nhịp tim" của vòng lặp UI: vừa là khoảng chờ sự kiện menu
    // (qua recv_timeout), vừa là tần suất bơm Win32 message queue.
    // 300ms vẫn đủ nhanh để menu/click cảm giác mượt (con người khó nhận
    // ra độ trễ dưới ~300ms cho một thao tác click chuột phải mở menu),
    // nhưng giảm đáng kể số lần CPU phải thức dậy so với polling 50ms
    // trước đây (giảm từ 20 lần/giây xuống ~3.3 lần/giây khi idle).
    const UI_POLL_INTERVAL: Duration = Duration::from_millis(300);
    const UI_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

    loop {
        // Bơm hết các message Win32 đang chờ trong hàng đợi của thread
        // này (bắt buộc để Shell_NotifyIcon và menu popup hoạt động).
        // SAFETY: `msg` là buffer hợp lệ sở hữu bởi thread hiện tại;
        // đây là vòng lặp message tiêu chuẩn của mọi ứng dụng Win32.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Sự kiện click trực tiếp lên icon (không phải menu) — hiện tại
        // không cần hành động đặc biệt, chỉ tiêu thụ để tránh tràn kênh.
        let _ = tray_event_receiver.try_recv();

        // Cập nhật tooltip + nội dung menu định kỳ thay vì mỗi vòng lặp,
        // để tránh gọi WinAPI cập nhật UI quá dày (không cần thiết vì
        // trạng thái RAM chỉ đổi mỗi vài giây).
        if last_ui_refresh.elapsed() >= UI_REFRESH_INTERVAL {
            refresh_ui(&state, &status_item, &ram_item, &last_trim_item, &_tray_icon);
            last_ui_refresh = std::time::Instant::now();
        }

        // Chờ sự kiện menu tối đa UI_POLL_INTERVAL — đây là điểm mấu chốt
        // để tiết kiệm tài nguyên: thread thực sự NGỦ (0% CPU) trong lúc
        // chờ, thay vì busy-poll bằng sleep() cố định như trước. Nếu có
        // click ngay lập tức, hàm trả về ngay (độ trễ ~0ms) thay vì phải
        // đợi tới vòng lặp tiếp theo.
        match menu_event_receiver.recv_timeout(UI_POLL_INTERVAL) {
            Ok(event) if event.id.0 == MENU_ID_QUIT => {
                log::info!("Người dùng chọn Thoát từ tray menu.");
                std::process::exit(0);
            }
            // Menu item khác (nếu sau này thêm) hoặc hết thời gian chờ —
            // cả hai đều quay lại đầu vòng lặp bình thường.
            Ok(_) | Err(_) => {}
        }
    }
}

/// Đọc snapshot trạng thái mới nhất và cập nhật tooltip + các dòng menu.
///
/// Chỉ gọi `state.snapshot()` đúng một lần (thay vì để mỗi hàm định dạng
/// tự lấy snapshot riêng) — tránh khóa mutex và clone dữ liệu 2 lần cho
/// cùng một lần refresh.
fn refresh_ui(
    state: &AppStateHandle,
    status_item: &MenuItem,
    ram_item: &MenuItem,
    last_trim_item: &MenuItem,
    tray_icon: &tray_icon::TrayIcon,
) {
    let snapshot = state.snapshot();

    let status_text = snapshot
        .status
        .as_ref()
        .map(AppStatus::display_text)
        .unwrap_or_else(|| "Đang khởi động...".to_string());

    status_item.set_text(&status_text);
    ram_item.set_text(format_ram_line(snapshot.last_ram_percent));
    last_trim_item.set_text(format_last_trim_line(snapshot.last_trim.as_ref()));

    let tooltip = format!("Roblox RAM Trimmer\n{status_text}");
    let _ = tray_icon.set_tooltip(Some(tooltip));
}