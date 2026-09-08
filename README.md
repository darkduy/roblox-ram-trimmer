# Roblox RAM Trimmer

App nền chạy trên system tray (Windows), tự động trim **512MB** working
set của tiến trình Roblox đang chiếm nhiều RAM nhất khi **RAM toàn hệ
thống** vượt **85%**. Việc trim được chia thành nhiều bước nhỏ (32MB/bước,
cách nhau 1.5s) để tránh gây spike CPU/I/O làm giật lag trong lúc chơi.

## Yêu cầu hệ thống

- **Windows 10/11** (dùng WinAPI: `GlobalMemoryStatusEx`,
  `SetProcessWorkingSetSizeEx`, `Shell_NotifyIcon` — không chạy được trên
  Linux/macOS).
- **Rust 1.98.0** trở lên, edition 2024. Cài qua [rustup](https://rustup.rs).
- Roblox (`RobloxPlayerBeta.exe` hoặc `RobloxStudioBeta.exe`) và app này
  nên chạy **cùng một user session** — `OpenProcess` với quyền
  `PROCESS_SET_QUOTA` sẽ thất bại nếu Roblox chạy dưới session/quyền khác
  (ví dụ chạy app này as Administrator trong khi Roblox chạy user thường,
  hoặc ngược lại, đôi khi cũng gây lỗi quyền — khuyến nghị chạy cả hai ở
  cùng mức quyền, không cần Admin).

## Build

```powershell
cd roblox-ram-trimmer
cargo build --release
```

Binary xuất ra tại `target\release\roblox-ram-trimmer.exe`. Đây là file
**độc lập** — icon tray được sinh bằng code, không cần file `.ico` đi kèm.

Bản release **ẩn cửa sổ console** (chỉ hiện tray icon). Muốn xem log trực
tiếp khi phát triển, chạy bản debug:

```powershell
cargo run
```

## Build tự động bằng GitHub Actions

Workflow có sẵn tại `.github/workflows/build.yml`, build trên runner
`windows-latest` (bắt buộc — code dùng WinAPI, không cross-compile được
từ Linux). Đẩy code lên `darkduy/roblox-ram-trimmer` là dùng được ngay,
không cần cấu hình gì thêm (không cần secret nào ngoài `GITHUB_TOKEN` có
sẵn mặc định).

**Khi nào workflow chạy:**
- Push lên nhánh `main` → build để kiểm tra compile OK, kết quả nằm ở
  tab **Actions** dưới dạng artifact (không tạo release).
- Bấm nút **Run workflow** thủ công trên tab Actions → build theo yêu cầu.
- Push một **tag** dạng `v*` (ví dụ `v0.1.0`) → build **và** tự động tạo
  GitHub Release đính kèm file `.exe`.

**Cách tải binary sau khi build xong:**
- Build từ push/workflow_dispatch: vào tab **Actions** → chọn lần chạy →
  tải artifact `roblox-ram-trimmer-windows-x64` (file `.zip` chứa `.exe`).
- Build từ tag: vào tab **Releases** → tải thẳng file `.exe` đính kèm.

**Cách phát hành một bản release mới:**
```powershell
git tag v0.1.0
git push origin v0.1.0
```

Lần build đầu tiên trên CI sẽ tự sinh `Cargo.lock` (chưa có sẵn vì môi
trường phát triển ban đầu không cross-compile được sang Windows để tạo
file này). Sau lần đó, nhớ `git add Cargo.lock && git commit` để các lần
build sau tái lập được chính xác cùng phiên bản dependency.

## Chạy

Double-click `roblox-ram-trimmer.exe`, hoặc đặt shortcut vào thư mục
Startup (`shell:startup`) để tự chạy cùng Windows.

- Icon tròn xanh sẽ xuất hiện ở khay hệ thống (system tray).
- Click phải (hoặc trái, tùy hệ thống) vào icon để xem menu: trạng thái
  hiện tại, % RAM hệ thống, lần trim gần nhất, và nút **Thoát**.
- Log chi tiết được ghi vào:
  `%LOCALAPPDATA%\roblox-ram-trimmer.log`

## Cách hoạt động

1. Mỗi 3 giây, app đọc % RAM hệ thống qua `GlobalMemoryStatusEx`.
2. Khi RAM ≥ 85%, app tìm tiến trình Roblox đang chiếm nhiều RAM nhất.
3. App trim working set của tiến trình đó theo từng bước 32MB, nghỉ 1.5s
   giữa mỗi bước, cho tới khi đạt tổng 512MB hoặc chạm sàn an toàn 128MB
   (không bao giờ trim Roblox xuống dưới mức này).
4. Sau khi hoàn tất đợt trim (dù đủ 512MB hay dừng sớm do chạm sàn), app
   khóa (cooldown) **60 giây** trước khi cho phép trim lại — kể cả khi
   RAM vẫn còn trên 85%.

## Tinh chỉnh cấu hình

Mọi hằng số điều chỉnh hành vi nằm trong `src/config.rs`:

| Hằng số | Mặc định | Ý nghĩa |
|---|---|---|
| `RAM_THRESHOLD_PERCENT` | 85.0 | Ngưỡng % RAM hệ thống để kích hoạt trim |
| `TRIM_TOTAL_TARGET_BYTES` | 512 MB | Tổng dung lượng trim mỗi đợt |
| `TRIM_STEP_BYTES` | 32 MB | Kích thước mỗi bước trim nhỏ |
| `TRIM_STEP_INTERVAL` | 1.5s | Thời gian nghỉ giữa các bước trim |
| `RAM_CHECK_INTERVAL` | 3s | Tần suất kiểm tra RAM hệ thống |
| `COOLDOWN_AFTER_TRIM` | 60s | Thời gian khóa sau một đợt trim |
| `MIN_WORKING_SET_FLOOR_BYTES` | 128 MB | Sàn an toàn, không trim thấp hơn |

Sửa xong nhớ `cargo build --release` lại.

## Giới hạn kỹ thuật cần biết

`SetProcessWorkingSetSizeEx` chỉ **ra lệnh** cho Windows đẩy các trang bộ
nhớ ít dùng ra khỏi working set của tiến trình — đây là hành vi
**best-effort**, không phải một cam kết cứng:

- Con số "512MB" là **mục tiêu**, không phải kết quả đảm bảo. Nếu Roblox
  đang hoạt động và chạm lại các trang đó ngay sau khi trim, working set
  có thể tăng trở lại gần như ngay lập tức.
- Đây là giới hạn cố hữu của kỹ thuật working-set trimming trên Windows,
  không phải bug của app.
- Trim quá sâu hoặc quá thường xuyên có thể gây tăng page-fault khi
  Roblox cần lại trang đã bị đẩy ra — đây là lý do app chia nhỏ thành
  từng bước 32MB kèm nghỉ giữa các bước, và khóa cooldown 60s sau mỗi đợt.

## Cấu trúc mã nguồn

```
src/
├── main.rs             — entry point, nối các module
├── config.rs           — hằng số cấu hình tập trung
├── sysmem.rs            — đọc % RAM hệ thống (GlobalMemoryStatusEx)
├── roblox_process.rs    — tìm & trim tiến trình Roblox
├── app_state.rs         — trạng thái dùng chung giữa các thread
├── monitor.rs           — vòng lặp giám sát + state machine cooldown
└── tray_ui.rs           — giao diện system tray
```

## Đề xuất cải tiến tiếp theo

- **Icon động theo trạng thái**: đổi màu icon tray (xanh/vàng/xám) tương
  ứng Monitoring/Trimming/Cooldown thay vì chỉ đổi tooltip — dễ nhận biết
  hơn khi liếc mắt.
- **Toast notification**: dùng `Win32_UI_Shell` (`Shell_NotifyIcon` với
  `NIF_INFO`) để hiện thông báo balloon khi trim xong, thay vì phải mở
  menu mới thấy.
- **Cấu hình qua file**: đọc các hằng số từ một file `config.toml` cạnh
  `.exe` thay vì hard-code, để chỉnh ngưỡng/thời gian mà không cần
  build lại.
- **Retry với backoff khi OpenProcess thất bại do quyền**: hiện tại lỗi
  quyền chỉ được log và báo lên tray; có thể thêm gợi ý cụ thể ("thử chạy
  app này với quyền Administrator") ngay trong tray menu khi phát hiện
  lỗi `ERROR_ACCESS_DENIED`.
- **Unit test cho `roblox_process.rs`**: hiện chỉ test được phần logic
  thuần (đã verify kỹ trong quá trình phát triển); phần gọi WinAPI thật
  cần test tích hợp chạy trên Windows CI, hoặc trừu tượng hóa qua trait
  để mock được trong unit test.
