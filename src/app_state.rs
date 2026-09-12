//! Trạng thái ứng dụng được chia sẻ an toàn giữa thread giám sát RAM
//! (background worker) và thread giao diện tray (UI thread).
//!
//! Dùng `Arc<Mutex<...>>` vì tần suất cập nhật thấp (vài giây/lần) nên
//! chi phí khóa mutex là không đáng kể — ưu tiên sự đơn giản, dễ đọc.

use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Trạng thái hoạt động hiện tại của app, hiển thị trên tray tooltip/menu.
#[derive(Debug, Clone)]
pub enum AppStatus {
    /// Đang theo dõi RAM bình thường, chưa vượt ngưỡng.
    Monitoring,
    /// Đang trong quá trình trim, kèm % tiến độ của đợt trim hiện tại.
    Trimming { progress_percent: u8 },
    /// Vừa trim xong, đang trong thời gian khóa (cooldown) trước khi
    /// cho phép trim đợt tiếp theo.
    Cooldown { remaining_secs: u64 },
    /// Gặp lỗi (ví dụ không tìm thấy Roblox, hoặc lỗi WinAPI).
    Error { message: String },
}

impl AppStatus {
    /// Chuỗi hiển thị ngắn gọn cho tooltip tray (giới hạn ~128 ký tự).
    pub fn display_text(&self) -> String {
        match self {
            AppStatus::Monitoring => "Đang theo dõi RAM".to_string(),
            AppStatus::Trimming { progress_percent } => {
                format!("Đang trim RAM Roblox... {progress_percent}%")
            }
            AppStatus::Cooldown { remaining_secs } => {
                format!("Vừa trim xong — nghỉ {remaining_secs}s")
            }
            AppStatus::Error { message } => format!("Lỗi: {message}"),
        }
    }
}

/// Thông tin về lần trim gần nhất, để hiển thị trong menu tray.
#[derive(Debug, Clone)]
pub struct LastTrimInfo {
    pub process_name: String,
    pub bytes_trimmed: u64,
    pub finished_at: Instant,
}

/// Toàn bộ trạng thái runtime được chia sẻ giữa các thread.
#[derive(Debug, Default)]
pub struct SharedAppState {
    pub status: Option<AppStatus>,
    pub last_ram_percent: Option<f32>,
    pub last_trim: Option<LastTrimInfo>,
}

/// Handle nhân bản được (cheap to clone) để truyền trạng thái dùng chung
/// giữa các thread. Bọc `Arc<Mutex<_>>` phía trong.
#[derive(Debug, Clone)]
pub struct AppStateHandle(Arc<Mutex<SharedAppState>>);

impl Default for AppStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl AppStateHandle {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(SharedAppState::default())))
    }

    /// Cập nhật trạng thái hoạt động hiện tại.
    pub fn set_status(&self, status: AppStatus) {
        // unwrap() ở đây là an toàn theo thiết kế: mutex chỉ có thể
        // "poisoned" nếu một thread khác panic trong khi giữ khóa, và
        // toàn bộ app dùng panic="abort" nên trường hợp đó tương đương
        // app đã dừng hẳn — không còn ai đọc lại state nữa.
        if let Ok(mut state) = self.0.lock() {
            state.status = Some(status);
        }
    }

    /// Cập nhật % RAM hệ thống mới nhất vừa đo được.
    pub fn set_ram_percent(&self, percent: f32) {
        if let Ok(mut state) = self.0.lock() {
            state.last_ram_percent = Some(percent);
        }
    }

    /// Ghi nhận kết quả của một đợt trim vừa hoàn tất.
    pub fn record_trim(&self, process_name: String, bytes_trimmed: u64) {
        if let Ok(mut state) = self.0.lock() {
            state.last_trim = Some(LastTrimInfo {
                process_name,
                bytes_trimmed,
                finished_at: Instant::now(),
            });
        }
    }

    /// Đọc snapshot hiện tại của toàn bộ trạng thái (dùng để vẽ lại UI tray).
    pub fn snapshot(&self) -> SharedAppStateSnapshot {
        let state = self.0.lock().ok();
        SharedAppStateSnapshot {
            status: state.as_ref().and_then(|s| s.status.clone()),
            last_ram_percent: state.as_ref().and_then(|s| s.last_ram_percent),
            last_trim: state.as_ref().and_then(|s| s.last_trim.clone()),
        }
    }
}

/// Bản sao dữ liệu tại một thời điểm, tách khỏi khóa mutex — an toàn để
/// giữ lâu và truyền qua lại giữa các closure trên UI thread.
#[derive(Debug, Clone)]
pub struct SharedAppStateSnapshot {
    pub status: Option<AppStatus>,
    pub last_ram_percent: Option<f32>,
    pub last_trim: Option<LastTrimInfo>,
}