//! Đọc thông tin sử dụng RAM của toàn hệ thống (system-wide), không phải
//! của riêng một tiến trình.

use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// Snapshot trạng thái RAM hệ thống tại một thời điểm.
#[derive(Debug, Clone, Copy)]
pub struct SystemMemoryStatus {
    /// % RAM vật lý đang được sử dụng (0-100), do Windows tự tính.
    pub memory_load_percent: u32,
    /// Tổng RAM vật lý của máy, tính bằng byte.
    pub total_physical_bytes: u64,
    /// RAM vật lý còn khả dụng tại thời điểm truy vấn, tính bằng byte.
    pub available_physical_bytes: u64,
}

/// Lỗi khi không thể truy vấn trạng thái bộ nhớ hệ thống.
#[derive(Debug, thiserror::Error)]
#[error("Không thể đọc trạng thái RAM hệ thống (GlobalMemoryStatusEx thất bại): {0}")]
pub struct SysMemError(#[from] windows::core::Error);

/// Truy vấn trạng thái RAM hiện tại của toàn hệ thống qua WinAPI
/// `GlobalMemoryStatusEx`.
///
/// # Lỗi
/// Trả về `Err` nếu lời gọi WinAPI thất bại (rất hiếm khi xảy ra trên
/// Windows hiện đại, nhưng vẫn xử lý tường minh thay vì panic).
pub fn query_system_memory() -> Result<SystemMemoryStatus, SysMemError> {
    // MEMORYSTATUSEX yêu cầu trường dwLength được set trước khi gọi API,
    // đây là quy ước bắt buộc của WinAPI để version-check cấu trúc.
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };

    // SAFETY: `status` là một struct hợp lệ, đã set đúng dwLength theo
    // yêu cầu của API; con trỏ truyền vào còn sống trong suốt lời gọi.
    unsafe {
        GlobalMemoryStatusEx(&mut status)?;
    }

    Ok(SystemMemoryStatus {
        memory_load_percent: status.dwMemoryLoad,
        total_physical_bytes: status.ullTotalPhys,
        available_physical_bytes: status.ullAvailPhys,
    })
}