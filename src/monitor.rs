//! Vòng lặp giám sát chính, chạy trên một thread nền riêng biệt với UI
//! thread của tray icon.
//!
//! Trách nhiệm duy nhất của module này: định kỳ đọc % RAM hệ thống, và
//! khi vượt ngưỡng, kích hoạt một đợt trim theo từng bước nhỏ — sau đó
//! khóa (cooldown) trước khi cho phép trim lại.

use crate::app_state::{AppStateHandle, AppStatus};
use crate::config::{COOLDOWN_AFTER_TRIM, RAM_CHECK_INTERVAL, RAM_THRESHOLD_PERCENT, TRIM_TOTAL_TARGET_BYTES};
use crate::roblox_process::{self, RobloxProcessError};
use crate::sysmem;
use std::thread;
use std::time::Instant;

/// Trạng thái nội bộ của vòng lặp giám sát — không cần expose ra ngoài,
/// chỉ `monitor.rs` quan tâm tới chi tiết máy trạng thái (state machine) này.
enum MonitorPhase {
    /// Đang theo dõi bình thường, sẵn sàng trim khi cần.
    Watching,
    /// Vừa hoàn tất một đợt trim tại thời điểm `since`, đang chờ hết cooldown.
    CoolingDown { since: Instant },
}

/// Kết quả của một lần thử trim, quyết định phase tiếp theo của state machine.
enum TrimCycleResult {
    /// Trim thành công (dù đủ 512MB hay dừng sớm do chạm sàn) — vào cooldown.
    Trimmed,
    /// Không trim được gì (không tìm thấy Roblox, hoặc lỗi WinAPI) — quay
    /// lại Watching ngay để thử lại ở lần check tiếp theo, thay vì lãng
    /// phí 60 giây cooldown cho một đợt trim chưa từng xảy ra.
    NotAttempted,
}

/// Chạy vòng lặp giám sát vô hạn. Hàm này **block** thread hiện tại —
/// luôn luôn gọi trên một thread nền riêng (`std::thread::spawn`), không
/// bao giờ gọi trực tiếp trên UI thread của tray.
pub fn run_monitor_loop(state: AppStateHandle) -> ! {
    let mut phase = MonitorPhase::Watching;

    loop {
        thread::sleep(RAM_CHECK_INTERVAL);

        let ram_status = match sysmem::query_system_memory() {
            Ok(status) => status,
            Err(err) => {
                log::warn!("Không đọc được RAM hệ thống: {err}");
                state.set_status(AppStatus::Error {
                    message: "Không đọc được RAM hệ thống".to_string(),
                });
                continue;
            }
        };

        let ram_percent = ram_status.memory_load_percent as f32;
        state.set_ram_percent(ram_percent);

        phase = match phase {
            MonitorPhase::CoolingDown { since } => {
                let elapsed = since.elapsed();
                if elapsed >= COOLDOWN_AFTER_TRIM {
                    log::info!("Hết thời gian cooldown, tiếp tục theo dõi RAM.");
                    state.set_status(AppStatus::Monitoring);
                    MonitorPhase::Watching
                } else {
                    let remaining = COOLDOWN_AFTER_TRIM - elapsed;
                    state.set_status(AppStatus::Cooldown {
                        remaining_secs: remaining.as_secs(),
                    });
                    MonitorPhase::CoolingDown { since }
                }
            }
            MonitorPhase::Watching => {
                if ram_percent >= RAM_THRESHOLD_PERCENT {
                    log::info!(
                        "RAM hệ thống đạt {ram_percent:.1}% (>= ngưỡng {RAM_THRESHOLD_PERCENT}%), bắt đầu trim."
                    );
                    match perform_trim_cycle(&state) {
                        TrimCycleResult::Trimmed => MonitorPhase::CoolingDown {
                            since: Instant::now(),
                        },
                        // Không lãng phí cooldown cho một đợt trim chưa
                        // từng xảy ra — thử lại ngay ở lần check tiếp theo.
                        TrimCycleResult::NotAttempted => MonitorPhase::Watching,
                    }
                } else {
                    state.set_status(AppStatus::Monitoring);
                    MonitorPhase::Watching
                }
            }
        };
    }
}

/// Thực hiện một đợt trim đầy đủ (tối đa `TRIM_TOTAL_TARGET_BYTES`) trên
/// tiến trình Roblox đang chiếm nhiều RAM nhất, cập nhật tiến độ theo
/// thời gian thực vào `state`.
///
/// Trả về `NotAttempted` nếu không có gì để trim (không tìm thấy tiến
/// trình, hoặc lỗi trước khi kịp trim) — báo hiệu cho caller không nên
/// tính đây là một đợt trim thật sự (không vào cooldown).
fn perform_trim_cycle(state: &AppStateHandle) -> TrimCycleResult {
    let process = match roblox_process::find_largest_roblox_process() {
        Ok(p) => p,
        Err(RobloxProcessError::NotFound) => {
            log::info!("RAM vượt ngưỡng nhưng không tìm thấy tiến trình Roblox nào đang chạy.");
            state.set_status(AppStatus::Error {
                message: "Không tìm thấy tiến trình Roblox".to_string(),
            });
            return TrimCycleResult::NotAttempted;
        }
        Err(err) => {
            log::error!("Lỗi khi tìm tiến trình Roblox: {err}");
            state.set_status(AppStatus::Error {
                message: "Lỗi khi tìm tiến trình Roblox".to_string(),
            });
            return TrimCycleResult::NotAttempted;
        }
    };

    log::info!(
        "Tiến trình mục tiêu: {} (PID {}), working set hiện tại: {:.1} MB",
        process.name,
        process.pid,
        process.working_set_bytes as f64 / 1024.0 / 1024.0
    );

    let state_for_callback = state.clone();
    let trim_result = roblox_process::trim_process_gradually(&process, move |bytes_trimmed| {
        let progress = ((bytes_trimmed as f64 / TRIM_TOTAL_TARGET_BYTES as f64) * 100.0) as u8;
        state_for_callback.set_status(AppStatus::Trimming {
            progress_percent: progress.min(100),
        });
    });

    match trim_result {
        Ok(outcome) => {
            log::info!(
                "Trim hoàn tất: {:.1} MB trong {} bước (chạm sàn an toàn: {}, đã dồn gọn chống phân mảnh: {}).",
                outcome.bytes_trimmed as f64 / 1024.0 / 1024.0,
                outcome.steps_completed,
                outcome.hit_floor,
                outcome.defragmented
            );
            state.record_trim(process.name.clone(), outcome.bytes_trimmed);
            state.set_status(AppStatus::Cooldown {
                remaining_secs: COOLDOWN_AFTER_TRIM.as_secs(),
            });
            TrimCycleResult::Trimmed
        }
        Err(err) => {
            log::error!("Trim thất bại: {err}");
            state.set_status(AppStatus::Error {
                message: "Trim thất bại".to_string(),
            });
            // Trim đã thử nhưng lỗi giữa chừng do vấn đề WinAPI/quyền —
            // đây là lỗi có khả năng lặp lại ngay, nên vẫn cho vào cooldown
            // để tránh vòng lặp retry dồn dập gây tốn CPU nếu lỗi dai dẳng
            // (ví dụ do thiếu quyền PROCESS_SET_QUOTA — sẽ không tự hết).
            TrimCycleResult::Trimmed
        }
    }
}