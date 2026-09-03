//! `nasdedup-linux` — tầng syscall Linux (spec 3.2, Phase 3 và 5).

#![cfg(target_os = "linux")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
