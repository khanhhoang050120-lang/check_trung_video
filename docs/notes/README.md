# Sổ tay kỹ thuật của dự án

Thư mục này ghi lại những gì đã học được trong quá trình xây dựng, để không lặp lại cùng một lỗi. Đây **không** phải tài liệu người dùng và **không** phải bản đặc tả.

## Quy tắc ghi chép

Ghi vào đây ngay khi xảy ra, không để đến cuối phiên làm việc. Mỗi mục cần trả lời được ba câu: chuyện gì đã xảy ra, vì sao, và lần sau làm khác đi thế nào.

Mỗi mục có một mã định danh ổn định để chỗ khác tham chiếu tới, ví dụ `BUG-003`. Không sửa lại mã cũ, không tái sử dụng mã của mục đã xóa.

## Các file

| File | Ghi cái gì | Khi nào ghi |
| :--- | :--- | :--- |
| [BUGS.md](BUGS.md) | Lỗi đã gặp và đã sửa, kèm nguyên nhân gốc | Ngay sau khi sửa xong một lỗi mất hơn vài phút để hiểu |
| [ISSUES.md](ISSUES.md) | Việc còn dang dở, món nợ kỹ thuật, chỗ tạm bợ | Khi cố ý để lại một chỗ chưa hoàn chỉnh |
| [RISKS.md](RISKS.md) | Rủi ro đã biết chưa xử lý hết, kèm cách giảm thiểu | Khi nhận ra một kịch bản xấu chưa được chặn |
| [CONFIG.md](CONFIG.md) | Xung đột cấu hình, phiên bản thư viện, môi trường build | Khi mất thời gian vì một thiết lập nào đó |
| [DECISIONS.md](DECISIONS.md) | Quyết định kiến trúc: chọn gì, vì sao, đã loại phương án nào | Khi chọn một trong nhiều đường đi |
| [PERF.md](PERF.md) | Số đo thật, kết quả benchmark, giả định về hiệu năng | Khi đo được một con số hoặc phát hiện giả định sai |
| [CHECKLIST.md](CHECKLIST.md) | Việc phải làm trước khi commit, trước khi chuyển phase, trước khi phát hành | Khi phát hiện một bước hay bị quên |
| [SPEC-NOTES.md](SPEC-NOTES.md) | Chỗ spec mơ hồ, sai, hoặc lệch với code thực tế | Khi code không khớp spec |

## Cách dùng khi bắt đầu một phiên làm việc

Đọc [CHECKLIST.md](CHECKLIST.md) và [ISSUES.md](ISSUES.md) trước khi viết dòng code đầu tiên. Đọc [BUGS.md](BUGS.md) khi sắp đụng vào một vùng đã từng có lỗi.
