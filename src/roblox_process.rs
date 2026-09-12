//! Tìm kiếm tiến trình Roblox đang chạy và thực hiện trim working set
//! (bộ nhớ vật lý đang được ánh xạ vào tiến trình) theo từng bước nhỏ.

use crate::config::{
    MIN_WORKING_SET_FLOOR_BYTES, ROBLOX_PROCESS_NAMES, TRIM_STEP_BYTES, TRIM_STEP_INTERVAL,
    TRIM_TOTAL_TARGET_BYTES,
};
use std::thread;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
// SetProcessWorkingSetSizeEx thuộc module Memory (không phải Threading,
// dù thao tác trên process handle) trong windows-rs.
use windows::Win32::System::Memory::{SETPROCESSWORKINGSETSIZEEX_FLAGS, SetProcessWorkingSetSizeEx};
use windows::Win32::System::ProcessStatus::{
    EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA, PROCESS_VM_READ,
};

/// Thông tin tối thiểu về một tiến trình Roblox đang chạy, đủ để định vị
/// và mở handle thao tác với nó.
#[derive(Debug, Clone)]
pub struct RobloxProcessInfo {
    pub pid: u32,
    pub name: String,
    /// Working set hiện tại (RAM vật lý thực sự đang chiếm dụng), tính bằng byte.
    pub working_set_bytes: u64,
}

/// Kết quả của một đợt trim: bao nhiêu byte đã trim được trên thực tế
/// (best-effort — có thể ít hơn mục tiêu nếu chạm sàn hoặc lỗi giữa chừng).
#[derive(Debug, Clone, Copy, Default)]
pub struct TrimOutcome {
    pub bytes_trimmed: u64,
    pub steps_completed: u32,
    pub hit_floor: bool,
    /// Bước dồn/nén cuối cùng (EmptyWorkingSet) có chạy thành công không.
    /// Bước này không tính vào `bytes_trimmed` vì mục đích của nó là
    /// giảm phân mảnh working set, không phải trim thêm dung lượng.
    pub defragmented: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RobloxProcessError {
    #[error("Không tìm thấy tiến trình Roblox nào đang chạy")]
    NotFound,
    #[error("Không thể tạo snapshot danh sách tiến trình: {0}")]
    SnapshotFailed(windows::core::Error),
    #[error("Không thể mở handle tới tiến trình PID {pid}: {source}")]
    OpenProcessFailed {
        pid: u32,
        source: windows::core::Error,
    },
    #[error("Không thể đọc thông tin bộ nhớ của tiến trình PID {pid}: {source}")]
    QueryMemoryFailed {
        pid: u32,
        source: windows::core::Error,
    },
    #[error("Trim working set thất bại cho tiến trình PID {pid}: {source}")]
    TrimFailed {
        pid: u32,
        source: windows::core::Error,
    },
}

/// Chuyển buffer UTF-16 có null-terminator (kiểu C-string của Windows)
/// thành `String` UTF-8 thông thường của Rust.
fn wide_str_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

/// Liệt kê mọi tiến trình đang chạy trên hệ thống có tên khớp với danh
/// sách `ROBLOX_PROCESS_NAMES`, kèm theo working set hiện tại của mỗi cái.
fn enumerate_roblox_processes() -> Result<Vec<RobloxProcessInfo>, RobloxProcessError> {
    // SAFETY: CreateToolhelp32Snapshot là lời gọi WinAPI tiêu chuẩn để
    // chụp snapshot danh sách tiến trình; không có tiền điều kiện đặc biệt.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(RobloxProcessError::SnapshotFailed)?;

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut results = Vec::new();

    // SAFETY: `entry` đã được khởi tạo với dwSize đúng theo yêu cầu API;
    // `snapshot` là handle hợp lệ vừa tạo ở trên.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();

    while has_entry {
        let process_name = wide_str_to_string(&entry.szExeFile);

        let is_roblox = ROBLOX_PROCESS_NAMES
            .iter()
            .any(|&name| name.eq_ignore_ascii_case(&process_name));

        if is_roblox {
            // Chỉ những tiến trình mở được handle mới có ích để trim;
            // tiến trình không mở được (ví dụ do quyền) bị bỏ qua thay vì
            // làm hỏng toàn bộ quá trình liệt kê.
            if let Ok(working_set) = query_working_set_size(entry.th32ProcessID) {
                results.push(RobloxProcessInfo {
                    pid: entry.th32ProcessID,
                    name: process_name,
                    working_set_bytes: working_set,
                });
            }
        }

        // SAFETY: cùng handle snapshot, entry được ghi đè tiếp tục hợp lệ.
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }

    // SAFETY: đóng handle snapshot đã mở ở trên, tránh leak resource.
    unsafe {
        let _ = CloseHandle(snapshot);
    }

    Ok(results)
}

/// Đọc working set (RAM vật lý thực chiếm dụng) hiện tại của một tiến
/// trình, dùng một `HANDLE` đã mở sẵn (tránh mở/đóng handle mới mỗi lần
/// gọi — quan trọng khi hàm này được gọi lặp lại nhiều lần trong vòng
/// lặp trim, tiết kiệm chi phí syscall không cần thiết).
fn query_working_set_size_with_handle(
    handle: windows::Win32::Foundation::HANDLE,
    pid: u32,
) -> Result<u64, RobloxProcessError> {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };

