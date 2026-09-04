//! Phần thuần của các phép đi bộ thư viện (spec 5.10).
//!
//! Bốn phép quét — thêm vào hàng đợi (pha A của initial scan), delta reconcile theo
//! `ctime`, presence scan và quét lại root remote — dùng chung một vòng đi bộ; mỗi
//! phép chỉ khác nhau ở việc làm gì với một entry. Ở đây là phần "làm gì": quyết
//! định thuần trên `Identity` và `Repository`, không có `readdir`.
//!
//! Vòng đi bộ thật (`readdir`, `statx`, ranh giới mount, nhịp thư mục) nằm ở
//! `nasdedup-linux`, nên phần quyết định vẫn test được đầy đủ trên Windows.
