//! Cấu hình trung tâm cho toàn bộ ứng dụng.
//!
//! Gom mọi "con số ma thuật" (magic numbers) vào một nơi duy nhất giúp
//! việc tinh chỉnh hành vi (ngưỡng RAM, tốc độ trim, thời gian nghỉ...)
//! không đòi hỏi phải lục lọi khắp codebase.

use std::time::Duration;

/// Ngưỡng % RAM hệ thống để bắt đầu trim. Vượt mức này mới kích hoạt.
pub const RAM_THRESHOLD_PERCENT: f32 = 85.0;

/// Tổng dung lượng muốn trim khỏi working set của Roblox mỗi đợt kích hoạt.
pub const TRIM_TOTAL_TARGET_BYTES: u64 = 512 * 1024 * 1024; // 512 MB

/// Kích thước mỗi bước trim nhỏ. Chia nhỏ 512MB thành các bước này để
/// tránh gây spike I/O / CPU đột ngột (nguyên nhân gây lag khi trim).
pub const TRIM_STEP_BYTES: u64 = 32 * 1024 * 1024; // 32 MB / bước

/// Thời gian nghỉ giữa hai bước trim liên tiếp trong cùng một đợt.
pub const TRIM_STEP_INTERVAL: Duration = Duration::from_millis(1500);

/// Tần suất kiểm tra RAM hệ thống.
pub const RAM_CHECK_INTERVAL: Duration = Duration::from_secs(3);

/// Sau khi hoàn tất một đợt trim (512MB), khóa không cho trim lại trong
/// khoảng thời gian này, kể cả khi RAM vẫn còn trên ngưỡng.
pub const COOLDOWN_AFTER_TRIM: Duration = Duration::from_secs(60);

/// Working set của tiến trình Roblox sẽ không bao giờ bị trim xuống dưới
/// mức này, để tránh gây page-fault thrashing nếu Roblox đang dùng ít RAM.
pub const MIN_WORKING_SET_FLOOR_BYTES: u64 = 128 * 1024 * 1024; // 128 MB

/// Các tên tiến trình Roblox hợp lệ để tìm kiếm (không phân biệt hoa/thường).
/// RobloxPlayerBeta.exe là client chính; RobloxStudioBeta.exe cho Studio.
pub const ROBLOX_PROCESS_NAMES: &[&str] = &["RobloxPlayerBeta.exe", "RobloxStudioBeta.exe"];

/// Đường dẫn file log (đặt cạnh thư mục AppData local của user để tránh
/// cần quyền ghi vào Program Files).
pub const LOG_FILE_NAME: &str = "roblox-ram-trimmer.log";