    // SAFETY: `handle` do caller đảm bảo hợp lệ (còn sống, đủ quyền
    // PROCESS_QUERY_INFORMATION); `counters` đã set đúng trường `cb`.
    unsafe {
        GetProcessMemoryInfo(
            handle,
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    }
    .map_err(|source| RobloxProcessError::QueryMemoryFailed { pid, source })?;

    Ok(counters.WorkingSetSize as u64)
}

/// Đọc working set (RAM vật lý thực chiếm dụng) hiện tại của một tiến
/// trình theo PID, tự mở và đóng handle riêng — dùng cho các lần gọi
/// đơn lẻ, không lặp lại (ví dụ khi liệt kê danh sách tiến trình).
fn query_working_set_size(pid: u32) -> Result<u64, RobloxProcessError> {
    // SAFETY: OpenProcess với quyền tối thiểu cần thiết chỉ để đọc thông
    // tin bộ nhớ; PID đến từ snapshot hợp lệ của hệ thống.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
        .map_err(|source| RobloxProcessError::OpenProcessFailed { pid, source })?;

    let result = query_working_set_size_with_handle(handle, pid);

    // SAFETY: đóng handle ngay sau khi dùng xong, tránh leak.
    unsafe {
        let _ = CloseHandle(handle);
    }

    result
}

/// Tìm tiến trình Roblox đang chiếm nhiều RAM nhất trong số các tiến
/// trình Roblox hiện có (theo yêu cầu: chỉ trim process lớn nhất).
pub fn find_largest_roblox_process() -> Result<RobloxProcessInfo, RobloxProcessError> {
    let processes = enumerate_roblox_processes()?;

    processes
        .into_iter()
        .max_by_key(|p| p.working_set_bytes)
        .ok_or(RobloxProcessError::NotFound)
}

/// Trim working set của tiến trình chỉ định theo từng bước nhỏ
/// (`TRIM_STEP_BYTES` mỗi lần, nghỉ `TRIM_STEP_INTERVAL` giữa các bước)
/// cho tới khi đạt tổng `TRIM_TOTAL_TARGET_BYTES` hoặc chạm sàn an toàn.
///
/// Đây là hành vi **best-effort**: `SetProcessWorkingSetSizeEx` chỉ ra
/// lệnh cho Windows đẩy các trang bộ nhớ ít dùng ra khỏi working set —
/// không có gì đảm bảo con số trim được khớp chính xác 100% với mục tiêu,
/// và Roblox có thể chạm lại các trang đó ngay sau nếu đang cần dùng.
///
/// # Tham số
/// - `on_step`: callback được gọi sau mỗi bước trim thành công, nhận vào
///   tổng số byte đã trim tính đến thời điểm đó — dùng để cập nhật UI
///   tray theo thời gian thực mà không phải chờ trim xong toàn bộ.
pub fn trim_process_gradually(
    process: &RobloxProcessInfo,
    mut on_step: impl FnMut(u64),
) -> Result<TrimOutcome, RobloxProcessError> {
    // SAFETY: mở handle với đúng hai quyền cần thiết: đọc thông tin
    // (để re-query working set giữa các bước) và set quota (bắt buộc
    // để gọi SetProcessWorkingSetSizeEx).
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA | PROCESS_VM_READ,
            false,
            process.pid,
        )
    }
    .map_err(|source| RobloxProcessError::OpenProcessFailed {
        pid: process.pid,
        source,
    })?;

    let mut outcome = TrimOutcome::default();

    let trim_result = (|| -> Result<(), RobloxProcessError> {
        while outcome.bytes_trimmed < TRIM_TOTAL_TARGET_BYTES {
            // Tái dùng `handle` đã mở sẵn ở trên thay vì mở handle mới
            // mỗi bước — giảm số syscall OpenProcess/CloseHandle từ 2 lần
            // xuống 0 lần mỗi bước trim (tiết kiệm đáng kể qua 16 bước).
            let current_ws = query_working_set_size_with_handle(handle, process.pid)
                .unwrap_or(process.working_set_bytes);

            let remaining_target = TRIM_TOTAL_TARGET_BYTES - outcome.bytes_trimmed;
            let step = TRIM_STEP_BYTES.min(remaining_target);

            // Không cho phép trim xuống dưới sàn an toàn — tránh
            // page-fault thrashing nếu Roblox thực sự cần lượng RAM đó.
            if current_ws <= MIN_WORKING_SET_FLOOR_BYTES {
                outcome.hit_floor = true;
                break;
            }

            let new_target = current_ws.saturating_sub(step).max(MIN_WORKING_SET_FLOOR_BYTES);
            let actual_step = current_ws - new_target;

            if actual_step == 0 {
                outcome.hit_floor = true;
                break;
            }

            // SAFETY: `handle` hợp lệ, mở với PROCESS_SET_QUOTA ở trên.
            // flags=0 (tương đương SetProcessWorkingSetSize cũ): không
            // ép cứng giới hạn min/max, để Windows tự quản lý linh hoạt
            // sau khi trim — tránh khóa cứng working set có thể gây hại
            // nếu Roblox thực sự cần nhiều RAM hơn ngay sau đó.
            unsafe {
                SetProcessWorkingSetSizeEx(
                    handle,
                    new_target as usize,
                    new_target as usize,
                    SETPROCESSWORKINGSETSIZEEX_FLAGS(0),
                )
            }
            .map_err(|source| RobloxProcessError::TrimFailed {
                pid: process.pid,
                source,
            })?;

            outcome.bytes_trimmed += actual_step;
            outcome.steps_completed += 1;
            on_step(outcome.bytes_trimmed);

            if outcome.bytes_trimmed < TRIM_TOTAL_TARGET_BYTES {
                thread::sleep(TRIM_STEP_INTERVAL);
            }
        }
        Ok(())
    })();

    // Bước chống phân mảnh: sau khi đã trim xong theo từng bước nhỏ
    // (kiểm soát chính xác dung lượng theo yêu cầu), gọi EmptyWorkingSet
    // một lần dứt khoát để Windows dồn lại các trang còn resident thành
    // một working set gọn hơn — đây là API chuyên dụng cho việc này
    // (chính là API mà nút "Trim Working Set" của Task Manager dùng),
    // hiệu quả hơn việc chỉ dựa vào SetProcessWorkingSetSizeEx nhiều lần.
    //
    // Chỉ chạy bước này nếu vòng lặp trim ở trên không lỗi — nếu trim
    // chính đã thất bại thì không có ý nghĩa để "dọn gọn" thêm.
    if trim_result.is_ok() {
        // SAFETY: `handle` vẫn còn hợp lệ tại đây (chưa bị đóng ở dưới),
        // đã mở với đủ quyền PROCESS_QUERY_INFORMATION + PROCESS_SET_QUOTA
        // theo đúng yêu cầu của EmptyWorkingSet.
        outcome.defragmented = unsafe { EmptyWorkingSet(handle) }.is_ok();

        if !outcome.defragmented {
            // Không coi đây là lỗi nghiêm trọng: phần trim chính (kiểm
            // soát được dung lượng) đã thành công, bước dồn gọn thêm chỉ
            // là tối ưu bổ sung — thất bại ở đây không nên phá hỏng kết
            // quả trim đã đạt được.
            log::warn!(
                "EmptyWorkingSet (bước chống phân mảnh) thất bại cho PID {}, bỏ qua.",
                process.pid
            );
        }
    }

    // SAFETY: đóng handle bất kể trim thành công hay lỗi giữa chừng,
    // tránh leak handle nếu vòng lặp trên gặp lỗi và return sớm.
    unsafe {
        let _ = CloseHandle(handle);
    }

    trim_result?;
    Ok(outcome)
}