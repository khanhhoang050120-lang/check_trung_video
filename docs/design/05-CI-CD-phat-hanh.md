# Pipeline CI/CD và quy trình phát hành

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

Thiết kế CI/CD cho `nasdedup` sau khi thêm app desktop Tauri v2: 5 workflow (`ci.yml`, `release.yml`, `prepare-release.yml`, `tag-on-merge.yml`, `kernel-matrix.yml`) đã viết đầy đủ và kiểm tra cú pháp YAML bằng parser. CI có 13 job song song, lọc theo đường dẫn thay đổi, bổ sung cargo-deny (advisory + license + bans), coverage có ngưỡng, integration test Btrfs/XFS trên loop image chạy hai lần (kernel hiện đại và chế độ ép fallback syscall cũ), lint frontend, và ba guard thực thi trực tiếp nguyên tắc của spec: chống God Component (file > 400 dòng), đồng bộ version ba nơi, danh sách test bắt buộc không được xóa hay đổi tên. CD kích hoạt bằng tag với job `gate` chặn phát hành nếu tag không nằm trên main, version lệch, CI của commit đó không xanh, hoặc thiếu bằng chứng smoke test trên NAS thật; sau đó build ma trận (musl x86_64/aarch64 + NSIS Windows), ký updater bằng minisign của Tauri, ký checksum bằng cosign keyless + attestation GitHub, tạo Release draft rồi tự kiểm tra lại trước khi công khai. Toàn hệ thống chỉ tồn tại một private key thật (khóa updater Tauri); mọi chữ ký khác dùng OIDC keyless nên không có gì để lộ. Kernel 4.4 của DSM được phủ bằng ba tầng: ép fallback trong CI mỗi PR, QEMU/virtme-ng kernel 4.19–6.1 hằng đêm, và smoke test bắt buộc trên NAS thật trước mỗi release.

## 1. Bản đồ workspace và workflow

### 1.1 Cấu trúc repo sau khi thêm UI

```text
nasdedup/
├── Cargo.toml                workspace daemon; thêm exclude = ["apps/desktop/src-tauri"]
├── crates/
│   ├── core/ db/ linux/ daemon/
│   └── proto/                MỚI: nasdedup-proto — kiểu request/response control API,
│                             PROTOCOL_VERSION, pairing code. Không phụ thuộc OS.
├── apps/desktop/             app Tauri v2 (workspace Rust RIÊNG, lock riêng)
│   ├── package.json  pnpm-lock.yaml  vite.config.ts  eslint.config.js
│   ├── locales/vi.json       CHỈ MỘT locale
│   ├── src/                  frontend (component nhỏ, mỗi file ≤ 400 dòng)
│   └── src-tauri/            Cargo.toml (deps: tauri 2, nasdedup-proto path), tauri.conf.json, deny.toml
├── ci/                       guard + script CI/CD (bash, chạy được cả trên máy dev)
├── packaging/                nasdedup.service, install.sh
├── release-evidence/         bằng chứng smoke test NAS thật, commit theo tag
├── deny.toml  cliff.toml  .config/nextest.toml  CHANGELOG.md
└── .github/workflows/        ci.yml release.yml prepare-release.yml tag-on-merge.yml kernel-matrix.yml
```

### 1.2 Năm workflow và điều kiện kích hoạt

| Workflow | Kích hoạt | Việc chính | Chặn merge/release |
| :--- | :--- | :--- | :--- |
| `ci.yml` | push `main`, mọi PR | 13 job; gom lại thành một check tên **`CI OK`** | Có — required check duy nhất |
| `kernel-matrix.yml` | cron 18:00 UTC (01:00 VN), dispatch, PR có label `kernel-matrix` | QEMU kernel 4.19/5.4/5.10/6.1 + test aarch64 dưới qemu-user | Không (thông tin; hồi quy → mở issue) |
| `prepare-release.yml` | `workflow_dispatch` (maintainer chọn auto/patch/minor/major) | bump version 1 nơi, sync 3 nơi, sinh CHANGELOG tiếng Việt, mở PR `chore(release): vX.Y.Z` | — |
| `tag-on-merge.yml` | push `main` với commit `chore(release): v…` | tạo tag `vX.Y.Z`, gọi `release.yml` | — |
| `release.yml` | push tag `v*`, hoặc dispatch với input `tag` | gate → build ma trận → ký → Release → tự verify → công khai | Có — gate là cổng cuối |

### 1.3 Vì sao `src-tauri` là workspace riêng

`cargo test --workspace` của daemon phải chạy nhanh trên cả Windows lẫn Linux và không được kéo theo GTK/WebKit, tokio, hàng trăm crate của Tauri. Tách workspace giữ nguyên `[workspace.lints] unwrap_used/panic = deny` cho daemon (Tauri sinh code có `unwrap`), giữ MSRV 1.85 cho daemon độc lập với MSRV của Tauri, và cho phép `deny.toml` của daemon cấm `tokio` (spec 3.1: kiến trúc đồng bộ) trong khi app desktop vẫn dùng tokio. Chia sẻ kiểu qua path dependency `nasdedup-proto` — không cần publish crates.io.

## 2. Pipeline CI: job, cache, thời gian

### 2.1 Danh sách job

| Job | Runner | Chạy khi | Nội dung | Nguội | Ấm |
| :--- | :--- | :--- | :--- | ---: | ---: |
| `changes` | ubuntu | luôn | `dorny/paths-filter` → 4 cờ `rust/linux/desktop/frontend` | 15 s | 15 s |
| `guards` | ubuntu | luôn | file > 400 dòng; version 3 nơi; i18n chỉ `vi`; đổi proto phải bump `PROTOCOL_VERSION` | 40 s | 40 s |
| `lint` | ubuntu | rust\|desktop | `fmt` (2 workspace) + `clippy --all-targets -D warnings` + `cargo doc` + `cargo machete` | 9 m | 3 m |
| `test-linux` | ubuntu | rust | `cargo nextest run --workspace` + doctest | 8 m | 4 m |
| `test-windows` | windows | rust | `nextest` core+db+proto, clippy Windows (NFR-5) | 12 m | 6 m |
| `msrv` | ubuntu | rust | `cargo check` với toolchain 1.85.0 | 6 m | 2 m |
| `supply-chain` | ubuntu | luôn | `cargo deny check` × 2 workspace + `pnpm audit` | 1 m 30 | 50 s |
| `coverage` | ubuntu | rust | `llvm-cov nextest -p core -p db --fail-under-lines 80` | 14 m | 7 m |
| `fs-integration` × 2 | ubuntu | linux | Btrfs ×2 + XFS reflink loop image; matrix `modern`/`legacy` | 11 m | 6 m |
| `build-musl` × 2 | ubuntu | rust | `cross build` x86_64 + aarch64, kiểm tra thực sự tĩnh | 9 m | 5 m |
| `frontend` | ubuntu | frontend\|desktop | tsc, eslint, prettier, vitest, build | 3 m | 1 m 30 |
| `desktop-windows` | windows | desktop\|frontend | clippy + test `src-tauri`, `tauri build --no-bundle` | 14 m | 7 m |
| `ci-ok` | ubuntu | luôn (`if: always()`) | gom kết quả → **check duy nhất bắt buộc** | 10 s | 10 s |

**Wall clock:** ~15 phút nguội, ~8 phút ấm (PR chỉ chạm frontend: ~3 phút vì các job Rust bị `changes` lọc bỏ).

### 2.2 Chiến lược cache

| Cache | Cách làm | Ghi chú |
| :--- | :--- | :--- |
| Rust deps + target | `Swatinem/rust-cache@v2` với `shared-key` riêng **theo lane** (`linux-lint`, `linux-test`, `windows-test`, `msrv`, `coverage`, `musl-<target>`, `tauri-windows`) | Trộn chung một key giữa clippy/test/coverage làm cache đập nhau vì khác flag; tách ra ăn cache ~85 % |
| `fs-integration` | dùng lại `shared-key: linux-test` với `save-if: "false"` | Chỉ đọc, không ghi đè cache của job test |
| `src-tauri` | `workspaces: apps/desktop/src-tauri -> target` | Lock riêng nên không đụng cache daemon |
| pnpm store | `actions/setup-node@v4` với `cache: pnpm`, `cache-dependency-path` trỏ đúng lockfile | |
| Công cụ Rust | **`taiki-e/install-action@v2`** thay cho `cargo install` | `cargo install cross --locked` tốn 2–3 phút mỗi job; install-action tải binary sẵn ~10 s. Đây là mức cải thiện lớn nhất so với `ci.yml` hiện tại |
| Docker image của `cross` | không cache (pull ~40 s) | Cache docker layer đắt hơn pull |

### 2.3 Bốn thay đổi so với `ci.yml` hiện tại (ngoài việc thêm job)

1. **Bỏ `env: RUSTFLAGS: -D warnings` ở cấp workflow.** `RUSTFLAGS` áp lên cả dependency, nên một bản `serde` mới có deprecation warning sẽ làm đỏ CI dù code ta không đổi; và nó làm `rust-cache` mất hiệu lực khi biến đổi. Warning đã được chặn đúng chỗ: `[workspace.lints]` trong `Cargo.toml` + `cargo clippy -- -D warnings` + `RUSTDOCFLAGS` chỉ ở bước `cargo doc`.
2. **`--locked` ở mọi lệnh cargo.** Không có nó, `Cargo.lock` có thể bị cập nhật ngầm trên runner và CI test một tập dependency khác với cái sẽ được release.
3. **`cargo-nextest` thay `cargo test`** cho phần integration: có `--no-tests=fail` (test bị đổi tên/biến mất → job đỏ thay vì xanh giả), `--run-ignored all`, timeout mỗi test, và JUnit.
4. **`ci-ok` là required check duy nhất.** Khi `changes` lọc bỏ một job, GitHub coi required check bị `skipped` là *pending* và khoá merge vĩnh viễn; job tổng hợp `if: always()` giải quyết đúng vấn đề này.

## 3. `.github/workflows/ci.yml` (đầy đủ, đã kiểm tra cú pháp)

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: "0"
  CARGO_NET_RETRY: "10"
  RUSTUP_MAX_RETRIES: "10"
  RUST_BACKTRACE: "1"
  # KHÔNG đặt RUSTFLAGS ở đây: nó áp cả lên dependency và phá cache (mục 2.3).

jobs:
  changes:
    name: Phạm vi thay đổi
    runs-on: ubuntu-24.04
    outputs:
      rust: ${{ steps.f.outputs.rust }}
      linux: ${{ steps.f.outputs.linux }}
      desktop: ${{ steps.f.outputs.desktop }}
      frontend: ${{ steps.f.outputs.frontend }}
    steps:
      - uses: actions/checkout@v4
      - uses: dorny/paths-filter@v3
        id: f
        with:
          filters: |
            rust:
              - 'crates/**'
              - 'tests/**'
              - 'Cargo.toml'
              - 'Cargo.lock'
              - 'rustfmt.toml'
              - 'deny.toml'
              - '.config/nextest.toml'
              - '.github/workflows/ci.yml'
            linux:
              - 'crates/core/**'
              - 'crates/linux/**'
              - 'crates/db/**'
              - 'tests/**'
              - 'ci/**'
              - '.github/workflows/ci.yml'
            desktop:
              - 'apps/desktop/**'
              - 'crates/proto/**'
              - '.github/workflows/ci.yml'
            frontend:
              - 'apps/desktop/src/**'
              - 'apps/desktop/package.json'
              - 'apps/desktop/pnpm-lock.yaml'
              - 'apps/desktop/locales/**'
              - '.github/workflows/ci.yml'

  guards:
    name: Guard (God Component, version, i18n, proto)
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Chống God Component — file > 400 dòng (spec 3.2, NFR-6)
        run: bash ci/guard-file-size.sh
      - name: Version đồng bộ Cargo / tauri.conf.json / package.json
        run: bash ci/guard-version-sync.sh
      - name: Giao diện chỉ tiếng Việt
        run: bash ci/guard-i18n.sh
      - name: Đổi proto phải bump PROTOCOL_VERSION
        if: github.event_name == 'pull_request'
        run: bash ci/guard-proto-version.sh "${{ github.event.pull_request.base.sha }}"

  lint:
    name: fmt + clippy + doc
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.rust == 'true' || needs.changes.outputs.desktop == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: linux-lint
          cache-on-failure: true
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-machete
      - run: cargo fmt --all -- --check
      - if: needs.changes.outputs.desktop == 'true'
        run: cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - env:
          RUSTDOCFLAGS: -D warnings
        run: cargo doc --workspace --no-deps --locked
      - run: cargo machete

  test-linux:
    name: Test Linux (workspace)
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: linux-test
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-nextest
      - run: cargo nextest run --workspace --locked --profile ci --no-tests=fail
      - run: cargo test --workspace --locked --doc

  test-windows:
    name: Test Windows (core, db, proto) — NFR-5
    runs-on: windows-2022
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: windows-test
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-nextest
      - run: cargo nextest run -p nasdedup-core -p nasdedup-db -p nasdedup-proto --locked --profile ci --no-tests=fail
      - run: cargo clippy -p nasdedup-core -p nasdedup-db -p nasdedup-proto --all-targets --locked -- -D warnings

  msrv:
    name: MSRV 1.85
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.85.0
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: msrv
      - run: cargo check --workspace --all-targets --locked

  supply-chain:
    name: cargo-deny + pnpm audit
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-deny
      - name: Daemon workspace (advisories, licenses, bans, sources)
        run: cargo deny --all-features check advisories bans licenses sources
      - name: src-tauri
        run: >
          cargo deny --manifest-path apps/desktop/src-tauri/Cargo.toml
          --config apps/desktop/src-tauri/deny.toml
          check advisories bans licenses sources
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - working-directory: apps/desktop
        run: |
          pnpm install --frozen-lockfile --ignore-scripts
          pnpm audit --audit-level high

  coverage:
    name: Coverage core + db
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: llvm-tools-preview
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: coverage
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-llvm-cov,cargo-nextest
      - name: Ngưỡng bắt buộc cho logic thuần
        run: |
          cargo llvm-cov nextest --locked \
            -p nasdedup-core -p nasdedup-db \
            --lcov --output-path lcov.info \
            --fail-under-lines 80
      - continue-on-error: true
        run: cargo llvm-cov nextest --workspace --locked --summary-only
      - uses: actions/upload-artifact@v4
        with:
          name: coverage-lcov
          path: lcov.info

  fs-integration:
    name: Btrfs/XFS loop image (${{ matrix.syscall_mode }})
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.linux == 'true'
    strategy:
      fail-fast: false
      matrix:
        syscall_mode: [modern, legacy]
    env:
      NASDEDUP_IT_MOUNT: /mnt/nd-btrfs
      NASDEDUP_IT_MOUNT2: /mnt/nd-btrfs2
      NASDEDUP_IT_XFS: /mnt/nd-xfs
      NASDEDUP_FORCE_LEGACY_SYSCALLS: ${{ matrix.syscall_mode == 'legacy' && '1' || '0' }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: linux-test
          save-if: "false"
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-nextest
      - run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends btrfs-progs xfsprogs
      - name: Tạo loop image (2 Btrfs cho EXDEV + 1 XFS reflink)
        run: |
          set -euxo pipefail
          sudo mkdir -p /mnt/nd-btrfs /mnt/nd-btrfs2 /mnt/nd-xfs
          for n in 1 2; do
            truncate -s 3G "$RUNNER_TEMP/btrfs$n.img"
            mkfs.btrfs -q -f "$RUNNER_TEMP/btrfs$n.img"
          done
          truncate -s 3G "$RUNNER_TEMP/xfs.img"
          mkfs.xfs -q -f -m reflink=1 "$RUNNER_TEMP/xfs.img"
          sudo mount -o loop "$RUNNER_TEMP/btrfs1.img" /mnt/nd-btrfs
          sudo mount -o loop "$RUNNER_TEMP/btrfs2.img" /mnt/nd-btrfs2
          sudo mount -o loop "$RUNNER_TEMP/xfs.img" /mnt/nd-xfs
          sudo btrfs subvolume create /mnt/nd-btrfs/subA
          sudo btrfs subvolume create /mnt/nd-btrfs/subB
          sudo chmod -R 0777 /mnt/nd-btrfs /mnt/nd-btrfs2 /mnt/nd-xfs
          findmnt -no FSTYPE,TARGET /mnt/nd-btrfs /mnt/nd-btrfs2 /mnt/nd-xfs
      - run: cargo nextest run -p nasdedup-linux --locked --no-run
      - name: Test bắt buộc phải tồn tại (chống đổi tên làm mất cổng an toàn)
        run: bash ci/guard-required-tests.sh
      - name: Integration test (root — cần cho mount và F_SETLEASE)
        run: |
          sudo -E env "PATH=$PATH" cargo nextest run -p nasdedup-linux \
            --locked --profile ci --run-ignored all --no-tests=fail
      - name: CỔNG CHỐNG MẤT DỮ LIỆU (spec 1.2 / 10)
        run: |
          sudo -E env "PATH=$PATH" cargo nextest run -p nasdedup-linux \
            --locked --profile ci --run-ignored all --no-tests=fail \
            -E 'test(/differs_outside_sparse_window|dest_metadata_unchanged/)'
      - if: always()
        run: sudo umount /mnt/nd-btrfs /mnt/nd-btrfs2 /mnt/nd-xfs || true

  build-musl:
    name: Build musl ${{ matrix.target }}
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    strategy:
      fail-fast: false
      matrix:
        target: [x86_64-unknown-linux-musl, aarch64-unknown-linux-musl]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: musl-${{ matrix.target }}
      - uses: taiki-e/install-action@v2
        with:
          tool: cross
      - run: cross build --release --locked --target ${{ matrix.target }} --bin nasdedup
      - name: Kiểm tra binary thực sự tĩnh (NFR-4)
        run: |
          f=target/${{ matrix.target }}/release/nasdedup
          file "$f"
          file "$f" | grep -q "statically linked" || { echo "::error::$f không tĩnh"; exit 1; }
      - uses: actions/upload-artifact@v4
        with:
          name: nasdedup-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/nasdedup
          retention-days: 7

  frontend:
    name: Frontend app desktop
    runs-on: ubuntu-24.04
    needs: changes
    if: needs.changes.outputs.frontend == 'true' || needs.changes.outputs.desktop == 'true'
    defaults:
      run:
        working-directory: apps/desktop
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml
      - run: pnpm install --frozen-lockfile
      - run: pnpm exec tsc --noEmit
      - run: pnpm exec eslint . --max-warnings 0
      - run: pnpm exec prettier --check .
      - run: pnpm exec vitest run --coverage
      - run: pnpm build

  desktop-windows:
    name: Build app Windows (Tauri v2)
    runs-on: windows-2022
    needs: changes
    if: needs.changes.outputs.desktop == 'true' || needs.changes.outputs.frontend == 'true'
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: apps/desktop/src-tauri -> target
          shared-key: tauri-windows
      - working-directory: apps/desktop
        run: pnpm install --frozen-lockfile
      - run: cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --locked -- -D warnings
      - run: cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked
      - name: Build app (không đóng gói installer trên PR)
        working-directory: apps/desktop
        run: pnpm exec tauri build --no-bundle

  ci-ok:
    name: CI OK
    if: always()
    needs:
      - guards
      - lint
      - test-linux
      - test-windows
      - msrv
      - supply-chain
      - coverage
      - fs-integration
      - build-musl
      - frontend
      - desktop-windows
    runs-on: ubuntu-24.04
    steps:
      - env:
          NEEDS: ${{ toJSON(needs) }}
        run: |
          echo "$NEEDS" | jq -r 'to_entries[] | "\(.key)=\(.value.result)"'
          bad=$(echo "$NEEDS" | jq -r '[to_entries[] | select(.value.result != "success" and .value.result != "skipped") | .key] | join(", ")')
          if [ -n "$bad" ]; then
            echo "::error::Job thất bại hoặc bị hủy: $bad"
            exit 1
          fi
```

## 4. File hỗ trợ CI (guard, deny, nextest, dependabot)

### `ci/guard-file-size.sh` — thực thi nguyên tắc chống God Component

```bash
#!/usr/bin/env bash
set -euo pipefail
LIMIT=${LIMIT:-400}
fail=0
while IFS= read -r f; do
  [ -f "$f" ] || continue
  n=$(wc -l < "$f")
  if [ "$n" -gt "$LIMIT" ]; then
    echo "::error file=$f,line=1::$f có $n dòng (> $LIMIT). Tách module — spec 3.2 / NFR-6."
    fail=1
  fi
done < <(git ls-files -- '*.rs' '*.ts' '*.tsx' '*.svelte' \
           ':!:apps/desktop/src-tauri/gen/*' ':!:**/*.generated.*')
exit "$fail"
```

### `ci/guard-version-sync.sh` và `ci/sync-version.sh`

```bash
# ci/guard-version-sync.sh
#!/usr/bin/env bash
set -euo pipefail
V=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="nasdedup")|.version')
T=$(jq -r '.version' apps/desktop/src-tauri/tauri.conf.json)
P=$(jq -r '.version' apps/desktop/package.json)
S=$(cargo metadata --manifest-path apps/desktop/src-tauri/Cargo.toml --no-deps --format-version 1 \
     | jq -r '.packages[0].version')
if [ "$V" != "$T" ] || [ "$V" != "$P" ] || [ "$V" != "$S" ]; then
  echo "::error::Version lệch: workspace=$V tauri.conf=$T package.json=$P src-tauri=$S"
  echo "Sửa bằng: bash ci/sync-version.sh $V"
  exit 1
fi
echo "Version thống nhất: $V"
```

```bash
# ci/sync-version.sh <version>  — nguồn sự thật là [workspace.package].version
#!/usr/bin/env bash
set -euo pipefail
V="$1"; t=$(mktemp)
jq --arg v "$V" '.version=$v' apps/desktop/src-tauri/tauri.conf.json > "$t" && mv "$t" apps/desktop/src-tauri/tauri.conf.json
jq --arg v "$V" '.version=$v' apps/desktop/package.json           > "$t" && mv "$t" apps/desktop/package.json
cargo set-version --manifest-path apps/desktop/src-tauri/Cargo.toml "$V"
```

### `ci/guard-required-tests.sh` + `ci/required-tests.txt`

Đây là hàng rào chống "cổng an toàn biến mất trong im lặng": nếu ai đó đổi tên hoặc xóa test chống mất dữ liệu, job đỏ ngay.

```bash
#!/usr/bin/env bash
set -euo pipefail
list=$(cargo nextest list -p nasdedup-linux --run-ignored all --locked)
miss=0
while IFS= read -r t; do
  case "$t" in ''|\#*) continue ;; esac
  printf '%s' "$list" | grep -qF "$t" || { echo "::error::Thiếu test bắt buộc: $t"; miss=1; }
done < ci/required-tests.txt
exit "$miss"
```

```text
# ci/required-tests.txt — KHÔNG xóa, KHÔNG đổi tên. Mỗi dòng ánh xạ tới một kịch bản mục 10.
differs_outside_sparse_window          # (2) A≠B 1 byte ngoài cửa sổ → Differs, B không đổi byte nào
dest_metadata_unchanged                # (1) ino/uid/mode/xattr/mtime của B giữ nguyên
byte_identical_pair_shares_extent      # (1) A=B 256 MiB → Same, bytes_shared == size
unsupported_fs_no_side_effect          # (3) tmpfs/ext4 → unsupported, không tạo/xóa file
exdev_across_mounts                    # (4) hai loop mount → EXDEV
fiemap_fast_path_second_run            # (5) chạy lần 2 không gọi ioctl
btrfs_two_subvolumes_same_ino          # (6) cùng st_ino ở 2 subvolume → 2 row
write_to_dst_during_ioctl              # (7) ghi vào B giữa vòng ioctl → B không ở deduped
verified_clone_lease_busy              # (8) lease bị phá → Busy/aborted, file mới không đổi byte
undo_keeps_inode_and_hash              # (10) undo giữ inode, hash không đổi
scan_resume_cursor                     # (11) restart giữa scan → cursor đúng (a/ vs a-b)
```

Bổ sung một unit test chạy trên **mọi OS** để fixture không bị rỗng nghĩa:

```rust
#[test] // nasdedup-core, chạy cả trên Windows
fn fixture_pair_is_not_vacuous() {
    let (a, b) = fixtures::pair_differing_outside_window();
    assert_eq!(sparse_hash(&a).unwrap(), sparse_hash(&b).unwrap(), "fixture vô nghĩa: hash đã khác");
    assert_ne!(a.bytes(), b.bytes(), "fixture vô nghĩa: hai file giống hệt");
}
```

### `ci/guard-i18n.sh` và `ci/guard-proto-version.sh`

```bash
# ci/guard-i18n.sh — giao diện CHỈ tiếng Việt
#!/usr/bin/env bash
set -euo pipefail
extra=$(find apps/desktop/locales -maxdepth 1 -name '*.json' ! -name 'vi.json')
[ -z "$extra" ] || { echo "::error::Chỉ được có locales/vi.json. Thừa: $extra"; exit 1; }
# Key dùng trong code phải khớp 1-1 với vi.json (thiếu key = hiện raw key cho người dùng)
node ci/i18n-keys.mjs
```

```bash
# ci/guard-proto-version.sh <base_sha>
#!/usr/bin/env bash
set -euo pipefail
BASE="$1"
git diff --name-only "$BASE"...HEAD -- crates/proto/ | grep -q . || exit 0
if ! git diff "$BASE"...HEAD -- crates/proto/src/version.rs | grep -q '^+pub const PROTOCOL_VERSION'; then
  echo "::error::Có thay đổi trong crates/proto nhưng PROTOCOL_VERSION không đổi."
  echo "Nếu thay đổi tương thích ngược, thêm nhãn 'proto-compatible' vào PR."
  exit 1
fi
```

Rule ESLint cấm chuỗi hiển thị hard-code (đảm bảo mọi text đi qua `t()` để dịch tập trung, và chỉ có một nguồn tiếng Việt):

```js
// apps/desktop/eslint.config.js (trích)
import i18next from 'eslint-plugin-i18next';
export default [
  i18next.configs['flat/recommended'],       // bật rule i18next/no-literal-string
  { rules: { 'i18next/no-literal-string': ['error', { markupOnly: true }] } },
];
```

### `deny.toml` (workspace daemon)

```toml
[graph]
targets = [
  { triple = "x86_64-unknown-linux-musl" },
  { triple = "aarch64-unknown-linux-musl" },
  { triple = "x86_64-pc-windows-msvc" },
]
all-features = true

[advisories]
version = 2
yanked = "deny"
ignore = []                       # mỗi mục ignore phải kèm comment và ngày hết hạn

[licenses]
version = 2
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
         "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib", "CC0-1.0"]
confidence-threshold = 0.93
# MPL-2.0 / GPL không được phép: dự án phát hành binary tĩnh cho người dùng cuối.

[bans]
multiple-versions = "warn"
wildcards = "deny"
deny = [
  { name = "openssl-sys", reason = "phá build musl tĩnh; dùng rustls nếu cần TLS" },
  { name = "reflink-copy", reason = "spec 5.7.3 cấm mọi API clone nhận path" },
  { name = "tokio", reason = "spec 3.1: daemon là kiến trúc đồng bộ, không async runtime" },
  { name = "notify", version = ">=9", reason = "spec 3.4: ghim 8.2, notify 9 tắt CLOSE_WRITE mặc định" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

`apps/desktop/src-tauri/deny.toml` giống hệt nhưng **bỏ** mục cấm `tokio` (Tauri bắt buộc dùng) và thêm target `x86_64-pc-windows-msvc` duy nhất.

### `.config/nextest.toml`

```toml
[profile.ci]
fail-fast = false
failure-output = "immediate-final"
status-level = "skip"
slow-timeout = { period = "120s", terminate-after = 10 }   # verify 256 MiB có thể chậm

[profile.ci.junit]
path = "junit.xml"

[[profile.ci.overrides]]
filter = 'package(nasdedup-linux)'
test-threads = 1        # integration test dùng chung mount point và lease
```

### `.github/dependabot.yml`

```yaml
version: 2
updates:
  - package-ecosystem: cargo
    directory: "/"
    schedule: { interval: weekly, day: monday, time: "02:00", timezone: Asia/Ho_Chi_Minh }
    open-pull-requests-limit: 5
    groups:
      rust-patch:
        update-types: [patch]
    ignore:
      - dependency-name: notify
        versions: [">=9"]        # spec 3.4 ghim 8.2
  - package-ecosystem: cargo
    directory: "/apps/desktop/src-tauri"
    schedule: { interval: weekly }
  - package-ecosystem: npm
    directory: "/apps/desktop"
    schedule: { interval: weekly }
  - package-ecosystem: github-actions
    directory: "/"
    schedule: { interval: weekly }
```

## 5. Pipeline phát hành (CD): thiết kế, semver, changelog

### 5.1 Luồng phát hành từ đầu đến cuối

```text
maintainer chạy "Prepare release" (chọn auto/patch/minor/major)
   │  git-cliff --bumped-version  →  cargo set-version --workspace
   │  ci/sync-version.sh          →  tauri.conf.json + package.json + src-tauri
   │  git-cliff                   →  CHANGELOG.md (tiếng Việt)
   ▼
PR "chore(release): vX.Y.Z"  ──►  CI OK + review + checklist bằng chứng NAS  ──►  merge (squash)
   ▼
tag-on-merge.yml: tạo tag vX.Y.Z, gọi release.yml
   ▼
release.yml
   gate      tag trên main? version khớp? CHANGELOG có mục? CI của SHA xanh? release-evidence có?
   daemon    cross build musl x86_64 + aarch64 → kiểm tra tĩnh → tarball
   desktop   [environment: release, cần approve] tauri build --bundles nsis, ký minisign, Authenticode
   publish   [environment: release] SHA256SUMS, latest.json, attestation, cosign, SBOM
             → tạo Release DRAFT → tải lại và verify → mới công khai
   smoke     curl endpoint auto-update thật, đối chiếu version
```

### 5.2 Ma trận build

| Artifact | Target | Công cụ | Tên file |
| :--- | :--- | :--- | :--- |
| Daemon NAS x86 | `x86_64-unknown-linux-musl` | `cross build` | `nasdedup-<v>-x86_64-unknown-linux-musl.tar.gz` |
| Daemon NAS ARM | `aarch64-unknown-linux-musl` | `cross build` | `nasdedup-<v>-aarch64-unknown-linux-musl.tar.gz` |
| App Windows | `x86_64-pc-windows-msvc` | `tauri build --bundles nsis` | `nasdedup-ui_<v>_x64-setup.exe` (+ `.sig`) |
| Kèm theo | — | — | `SHA256SUMS`, `SHA256SUMS.cosign.bundle`, `latest.json`, `nasdedup-<v>.cdx.json` (SBOM) |

Tarball daemon chứa: `nasdedup`, `config.example.toml`, `nasdedup.service`, `install.sh`, `README.md`, `LICENSE`, `CHANGELOG.md`. Chỉ NSIS, không MSI: MSI không hỗ trợ cài per-user và updater Tauri xử lý NSIS gọn hơn; một định dạng = một đường kiểm thử.

### 5.3 Đánh số phiên bản — ai bump và bump thế nào

**Nguồn sự thật duy nhất:** `[workspace.package] version` trong `Cargo.toml` gốc. Ba nơi còn lại (`tauri.conf.json`, `package.json`, `src-tauri/Cargo.toml`) do `ci/sync-version.sh` ghi, và `guard-version-sync.sh` chặn nếu lệch. Không ai sửa version bằng tay.

| Mức | Khi nào | Ví dụ trong dự án này |
| :--- | :--- | :--- |
| **patch** | sửa lỗi, không đổi `PROTOCOL_VERSION`, không đổi schema DB, không đổi `hash_version` | sửa parse magic MXF; sửa hiển thị báo cáo |
| **minor** | tính năng mới, proto tương thích ngược (thêm field optional), migration DB chỉ thêm cột/bảng | thêm màn hình lịch sử; thêm `remote_verify` mới |
| **major** | đổi `PROTOCOL_VERSION` không tương thích, đổi `hash_version`/`hash.chunks` (buộc `nasdedup db rebuild`), đổi mặc định `general.mode` | v1.0 khi bật mặc định fanotify |

**Ai bump:** không ai. `prepare-release.yml` chạy `git-cliff --bumped-version` suy ra mức từ conventional commits (`fix:` → patch, `feat:` → minor, `feat!:`/`BREAKING CHANGE` → major); maintainer chỉ ghi đè khi git-cliff suy sai. **Ai được chạy:** người có quyền `write` (workflow_dispatch) — nhưng release chỉ thực sự xảy ra sau khi PR được merge (cần review) và sau khi reviewer của environment `release` bấm approve.

### 5.4 Conventional commits và changelog tiếng Việt

Dùng squash merge, nên **PR title chính là commit message**. Bắt buộc đúng dạng bằng một action nhỏ (thêm vào `ci.yml` nếu muốn hard-fail, hoặc chạy riêng):

```yaml
  pr-title:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-24.04
    permissions: { pull-requests: read }
    steps:
      - uses: amannn/action-semantic-pull-request@v5
        env: { GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}' }
        with:
          types: |
            feat
            fix
            perf
            refactor
            docs
            test
            build
            ci
            chore
          scopes: |
            core
            db
            linux
            daemon
            proto
            ui
            ci
            docs
```

`cliff.toml` — nhóm tiếng Việt, vì changelog chính là nội dung hiển thị trong hộp thoại cập nhật của app:

```toml
[changelog]
header = "# Nhật ký thay đổi\n\n"
body = """
{% if version %}## [{{ version | trim_start_matches(pat="v") }}] — {{ timestamp | date(format="%d/%m/%Y") }}
{% else %}## Chưa phát hành{% endif %}
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group }}
{% for c in commits %}- {{ c.message | upper_first }}{% if c.breaking %} **(thay đổi phá vỡ tương thích)**{% endif %}
{% endfor %}{% endfor %}\n
"""
trim = true

[git]
conventional_commits = true
filter_unconventional = true
protect_breaking_commits = true
commit_parsers = [
  { message = "^feat",           group = "Tính năng mới" },
  { message = "^fix",            group = "Sửa lỗi" },
  { message = "^perf",           group = "Hiệu năng" },
  { message = "^refactor",       group = "Tái cấu trúc" },
  { message = "^docs",           group = "Tài liệu" },
  { message = "^(test|ci|build|chore)", skip = true },
]
tag_pattern = "v[0-9]*"
```

`git-cliff --latest --strip all` sinh phần thân của GitHub Release và đồng thời là `notes` trong `latest.json`. Người dùng đọc **đúng một** bản mô tả ở cả ba nơi (GitHub, hộp thoại cập nhật, `CHANGELOG.md` trong tarball).

## 6. `.github/workflows/release.yml` (đầy đủ, đã kiểm tra cú pháp)

```yaml
name: Release

on:
  push:
    tags: ["v[0-9]+.[0-9]+.[0-9]+", "v[0-9]+.[0-9]+.[0-9]+-*"]
  workflow_dispatch:
    inputs:
      tag:
        description: "Tag đã tồn tại, ví dụ v1.2.3"
        required: true
        type: string

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: "0"
  TAG: ${{ inputs.tag || github.ref_name }}

jobs:
  gate:
    name: Cổng chất lượng trước phát hành
    runs-on: ubuntu-24.04
    permissions:
      contents: read
      actions: read
    outputs:
      version: ${{ steps.v.outputs.version }}
      prerelease: ${{ steps.v.outputs.prerelease }}
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ env.TAG }}
          fetch-depth: 0
      - name: Tag phải trỏ tới commit nằm trên main
        run: |
          git fetch origin main --depth=200
          git merge-base --is-ancestor "$(git rev-parse HEAD)" origin/main \
            || { echo "::error::Tag $TAG không nằm trên main"; exit 1; }
      - id: v
        name: Đối chiếu version với tag
        run: |
          bash ci/guard-version-sync.sh
          V=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="nasdedup")|.version')
          [ "v$V" = "$TAG" ] || { echo "::error::Tag $TAG != version $V"; exit 1; }
          case "$V" in *-*) PRE=true ;; *) PRE=false ;; esac
          echo "version=$V" >> "$GITHUB_OUTPUT"
          echo "prerelease=$PRE" >> "$GITHUB_OUTPUT"
      - name: CHANGELOG phải có mục cho version này
        run: grep -q "\[${{ steps.v.outputs.version }}\]" CHANGELOG.md
      - name: CI của chính commit này phải xanh
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          SHA=$(git rev-parse HEAD)
          CONC=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs?head_sha=$SHA&status=completed" \
            --jq '[.workflow_runs[] | select(.name=="CI")] | sort_by(.created_at) | last | .conclusion')
          echo "CI conclusion = $CONC"
          [ "$CONC" = "success" ] || { echo "::error::CI cho $SHA không xanh"; exit 1; }
      - name: Bằng chứng smoke test trên NAS thật (kernel 4.4 DSM)
        run: |
          F="release-evidence/${TAG}.json"
          [ -f "$F" ] || { echo "::error::Thiếu $F — chạy ci/nas-smoke.sh trên NAS rồi commit kết quả"; exit 1; }
          jq -e '.result == "pass" and .kernel != null and .fstype != null' "$F"

  daemon:
    name: Daemon musl ${{ matrix.target }}
    needs: gate
    runs-on: ubuntu-24.04
    permissions:
      contents: read
    strategy:
      fail-fast: true
      matrix:
        target: [x86_64-unknown-linux-musl, aarch64-unknown-linux-musl]
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ env.TAG }}
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: rel-${{ matrix.target }}
      - uses: taiki-e/install-action@v2
        with:
          tool: cross
      - run: cross build --release --locked --target ${{ matrix.target }} --bin nasdedup
      - name: Kiểm tra tĩnh và chạy thử
        run: |
          f=target/${{ matrix.target }}/release/nasdedup
          file "$f" | grep -q "statically linked" || { echo "::error::không tĩnh"; exit 1; }
          if [ "${{ matrix.target }}" = "x86_64-unknown-linux-musl" ]; then "$f" --version; fi
      - name: Đóng gói tarball
        env:
          VERSION: ${{ needs.gate.outputs.version }}
          TARGET: ${{ matrix.target }}
        run: |
          set -euo pipefail
          D="nasdedup-$VERSION-$TARGET"
          mkdir -p "dist/$D"
          cp "target/$TARGET/release/nasdedup" "dist/$D/"
          cp examples/config.example.toml "dist/$D/"
          cp packaging/nasdedup.service packaging/install.sh "dist/$D/"
          cp README.md LICENSE CHANGELOG.md "dist/$D/"
          chmod +x "dist/$D/nasdedup" "dist/$D/install.sh"
          tar -C dist -czf "dist/$D.tar.gz" "$D"
          rm -rf "dist/$D"
      - uses: actions/upload-artifact@v4
        with:
          name: dist-daemon-${{ matrix.target }}
          path: dist/*.tar.gz
          retention-days: 7

  desktop:
    name: App Windows (Tauri v2)
    needs: gate
    runs-on: windows-2022
    environment: release          # cần approve; secret ký chỉ tồn tại ở đây
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ env.TAG }}
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: apps/desktop/src-tauri -> target
          shared-key: rel-tauri
      - working-directory: apps/desktop
        run: pnpm install --frozen-lockfile
      - name: Build và ký artifact updater (minisign)
        working-directory: apps/desktop
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        run: pnpm exec tauri build --target x86_64-pc-windows-msvc --bundles nsis
      - name: Ký Authenticode (Azure Trusted Signing qua OIDC — không lưu PFX)
        if: vars.SIGN_WINDOWS == 'true'
        uses: azure/trusted-signing-action@v0
        with:
          azure-tenant-id: ${{ secrets.AZURE_TENANT_ID }}
          azure-client-id: ${{ secrets.AZURE_CLIENT_ID }}
          azure-client-secret: ${{ secrets.AZURE_CLIENT_SECRET }}
          endpoint: ${{ vars.AZURE_SIGNING_ENDPOINT }}
          trusted-signing-account-name: ${{ vars.AZURE_SIGNING_ACCOUNT }}
          certificate-profile-name: ${{ vars.AZURE_SIGNING_PROFILE }}
          files-folder: apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis
          files-folder-filter: exe
          file-digest: SHA256
          timestamp-rfc3161: http://timestamp.acs.microsoft.com
          timestamp-digest: SHA256
      - name: Gom artifact
        shell: bash
        run: |
          set -euo pipefail
          mkdir -p dist
          B=apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis
          cp "$B"/*-setup.exe dist/
          cp "$B"/*-setup.exe.sig dist/
          ls -l dist
      - uses: actions/upload-artifact@v4
        with:
          name: dist-desktop-windows
          path: dist/*
          retention-days: 7

  publish:
    name: Ký, checksum, tạo Release
    needs: [gate, daemon, desktop]
    runs-on: ubuntu-24.04
    environment: release
    permissions:
      contents: write
      id-token: write
      attestations: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ env.TAG }}
          fetch-depth: 0
      - uses: actions/download-artifact@v4
        with:
          pattern: dist-*
          merge-multiple: true
          path: dist
      - uses: taiki-e/install-action@v2
        with:
          tool: git-cliff
      - name: Ghi chú phát hành (tiếng Việt)
        run: git-cliff --latest --strip all -o release-notes.md
      - name: SHA256SUMS
        working-directory: dist
        run: |
          sha256sum * > SHA256SUMS
          cat SHA256SUMS
      - name: Sinh latest.json cho Tauri updater
        env:
          VERSION: ${{ needs.gate.outputs.version }}
        run: bash ci/make-latest-json.sh "$VERSION" "$TAG" dist release-notes.md
      - uses: actions/attest-build-provenance@v1
        with:
          subject-path: |
            dist/*.tar.gz
            dist/*-setup.exe
      - uses: sigstore/cosign-installer@v3
      - name: Ký SHA256SUMS bằng cosign keyless (không có private key)
        working-directory: dist
        run: cosign sign-blob --yes --bundle SHA256SUMS.cosign.bundle SHA256SUMS
      - uses: anchore/sbom-action@v0
        with:
          path: .
          format: cyclonedx-json
          output-file: dist/nasdedup-${{ needs.gate.outputs.version }}.cdx.json
          upload-artifact: false
          upload-release-assets: false
      - name: Tạo GitHub Release (DRAFT)
        env:
          GH_TOKEN: ${{ github.token }}
          PRE: ${{ needs.gate.outputs.prerelease }}
        run: |
          set -euo pipefail
          args=(--draft --title "$TAG" --notes-file release-notes.md)
          [ "$PRE" = "true" ] && args+=(--prerelease)
          gh release create "$TAG" "${args[@]}" dist/*
      - name: Tải lại từ Release và tự kiểm tra trước khi công khai
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          mkdir -p verify && cd verify
          gh release download "$TAG" --clobber
          sha256sum -c SHA256SUMS
          cosign verify-blob --bundle SHA256SUMS.cosign.bundle \
            --certificate-identity-regexp "^https://github.com/$GITHUB_REPOSITORY/.github/workflows/release.yml@refs/tags/" \
            --certificate-oidc-issuer https://token.actions.githubusercontent.com \
            SHA256SUMS
          jq -e '.version and .platforms["windows-x86_64"].signature and .platforms["windows-x86_64"].url' latest.json
          tar -tzf nasdedup-*-x86_64-unknown-linux-musl.tar.gz > /dev/null
      - name: Công khai release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release edit "$TAG" --draft=false --latest=${{ needs.gate.outputs.prerelease == 'false' }}

  smoke-update:
    name: Kiểm tra endpoint auto-update
    needs: [gate, publish]
    if: needs.gate.outputs.prerelease == 'false'
    runs-on: ubuntu-24.04
    steps:
      - name: latest.json phải tải được từ endpoint thật
        run: |
          set -euo pipefail
          URL="https://github.com/$GITHUB_REPOSITORY/releases/latest/download/latest.json"
          for i in 1 2 3 4 5; do curl -fsSL "$URL" -o latest.json && break; sleep 10; done
          jq -e --arg v "${{ needs.gate.outputs.version }}" '.version == $v' latest.json
          U=$(jq -r '.platforms["windows-x86_64"].url' latest.json)
          curl -fsIL "$U" -o /dev/null
          echo "Endpoint auto-update OK: $U"
```

### `ci/make-latest-json.sh`

```bash
#!/usr/bin/env bash
# make-latest-json.sh <version> <tag> <dist_dir> <notes_file>
set -euo pipefail
VERSION="$1"; TAG="$2"; DIST="$3"; NOTES="$4"
sig=$(ls "$DIST"/*-setup.exe.sig)          # đúng 1 file, ls sẽ lỗi nếu 0 hoặc glob sai
name=$(basename "${sig%.sig}")
jq -n \
  --arg v "$VERSION" \
  --arg n "$(cat "$NOTES")" \
  --arg d "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg s "$(cat "$sig")" \
  --arg u "https://github.com/$GITHUB_REPOSITORY/releases/download/$TAG/$name" \
  '{version:$v, notes:$n, pub_date:$d, platforms:{"windows-x86_64":{signature:$s, url:$u}}}' \
  > "$DIST/latest.json"
jq -e '.platforms["windows-x86_64"].signature | length > 100' "$DIST/latest.json" >/dev/null
echo "Đã sinh $DIST/latest.json cho $name"
```

## 7. `prepare-release.yml` và `tag-on-merge.yml`

```yaml
# .github/workflows/prepare-release.yml
name: Prepare release

on:
  workflow_dispatch:
    inputs:
      bump:
        description: "Mức tăng phiên bản (auto = suy từ conventional commits)"
        required: true
        type: choice
        options: [auto, patch, minor, major]
        default: auto

permissions:
  contents: read

jobs:
  pr:
    name: Mở PR chore(release)
    runs-on: ubuntu-24.04
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: main
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@stable
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-edit,git-cliff
      - id: bump
        name: Tăng version ở một nơi rồi đồng bộ
        run: |
          set -euo pipefail
          if [ "${{ inputs.bump }}" = "auto" ]; then
            NEXT=$(git-cliff --bumped-version | sed 's/^v//')
            cargo set-version --workspace "$NEXT"
          else
            cargo set-version --workspace --bump "${{ inputs.bump }}"
          fi
          V=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="nasdedup")|.version')
          bash ci/sync-version.sh "$V"
          echo "version=$V" >> "$GITHUB_OUTPUT"
      - run: git-cliff --tag "v${{ steps.bump.outputs.version }}" -o CHANGELOG.md
      - run: bash ci/guard-version-sync.sh
      - uses: peter-evans/create-pull-request@v6
        with:
          branch: release/v${{ steps.bump.outputs.version }}
          title: "chore(release): v${{ steps.bump.outputs.version }}"
          commit-message: "chore(release): v${{ steps.bump.outputs.version }}"
          body: |
            Bản phát hành v${{ steps.bump.outputs.version }}.

            Trước khi merge:
            - [ ] CI OK xanh
            - [ ] Đã chạy `ci/nas-smoke.sh` trên NAS 192.168.1.213 và commit
                  `release-evidence/v${{ steps.bump.outputs.version }}.json`
            - [ ] Đọc lại CHANGELOG.md — đây là nội dung người dùng thấy trong hộp thoại cập nhật
            - [ ] Với bản major: đã có ghi chú về `nasdedup db rebuild` / đổi PROTOCOL_VERSION
          labels: release
          delete-branch: true
```

```yaml
# .github/workflows/tag-on-merge.yml
name: Tag on merge

on:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  tag:
    if: "startsWith(github.event.head_commit.message, 'chore(release): v')"
    runs-on: ubuntu-24.04
    permissions:
      contents: write
      actions: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - id: v
        run: |
          set -euo pipefail
          bash ci/guard-version-sync.sh
          V=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[]|select(.name=="nasdedup")|.version')
          echo "tag=v$V" >> "$GITHUB_OUTPUT"
      - name: Tạo tag
        env:
          TAG: ${{ steps.v.outputs.tag }}
        run: |
          set -euo pipefail
          if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
            echo "::error::Tag $TAG đã tồn tại"; exit 1
          fi
          git config user.name  "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git tag -a "$TAG" -m "$TAG"
          git push origin "$TAG"
      - name: Kích hoạt workflow Release
        env:
          GH_TOKEN: ${{ github.token }}
          TAG: ${{ steps.v.outputs.tag }}
        run: gh workflow run release.yml --ref main -f tag="$TAG"
```

**Cạm bẫy đã xử lý:** tag do `GITHUB_TOKEN` push **không** kích hoạt `on: push: tags` (GitHub chặn đệ quy workflow). Vì thế bước cuối gọi `gh workflow run` — `workflow_dispatch` thì `GITHUB_TOKEN` gọi được, không cần PAT hay GitHub App. Đây là lý do `release.yml` có cả hai trigger và biến `TAG: ${{ inputs.tag || github.ref_name }}`.

## 8. Quản lý khóa ký

### 8.1 Nguyên tắc: giảm số private key xuống mức nhỏ nhất có thể

| Thứ cần ký | Cơ chế | Có private key không? | Lưu ở đâu |
| :--- | :--- | :--- | :--- |
| **Gói cập nhật app (bắt buộc)** | minisign của Tauri updater | **Có — khóa DUY NHẤT của dự án** | `secrets.TAURI_SIGNING_PRIVATE_KEY` + `..._PASSWORD`, chỉ trong environment `release` |
| Toàn bộ artifact (provenance) | `actions/attest-build-provenance` (Sigstore, OIDC) | Không | — |
| `SHA256SUMS` | `cosign sign-blob` keyless (OIDC) | Không | — |
| Installer Windows (SmartScreen) | Azure Trusted Signing, xác thực bằng OIDC federated credential | Không (khóa nằm trong HSM của Azure) | — |
| Commit/tag | Signed commits bắt buộc trên `main` (ruleset) | Khóa cá nhân của maintainer | Máy maintainer / 1Password |

Mọi thứ trừ khóa updater đều **keyless**: không có gì để lộ, không có gì để xoay vòng, không có gì hết hạn giữa đêm.

### 8.2 Sinh và nạp khóa updater

```bash
# CHẠY TRÊN MÁY MAINTAINER, không bao giờ trong CI, không bao giờ trên NAS
cd apps/desktop
pnpm exec tauri signer generate -w "$HOME/.tauri/nasdedup-updater.key"
# → sinh nasdedup-updater.key (private, có mật khẩu) và nasdedup-updater.key.pub

# 1. Public key vào repo (commit)
jq --arg k "$(cat ~/.tauri/nasdedup-updater.key.pub)" \
   '.plugins.updater.pubkey = $k' src-tauri/tauri.conf.json > t && mv t src-tauri/tauri.conf.json

# 2. Private key vào GitHub Secrets của environment "release" (KHÔNG phải repo secret)
gh secret set TAURI_SIGNING_PRIVATE_KEY --env release < ~/.tauri/nasdedup-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release

# 3. Sao lưu: 1Password/Bitwarden + một bản in giấy cất két. Xóa khỏi ~/Downloads, shell history.
```

Quy tắc vận hành:

- Secret đặt ở **environment** `release`, không phải repo-level: job nào không khai `environment: release` thì không đọc được, kể cả PR từ fork.
- Fork không bao giờ nhận được secret (GitHub chặn sẵn); `pull_request_target` **không được dùng** ở bất kỳ workflow nào.
- Không `echo` khóa; nếu buộc phải log gì đó liên quan, dùng `::add-mask::`.
- Chỉ maintainer có role Admin mới sửa được secret; bật audit log để thấy ai đọc/ghi.

### 8.3 Xoay khóa updater (có kế hoạch)

Tauri v2 chỉ nhận **một** `pubkey` trong `tauri.conf.json`, nên xoay khóa cần một **bản phát hành cầu nối**:

| Bước | Việc | Kết quả |
| :--- | :--- | :--- |
| 1 | Sinh cặp khóa mới `K2` ở máy maintainer | |
| 2 | Phát hành `vN` — **config chứa pubkey của K2**, nhưng artifact vẫn ký bằng `K1` | Máy đang chạy `vN-1` (biết `K1`) chấp nhận và cài `vN` |
| 3 | Sau khi telemetry/GitHub cho thấy phần lớn đã lên `vN`, đổi secret sang `K2` | |
| 4 | Phát hành `vN+1` ký bằng `K2` | Chỉ máy đã lên `vN` cập nhật được |
| 5 | Máy còn kẹt ở `< vN` | Phải tải installer thủ công từ Releases — ghi rõ trong ghi chú phát hành và trong app |

Lịch: xoay định kỳ **24 tháng**, hoặc ngay lập tức khi nghi ngờ. Vì bước 5 gây phiền cho người dùng, đừng xoay khi không có lý do.

### 8.4 Nếu lộ khóa updater

Hiểu đúng mức thiệt hại trước: **chữ ký một mình không đủ để tấn công**. Client chỉ tải từ `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`. Kẻ có khóa vẫn phải ghi được vào GitHub Releases của repo này (hoặc chiếm được DNS/TLS của github.com). Vì vậy bảo vệ repo (2FA bắt buộc, ruleset, environment approval, không có PAT dài hạn) là lớp phòng thủ ngang hàng với bảo vệ khóa.

Sổ tay xử lý sự cố:

| Thời điểm | Hành động | Ai làm |
| :--- | :--- | :--- |
| 0–15 phút | Xóa `TAURI_SIGNING_PRIVATE_KEY` khỏi environment `release`; vô hiệu hóa `release.yml` (đổi tên file hoặc thêm `if: false`); kiểm tra danh sách release và asset xem có bản lạ không | Maintainer |
| 0–30 phút | Nếu **đã có** bản độc hại: `gh release delete <tag> --yes` + `gh release edit <tag_tốt> --latest` để endpoint quay về bản sạch; endpoint hồi phục trong vài giây | Maintainer |
| 30–60 phút | Rà audit log (ai đọc secret, token nào), thu hồi mọi PAT/GitHub App token, xoay `AZURE_CLIENT_SECRET` | Maintainer |
| 1–4 giờ | Sinh `K2`, phát hành bản cầu nối theo 8.3 bước 2, ký bằng `K1` **từ máy offline** (không dùng khóa trong CI nữa cho tới khi làm rõ) | Maintainer |
| ≤ 24 giờ | Mở **GitHub Security Advisory** kèm SHA-256 của mọi asset hợp lệ; đăng thông báo trên README bằng tiếng Việt: cách kiểm tra máy mình có cài bản độc hại không (so `SHA256` của `nasdedup-ui.exe` đã cài) | Maintainer |
| ≤ 7 ngày | Chuyển sang phát hành từ môi trường sạch, cân nhắc hạ tần suất auto-update, viết post-mortem | Maintainer |

Điều **không** làm được: gỡ bản độc hại đã cài trên máy người dùng. Đây chính là lý do phần lớn thiết kế ở trên hướng tới việc *không có khóa để lộ* và *cần approve của con người* cho mọi lần chạm vào khóa duy nhất còn lại.

## 9. Cổng chất lượng bắt buộc trước khi phát hành

### 9.1 Danh sách cổng — tất cả phải xanh

| # | Cổng | Thực thi ở đâu | Chặn cái gì |
| :--- | :--- | :--- | :--- |
| 1 | **`differs_outside_sparse_window`**: A và B khác đúng 1 byte ngoài cửa sổ sparse hash → `Differs`, B không đổi byte nào | `ci.yml/fs-integration` (cả `modern` và `legacy`), bước có tên "CỔNG CHỐNG MẤT DỮ LIỆU" | merge + release |
| 2 | `dest_metadata_unchanged`: sau dedupe, ino/uid/mode/xattr/mtime của B y nguyên | như trên | merge + release |
| 3 | `ci/required-tests.txt` đầy đủ (11 test) tồn tại trong danh sách nextest | `guard-required-tests.sh` | merge + release |
| 4 | `--no-tests=fail` ở mọi lệnh nextest | `ci.yml` | test bị lọc hết mà job vẫn xanh |
| 5 | `fixture_pair_is_not_vacuous`: fixture có hash sparse bằng nhau nhưng byte khác nhau | `test-linux` + `test-windows` | cổng #1 trở nên vô nghĩa |
| 6 | clippy `-D warnings` với `unwrap_used/expect_used/panic = deny` | `lint` | merge |
| 7 | Coverage `nasdedup-core` + `nasdedup-db` ≥ 80 % dòng | `coverage` | merge |
| 8 | `cargo deny check advisories bans licenses sources` × 2 workspace + `pnpm audit --audit-level high` | `supply-chain` | merge |
| 9 | MSRV 1.85 build được | `msrv` | merge |
| 10 | Binary musl **thực sự tĩnh** (`file` phải nói `statically linked`) | `build-musl`, `daemon` | merge + release |
| 11 | File ≤ 400 dòng; version khớp 3 nơi; chỉ `locales/vi.json` | `guards` | merge |
| 12 | Tag nằm trên `main`, khớp version, `CHANGELOG.md` có mục, CI của SHA xanh | `release.yml/gate` | release |
| 13 | `release-evidence/<tag>.json` có `result == "pass"` (smoke test NAS thật) | `release.yml/gate` | release |
| 14 | Người phê duyệt environment `release` bấm approve | GitHub Environments | release |
| 15 | Verify sau khi tạo draft: `sha256sum -c`, `cosign verify-blob`, `latest.json` hợp lệ, tarball mở được | `release.yml/publish` | công khai release |
| 16 | `latest.json` tải được từ endpoint thật và đúng version | `release.yml/smoke-update` | phát hiện sau, mở incident |

### 9.2 Vì sao cổng #1 cần tới bốn lớp bảo vệ

Một test chống mất dữ liệu có thể trở nên vô dụng theo bốn cách khác nhau, và mỗi cách được chặn riêng:

| Cách hỏng | Lớp chặn |
| :--- | :--- |
| Test bị xóa hoặc đổi tên | `guard-required-tests.sh` đối chiếu `ci/required-tests.txt` |
| Test bị `#[ignore]` mà không ai chạy | `--run-ignored all` + `--no-tests=fail` |
| Filter `-E` không khớp gì (đổi tên module) → nextest báo 0 test | `--no-tests=fail` biến 0 test thành lỗi |
| Fixture bị sửa thành hai file có sparse hash **khác** nhau → test qua nhưng chẳng chứng minh gì | `fixture_pair_is_not_vacuous` assert `hash_A == hash_B && bytes_A != bytes_B` |

Khung test tối thiểu cho cổng #1 (spec 10, kịch bản 2):

```rust
#[test]
#[ignore = "cần NASDEDUP_IT_MOUNT"]
fn differs_outside_sparse_window() -> anyhow::Result<()> {
    let mnt = it::mount()?;                       // Btrfs loop image
    // 256 MiB; sửa đúng 1 byte tại offset nằm GIỮA hai chunk mẫu của sparse hash
    let (a, b) = it::pair_differing_outside_window(&mnt, 256 << 20)?;
    let before = it::snapshot_metadata(&b)?;      // ino, uid, mode, xattr, mtime, ctime, blake3

    assert_eq!(sparse_hash(&a)?, sparse_hash(&b)?, "tiền đề: bộ lọc PHẢI cho qua");

    let out = KernelDedupe::new().dedupe(&a, &b, a.len(), &Unlimited, &mut NoJournal)?;
    assert!(matches!(out, DedupeOutcome::Differs { .. }), "kernel phải từ chối: {out:?}");

    let after = it::snapshot_metadata(&b)?;
    assert_eq!(before, after, "B bị thay đổi — VI PHẠM Zero Data Loss");
    Ok(())
}
```

### 9.3 Soak test — cổng thủ công cho minor/major

Spec mục 10 yêu cầu soak report-only ≥ 3 ngày trước khi bật dedup. CI không làm được việc này. Đưa vào quy trình bằng cách: bản `minor`/`major` chỉ được release khi `release-evidence/<tag>.json` có thêm trường `soak` (số ngày, số cặp verify, tỉ lệ `DIFFERS`); `gate` chỉ kiểm tra sự tồn tại, người phê duyệt environment đọc nội dung.

## 10. Bảo vệ nhánh và quy trình

### 10.1 Ruleset cho `main`

```bash
gh api --method POST /repos/OWNER/REPO/rulesets --input - <<'JSON'
{
  "name": "main protected",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "required_linear_history" },
    { "type": "required_signatures" },
    { "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 1,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": true,
        "require_last_push_approval": true,
        "required_review_thread_resolution": true,
        "allowed_merge_methods": ["squash"]
      } },
    { "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": true,
        "required_status_checks": [{ "context": "CI OK" }]
      } }
  ]
}
JSON
```

Ghi chú quan trọng:

- **`bypass_actors` rỗng**: admin cũng không đẩy thẳng lên `main`. Đây là điểm hay bị bỏ qua nhất; với dự án chạm vào dữ liệu người dùng thì không có ngoại lệ.
- `allowed_merge_methods: ["squash"]` khiến PR title trở thành commit message → conventional commits + git-cliff hoạt động.
- `required_status_checks` chỉ liệt kê **`CI OK`** (mục 2.3.4).
- `required_signatures`: bắt buộc ký commit. `tag-on-merge` dùng `github-actions[bot]`, commit của nó (không có) và tag annotated đều đi qua API nên không vướng.

### 10.2 Ruleset cho tag

```bash
gh api --method POST /repos/OWNER/REPO/rulesets --input - <<'JSON'
{
  "name": "release tags immutable",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [ { "type": "deletion" }, { "type": "update" }, { "type": "non_fast_forward" } ]
}
JSON
```

Tag `v*` không xóa được, không dời được. Một `vX.Y.Z` đã phát hành thì vĩnh viễn trỏ đúng một commit — điều kiện cần để attestation và cosign có ý nghĩa. Không thêm rule `creation` vì `tag-on-merge` cần tạo tag; kiểm soát nằm ở chỗ chỉ commit đã merge vào `main` mới sinh ra tag.

### 10.3 Environment `release`

```bash
gh api --method PUT /repos/OWNER/REPO/environments/release --input - <<'JSON'
{
  "wait_timer": 0,
  "prevent_self_review": false,
  "reviewers": [ { "type": "User", "id": 0 } ],
  "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true }
}
JSON
# id lấy từ: gh api /users/<username> --jq .id
gh api --method POST /repos/OWNER/REPO/environments/release/deployment-branch-policies \
  -f name='v*' -f type='tag'
```

Hệ quả: job `desktop` và `publish` dừng lại chờ người bấm approve; secret ký chỉ được cấp sau đó; và chỉ ref dạng tag `v*` mới deploy được vào environment này.

### 10.4 Ai được làm gì

| Vai trò | Quyền | Có thể release? |
| :--- | :--- | :--- |
| Contributor ngoài | fork + PR | Không. Fork không nhận secret; `pull_request_target` bị cấm dùng |
| Collaborator (write) | PR, chạy `Prepare release` | Chỉ mở được PR release; không tự phát hành được |
| Maintainer (admin, trong `CODEOWNERS`) | review, approve environment | **Có** — nhưng vẫn phải qua PR + CI + approve |

`CODEOWNERS` khoá các vùng nhạy cảm:

```text
# .github/CODEOWNERS
*                               @maintainer
/crates/linux/src/dedupe.rs     @maintainer
/crates/linux/src/ioctl.rs      @maintainer
/crates/linux/src/lease.rs      @maintainer
/crates/linux/src/undo.rs       @maintainer
/crates/core/src/hash.rs        @maintainer
/ci/required-tests.txt          @maintainer
/.github/                       @maintainer
```

### 10.5 Cấu hình repo bắt buộc khác

| Mục | Giá trị | Lý do |
| :--- | :--- | :--- |
| Settings → Actions → Workflow permissions | **Read repository contents** (mặc định read-only) | Mọi job khai `permissions:` riêng; token mặc định không ghi được |
| Fork pull request workflows | "Require approval for all external contributors" | Chặn PR độc hại chạy CI với cache của ta |
| Actions cho phép | "Allow select actions" + danh sách action đã duyệt | Chặn cài action tùy tiện |
| Pin action | ghim theo **commit SHA** thay vì tag, cập nhật bằng Dependabot `github-actions` | Tag có thể bị dời (sự cố `tj-actions/changed-files` 2025). YAML ở trên dùng tag cho dễ đọc; chạy `pin-github-action` một lần trước khi public repo |
| 2FA | bắt buộc cho mọi collaborator | |
| Repo public | có | Runner miễn phí không giới hạn (Windows runner tính giá 2× ở repo private) |

## 11. Ma trận hệ điều hành NAS: kiểm thử kernel cũ

### 11.1 Vấn đề

Runner `ubuntu-24.04` chạy kernel 6.x. Spec yêu cầu chạy được từ kernel **4.4** (DSM cũ) với hàng loạt fallback: `openat2` (5.6) → `openat(O_PATH)` từng component; `statx STATX_MNT_ID` (5.8) → mountinfo; `FIDEDUPERANGE` với dest read-only (4.20) → `dest_needs_write` mở `O_RDWR`; `fanotify FAN_REPORT_DFID_NAME` (5.9) → inotify. Trên runner kernel mới, **không nhánh fallback nào từng được thực thi** — đúng phần code sẽ chạy trên NAS thật lại là phần chưa bao giờ được test.

### 11.2 Ba tầng, mỗi tầng bắt một loại rủi ro khác nhau

| Tầng | Cách làm | Bắt được gì | Tần suất | Chặn release |
| :--- | :--- | :--- | :--- | :--- |
| **L0 — Ép fallback** | `NASDEDUP_FORCE_LEGACY_SYSCALLS=1` khiến lớp `linux::compat` trả `ENOSYS` cho mọi syscall "mới"; toàn bộ `fs-integration` chạy lại lần hai | Nhánh fallback compile được, đúng logic, không panic, cho kết quả **giống hệt** nhánh hiện đại | mỗi PR | **Có** |
| **L1 — QEMU kernel thật** | virtme-ng + `vmlinuz` 4.19/5.4/5.10/6.1 từ `ghcr.io/cilium/ci-kernels`, chạy chính test binary musl tĩnh | `ENOSYS` thật của kernel cũ, hành vi ioctl/btrfs khác biệt, `FILE_DEDUPE_RANGE` dest read-only trước 4.20 | hằng đêm + label `kernel-matrix` | Không (4.19 `continue-on-error`) |
| **L2 — NAS thật** | `ci/nas-smoke.sh` chạy qua SSH lên 192.168.1.213, kết quả commit vào `release-evidence/<tag>.json` | Kernel Synology đã vá riêng, eCryptfs, quota shared folder, subvolume thật, mount CIFS tới 192.168.1.214 | trước mỗi release | **Có** (`gate` kiểm) |

**Vì sao không dùng container base image cũ:** container chia sẻ kernel của host — Debian Jessie trong Docker trên runner vẫn chạy kernel 6.x. Nó chỉ test glibc/userspace, mà daemon là musl tĩnh nên userspace không liên quan. Đây là cái bẫy phổ biến nhất và nó **không** giải quyết vấn đề.

**Vì sao không tự build kernel 4.4:** DSM 4.4 là bản fork của Synology (backport riêng, module btrfs sửa đổi, quota kiểu Synology). Một kernel 4.4 vanilla tự build vẫn không phải cái chạy trên NAS, mà tốn ~40 phút build mỗi lần. L2 rẻ hơn và trung thực hơn. `ci-kernels` không có 4.4; 4.19 là mốc gần nhất có sẵn và đủ để bắt phần lớn khác biệt syscall.

### 11.3 Lớp `compat` — điều kiện tiên quyết của L0

```rust
// crates/linux/src/compat.rs — mọi syscall "mới" đi qua đây, không gọi trực tiếp
pub const FEATURES: &[&str] = &["openat2", "statx_mnt_id", "dedupe_dest_ro", "fanotify_dfid"];

fn forced_legacy() -> bool {
    std::env::var("NASDEDUP_FORCE_LEGACY_SYSCALLS").as_deref() == Ok("1")
}

pub fn openat2_available() -> bool {
    !forced_legacy() && OPENAT2_OK.get_or_init(probe_openat2)
}

#[test] // chạy trên mọi OS: chặn việc thêm syscall mới mà quên đăng ký fallback
fn every_feature_has_fallback() {
    assert_eq!(FEATURES.len(), 4, "thêm feature mới thì phải thêm fallback và cập nhật ci/required-tests.txt");
}
```

Cổng kèm theo: `guard-file-size.sh` không đủ — thêm một grep trong `guards` cấm gọi thẳng `rustix::fs::openat2`/`statx` ngoài `compat.rs`:

```bash
# trong ci/guard-file-size.sh hoặc guard riêng
if git grep -n -E 'openat2|STATX_MNT_ID' -- 'crates/linux/src/*.rs' ':!:crates/linux/src/compat.rs' | grep -q .; then
  echo "::error::Gọi syscall mới ngoài compat.rs — nhánh fallback sẽ không được test (mục 11)"; exit 1
fi
```

### 11.4 `.github/workflows/kernel-matrix.yml`

```yaml
name: Kernel matrix

on:
  schedule:
    - cron: "0 18 * * *"      # 01:00 giờ Việt Nam
  workflow_dispatch:
  pull_request:
    types: [labeled, synchronize]

permissions:
  contents: read

jobs:
  qemu:
    name: Kernel ${{ matrix.kernel }} (QEMU)
    if: github.event_name != 'pull_request' || contains(github.event.pull_request.labels.*.name, 'kernel-matrix')
    runs-on: ubuntu-24.04
    continue-on-error: ${{ matrix.experimental }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - { kernel: "4.19", experimental: true }
          - { kernel: "5.4",  experimental: false }
          - { kernel: "5.10", experimental: false }
          - { kernel: "6.1",  experimental: false }
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-unknown-linux-musl
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: kernel-matrix
      - name: Công cụ
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends \
            qemu-system-x86 busybox-static btrfs-progs xfsprogs musl-tools python3-pip
          pipx install virtme-ng || pip3 install --break-system-packages virtme-ng
      - name: Build test binary tĩnh (musl) — chạy được trên mọi kernel
        run: |
          set -euo pipefail
          export CC_x86_64_unknown_linux_musl=musl-gcc
          cargo test -p nasdedup-linux --release --locked --no-run \
            --target x86_64-unknown-linux-musl --message-format=json \
            | jq -r 'select(.profile.test == true) | .executable' | grep -v null > tests.txt
          cat tests.txt
      - name: Lấy vmlinuz ${{ matrix.kernel }}
        run: |
          set -euo pipefail
          cid=$(docker create "ghcr.io/cilium/ci-kernels:${{ matrix.kernel }}")
          docker cp "$cid:/boot/vmlinuz" "$RUNNER_TEMP/vmlinuz"
          docker rm "$cid"
      - run: truncate -s 2G "$RUNNER_TEMP/fsimg.raw"
      - name: Chạy integration test trong VM
        run: |
          vng --run "$RUNNER_TEMP/vmlinuz" \
              --cpus 2 --memory 2G \
              --disk fsimg="$RUNNER_TEMP/fsimg.raw" \
              --rwdir "$RUNNER_TEMP" \
              --exec "bash $GITHUB_WORKSPACE/ci/qemu-run.sh" | tee vm.log
          grep -q "NASDEDUP_VM_RESULT=0" vm.log \
            || { echo "::error::Test thất bại trên kernel ${{ matrix.kernel }}"; exit 1; }
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: vmlog-${{ matrix.kernel }}
          path: vm.log

  cross-arch:
    name: Test aarch64 dưới qemu-user
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-unknown-linux-musl
      - uses: taiki-e/install-action@v2
        with:
          tool: cross
      - run: cross test --release --locked --target aarch64-unknown-linux-musl -p nasdedup-core -p nasdedup-db
```

```bash
# ci/qemu-run.sh — chạy BÊN TRONG VM (virtme-ng chia sẻ rootfs của host qua 9p)
#!/usr/bin/env bash
set -euxo pipefail
mount -t proc  proc  /proc  2>/dev/null || true
mount -t sysfs sysfs /sys   2>/dev/null || true
DEV=$(lsblk -pnro NAME,TYPE | awk '$2=="disk"{print $1}' | tail -1)
mkfs.btrfs -q -f "$DEV"
mkdir -p /mnt/nd-btrfs && mount "$DEV" /mnt/nd-btrfs
btrfs subvolume create /mnt/nd-btrfs/subA
btrfs subvolume create /mnt/nd-btrfs/subB
export NASDEDUP_IT_MOUNT=/mnt/nd-btrfs
export NASDEDUP_IT_SMALL=1        # file thử 64 MiB thay vì 256 MiB (VM chỉ có 2 GiB RAM)
rc=0
while read -r bin; do "$bin" --include-ignored --test-threads 1 || rc=$?; done < "$GITHUB_WORKSPACE/tests.txt"
echo "kernel=$(uname -r)"
echo "NASDEDUP_VM_RESULT=$rc"
```

### 11.5 `ci/nas-smoke.sh` — bằng chứng từ NAS thật (L2)

```bash
#!/bin/sh
# Chạy TRÊN NAS 192.168.1.213 trước mỗi release:
#   scp nasdedup ci/nas-smoke.sh nas:/tmp/ && ssh nas 'sh /tmp/nas-smoke.sh v1.2.3 /tmp/nasdedup'
# Kết quả JSON đem về commit vào release-evidence/<tag>.json
set -eu
TAG="$1"; BIN="${2:-./nasdedup}"
ROOT="${NASDEDUP_SMOKE_ROOT:-/volume1/video/.nasdedup-smoke}"
mkdir -p "$ROOT"; RESULT=pass

KERNEL=$(uname -r)
FSTYPE=$(stat -f -c %T "$ROOT")

# 1. Binary musl tĩnh chạy được trên kernel này
"$BIN" --version || RESULT=fail

# 2. Cặp giống hệt và cặp khác 1 byte ngoài cửa sổ — check LUÔN dry-run, không đụng dữ liệu thật
head -c 67108864 /dev/urandom > "$ROOT/a.bin"; cp "$ROOT/a.bin" "$ROOT/b.bin"
cp "$ROOT/a.bin" "$ROOT/c.bin"
printf 'X' | dd of="$ROOT/c.bin" bs=1 seek=33554433 conv=notrunc 2>/dev/null
"$BIN" check "$ROOT/a.bin" "$ROOT/b.bin" | grep -qi 'same'   || RESULT=fail
"$BIN" check "$ROOT/a.bin" "$ROOT/c.bin" | grep -qi 'differs' || RESULT=fail

# 3. Probe backend: kỳ vọng kernel_dedupe trên Btrfs của DSM
BACKEND=$("$BIN" status --json 2>/dev/null | sed -n 's/.*"backend":"\([a-z_]*\)".*/\1/p' | head -1)
[ -n "$BACKEND" ] || BACKEND=unknown

# 4. Mount CIFS tới máy Windows còn sống (mục 1.5)
REMOTE=$(mount | grep -c cifs || true)

rm -f "$ROOT/a.bin" "$ROOT/b.bin" "$ROOT/c.bin"
printf '{"tag":"%s","kernel":"%s","fstype":"%s","backend":"%s","cifs_mounts":%s,"result":"%s","at":"%s"}\n' \
  "$TAG" "$KERNEL" "$FSTYPE" "$BACKEND" "$REMOTE" "$RESULT" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
```

Script cố tình dùng `/bin/sh` + `dd`/`sed` chứ không bash/jq: BusyBox của DSM không có đủ công cụ GNU.

## 12. Auto-update: endpoint, cấu hình, rollback, và phiên bản daemon

### 12.1 Cấu hình Tauri updater

```json5
// apps/desktop/src-tauri/tauri.conf.json (trích)
{
  "productName": "nasdedup",
  "version": "0.1.0",                      // do ci/sync-version.sh ghi, không sửa tay
  "identifier": "vn.nasdedup.app",
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "createUpdaterArtifacts": true,        // sinh *-setup.exe.sig
    "windows": { "nsis": { "installMode": "currentUser", "languages": ["Vietnamese"] } }
  },
  "plugins": {
    "updater": {
      "active": true,
      "dialog": false,                     // TỰ vẽ hộp thoại tiếng Việt, không dùng dialog mặc định
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6...",
      "endpoints": [
        "https://github.com/OWNER/REPO/releases/latest/download/latest.json"
      ]
    }
  }
}
```

`dialog: false` là bắt buộc với yêu cầu "giao diện chỉ tiếng Việt": hộp thoại built-in của Tauri hiện tiếng Anh. Thay vào đó frontend có các component nhỏ, tách bạch (chống God Component):

```text
apps/desktop/src/tinh-nang/cap-nhat/
├── dungCapNhat.ts          hook: check → tải → cài (bọc @tauri-apps/plugin-updater)
├── HuyHieuCoBanMoi.tsx     chấm nhỏ trên thanh tiêu đề khi có bản mới
├── HopThoaiCapNhat.tsx     hiện version, ghi chú (từ latest.json), nút "Cập nhật ngay"
├── ThanhTienTrinh.tsx      %, tốc độ, nút hủy
└── locale.ts               chỉ import key từ locales/vi.json
```

### 12.2 Kill switch — gỡ một bản xấu trong dưới 60 giây

Endpoint là `/releases/latest/download/latest.json`, mà `latest` do GitHub tính. Nên:

```bash
# Đánh dấu bản xấu là pre-release → GitHub trả "latest" về bản trước → client ngừng thấy bản mới
gh release edit v1.2.3 --prerelease --latest=false
gh release edit v1.2.2 --latest
# Nếu cần triệt để: xóa hẳn asset latest.json của bản xấu
gh release delete-asset v1.2.3 latest.json --yes
```

Máy đã lỡ cập nhật thì phải cài đè bản cũ thủ công — ghi rõ hướng dẫn trong Advisory. Đây là lý do `publish` bắt buộc verify trước khi rời trạng thái draft.

### 12.3 Phiên bản daemon và app lệch nhau

App Windows tự cập nhật được; daemon trên NAS thì **không** — cho daemon tự tải và tự thay binary trên NAS là thêm một đường ghi vào hệ thống mà spec đang cố tránh, và nó cần quyền root. v1 giữ nguyên tắc: **app không bao giờ ghi lên NAS**.

Cơ chế thay thế:

| Tình huống | Hành vi |
| :--- | :--- |
| `PROTOCOL_VERSION` khớp | Chạy bình thường |
| Daemon cũ hơn nhưng proto tương thích | Banner vàng: "NAS đang chạy nasdedup 1.1.0, ứng dụng là 1.2.0. Vẫn dùng được." + nút xem hướng dẫn |
| Proto không tương thích | Chặn mọi thao tác ghi (bật dedup, undo), chỉ cho xem báo cáo; hiện lệnh copy-paste một dòng để cập nhật daemon |

Lệnh cập nhật daemon hiện trong app (người dùng dán vào SSH), lấy đúng từ Release của bản app đang chạy:

```bash
curl -fsSL https://github.com/OWNER/REPO/releases/download/v1.2.0/nasdedup-1.2.0-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz && sudo ./nasdedup-1.2.0-x86_64-unknown-linux-musl/install.sh
```

Cổng CI đi kèm: `ci/guard-proto-version.sh` (mục 4) bắt buộc bump `PROTOCOL_VERSION` khi `crates/proto` đổi, và `nasdedup-proto` có test tương thích ngược (serde round-trip của mọi message từ version trước, fixture JSON lưu trong `crates/proto/tests/compat/`) — chạy trên cả Windows và Linux.

### 12.4 Ghép cặp (pairing) và CI

Mã pairing không thuộc phạm vi CI/CD, nhưng có hai ràng buộc phải kiểm trong pipeline:

1. Test `pairing_token_never_logged`: bật `tracing` mức TRACE, chạy luồng pairing với `MemoryRepository`, assert output không chứa token. Nằm trong `required-tests.txt`.
2. `cargo deny` cấm crate crypto không rõ nguồn; token dùng `blake3` + `getrandom` đã có sẵn trong cây phụ thuộc.

## 13. Lộ trình áp dụng theo phase

Không nên bật hết 13 job ngay hôm nay — nhiều job sẽ đỏ vì code chưa tồn tại (Phase 1–6 chưa làm). Thứ tự đưa vào:

| Khi nào | Thêm gì | Vì sao lúc này |
| :--- | :--- | :--- |
| **Ngay (Phase 0 đã xong)** | `guards`, sửa `lint` (bỏ `RUSTFLAGS` global, thêm `--locked`, `cargo doc`, `machete`), `test-linux`/`test-windows` với nextest, `msrv`, `supply-chain`, `build-musl` với `taiki-e/install-action`, `ci-ok`, ruleset `main` | Đều chạy được với code hiện có; `guards` sớm ngày nào thì tránh được God Component ngày đó |
| **Phase 1 xong** | `coverage` với ngưỡng 80 % | `nasdedup-db` có test đầy đủ thì ngưỡng mới có nghĩa |
| **Phase 2 xong** | `fixture_pair_is_not_vacuous`, `ci/required-tests.txt` (phần unit), `prepare-release.yml` + `cliff.toml` + `CHANGELOG.md` | Fixture "đổi 1 byte ngoài cửa sổ" là deliverable của Phase 2 |
| **Phase 3 xong** | `fs-integration` (mode `modern`), `ci/nas-smoke.sh` chạy lần đầu | Có `LinuxFs` và scan để test |
| **Phase 5 xong** | `fs-integration` mode `legacy`, `kernel-matrix.yml`, toàn bộ `required-tests.txt`, **cổng chống mất dữ liệu bật ở chế độ chặn** | `KernelDedupe`/`VerifiedClone` mới tồn tại từ Phase 5 |
| **Song song với UI** | `frontend`, `desktop-windows`, `guard-i18n.sh`, `guard-proto-version.sh` | Ngay khi tạo `apps/desktop` |
| **Trước release v0.1.0 công khai** | `release.yml`, `tag-on-merge.yml`, environment `release`, khóa updater, ruleset tag, ghim action theo SHA | |

### Ước tính chi phí

Repo **public** → runner miễn phí không giới hạn, kể cả `windows-2022`. Nếu buộc phải để private: Windows tính 2× phút, `test-windows` + `desktop-windows` ≈ 20 phút/PR ≈ 40 phút tính phí → khoảng 30 PR/tháng đã ăn hết gói 2 000 phút của Free. Khuyến nghị public ngay từ đầu, vì dự án dù sao cũng phát hành cho nhiều người qua GitHub.

## Quyết định thiết kế

- **`ci-ok` là status check bắt buộc DUY NHẤT, gom kết quả 11 job bằng `if: always()` + jq**
  - Lý do: Có lọc theo đường dẫn (`dorny/paths-filter`) nên job hay bị `skipped`. GitHub coi required check ở trạng thái skipped là *pending* và khoá merge vĩnh viễn. Job tổng hợp biến "skipped" thành "đạt" và "cancelled/failure" thành đỏ, đồng thời khiến ruleset chỉ cần khai một context — thêm job mới không phải sửa branch protection.
  - Đã loại: Liệt kê từng job trong `required_status_checks`: mỗi lần lọc bỏ một job là merge bị treo; và mỗi lần thêm job phải nhớ cập nhật ruleset (rất dễ quên → job mới đỏ mà vẫn merge được).
- **Bỏ `env: RUSTFLAGS: -D warnings` ở cấp workflow; chặn warning bằng `[workspace.lints]` + `cargo clippy -- -D warnings` + `RUSTDOCFLAGS` chỉ ở bước `cargo doc`**
  - Lý do: `RUSTFLAGS` áp lên **mọi** crate kể cả dependency từ crates.io: một bản `serde`/`rusqlite` mới phát sinh deprecation warning sẽ làm đỏ CI dù code ta không đổi một dòng. Nó cũng nằm trong cache key của `Swatinem/rust-cache`, nên mọi thay đổi biến này xoá sạch cache.
  - Đã loại: Giữ `RUSTFLAGS` global như `ci.yml` hiện tại: đơn giản hơn nhưng biến CI thành nguồn đỏ giả định kỳ, và chính điều đó dạy người ta bỏ qua CI đỏ — thói quen nguy hiểm với dự án có bất biến zero-data-loss.
- **`apps/desktop/src-tauri` là workspace Rust riêng (`exclude` khỏi workspace daemon), chia sẻ kiểu qua crate path `nasdedup-proto`**
  - Lý do: Giữ `cargo test --workspace` của daemon nhanh và chạy được trên Windows mà không kéo theo tokio/WebView2/hàng trăm crate của Tauri; giữ `[workspace.lints] unwrap_used/panic = deny` cho daemon (code Tauri sinh ra có `unwrap`); cho phép `deny.toml` của daemon cấm `tokio` theo spec 3.1 trong khi app vẫn dùng tokio; MSRV hai bên độc lập.
  - Đã loại: Một workspace duy nhất: `cargo clippy --workspace` sẽ đòi GTK/WebKit trên Linux runner, `Cargo.lock` chung khiến Dependabot của app làm rebuild daemon, và lints của daemon phải nới lỏng cho cả cây — mất chính hàng rào bảo vệ quan trọng nhất.
- **Test chống mất dữ liệu được bảo vệ bằng bốn lớp: `ci/required-tests.txt`, `--run-ignored all`, `--no-tests=fail`, và test `fixture_pair_is_not_vacuous`**
  - Lý do: Một test an toàn có thể mất hiệu lực theo bốn cách độc lập (bị xóa, bị `#[ignore]`, filter không khớp gì, hoặc fixture bị sửa thành hai file có sparse hash khác nhau) và trong cả bốn trường hợp CI vẫn xanh. Với bất biến số 1 của dự án, một cổng "xanh giả" nguy hiểm hơn không có cổng.
  - Đã loại: Chỉ chạy `cargo test -- --ignored` rồi tin là nó đã chạy: đây là cách phần lớn dự án làm và cũng là cách phần lớn dự án âm thầm mất cổng an toàn sau một lần refactor đổi tên module.
- **Chỉ tồn tại một private key trong toàn hệ thống (khóa updater Tauri, minisign); mọi chữ ký còn lại dùng OIDC keyless (GitHub attestation, cosign, Azure Trusted Signing)**
  - Lý do: Khóa nào không tồn tại thì không lộ được, không hết hạn, không cần xoay vòng, không cần sao lưu. Tauri v2 bắt buộc phải có khóa minisign riêng nên không tránh được — nhưng đúng một khóa thì sổ tay xử lý sự cố ngắn và ai cũng nhớ được. Khóa đó lại nằm trong environment `release` có required reviewer, nên mỗi lần dùng đều có con người bấm nút.
  - Đã loại: (a) Lưu PFX Authenticode trong Secrets: từ 2023 chứng chỉ OV/EV bắt buộc HSM nên gần như không xin được PFX hợp lệ, và một PFX trong Secrets là mục tiêu tấn công vĩnh viễn. (b) Tự ký GPG/minisign cho tarball daemon: thêm một khóa nữa để bảo vệ mà không có thêm bảo đảm nào so với attestation + cosign.
- **Kiểm thử kernel cũ theo ba tầng: ép fallback bằng `NASDEDUP_FORCE_LEGACY_SYSCALLS` (mỗi PR, chặn merge) → QEMU/virtme-ng kernel 4.19–6.1 (hằng đêm) → smoke test trên DSM thật với bằng chứng commit vào `release-evidence/` (chặn release)**
  - Lý do: Tầng 1 rẻ gần như bằng 0 và bắt được lỗi thường gặp nhất: nhánh fallback không compile hoặc cho kết quả khác nhánh hiện đại. Tầng 2 bắt hành vi kernel thật mà không cần phần cứng. Tầng 3 là thứ duy nhất chứng minh được trên kernel 4.4 đã vá riêng của Synology cùng eCryptfs, quota shared folder và mount CIFS thật; đưa vào repo dưới dạng file JSON khiến `gate` kiểm tra tự động thay vì trông vào trí nhớ.
  - Đã loại: (a) Container base image cũ (Debian Jessie/CentOS 7): container dùng chung kernel host nên hoàn toàn không test được gì về syscall — cái bẫy phổ biến nhất. (b) Tự build kernel 4.4 vanilla: ~40 phút/lần và vẫn không phải kernel Synology. (c) Cài self-hosted runner trên DSM: runner GitHub cần .NET, rất khó trên DSM, và mở một cổng tấn công vào chính máy chứa dữ liệu.
- **Version có một nguồn sự thật (`[workspace.package].version`), ba nơi còn lại do `ci/sync-version.sh` ghi và `guard-version-sync.sh` chặn; mức bump do `git-cliff --bumped-version` suy từ conventional commits**
  - Lý do: Bốn nơi khai version (Cargo workspace, src-tauri Cargo, tauri.conf.json, package.json) là công thức chắc chắn dẫn tới bản phát hành mà installer ghi 1.2.0 còn `latest.json` ghi 1.1.9 — updater sẽ lặp vô hạn hoặc không bao giờ kích hoạt. Guard chạy ở cả `guards`, `gate`, `prepare-release` và `tag-on-merge` nên không có đường lách.
  - Đã loại: `release-plz`/`release-please`: mạnh nhưng thiên về publish crates.io và không biết gì về `tauri.conf.json`/`package.json`; ta vẫn phải viết bước sync, nên thà dùng `cargo set-version` + `git-cliff` cho ít lớp trừu tượng hơn.
- **Release đi qua trạng thái draft, tự tải lại toàn bộ asset để verify (`sha256sum -c`, `cosign verify-blob`, mở tarball, đọc `latest.json`) rồi mới công khai; sau đó còn một job curl endpoint thật**
  - Lý do: Sai sót hay gặp nhất của CD không phải build hỏng mà là asset thiếu, tên file lệch so với `latest.json`, hoặc `.sig` rỗng vì secret không được nạp. Verify từ phía người dùng (tải từ Release chứ không dùng file trong `dist/`) bắt đúng lớp lỗi đó trước khi có ai tải về.
  - Đã loại: Tạo release công khai luôn rồi sửa nếu sai: `/releases/latest/download/latest.json` cập nhật gần như tức thì, nên một phút sai sót đủ để hàng loạt client tải bản hỏng — và với auto-update thì không rút lại được ở máy đã cài.

## Rủi ro

- [critical] Cổng chống mất dữ liệu chỉ có ý nghĩa nếu integration test thực sự chạy trên filesystem có FIDEDUPERANGE. Nếu bước tạo loop image thất bại âm thầm (thiếu `btrfs-progs`, hết đĩa runner, `losetup` không cấp được device), test có thể rơi vào nhánh `unsupported` và **vẫn xanh**.
  - Giảm thiểu: Bước tạo image dùng `set -euxo pipefail` và kết thúc bằng `findmnt -no FSTYPE,TARGET` — mount hỏng thì job đỏ ngay tại đó. Thêm một test `it_mount_is_really_btrfs` trong `required-tests.txt` assert `statfs.f_type == 0x9123683E` và assert backend probe ra `kernel_dedupe`; nếu ra `unsupported` thì `panic!` chứ không skip. Ba loop image dùng 9 GB trong khi runner ubuntu-24.04 còn ~25 GB — kiểm `df -h` ở đầu job nếu sau này tăng kích thước.
- [critical] Rò rỉ `TAURI_SIGNING_PRIVATE_KEY` cho phép ký gói cập nhật độc hại. Kết hợp với quyền ghi vào GitHub Releases, kẻ tấn công đẩy được mã tùy ý xuống mọi máy Windows đã cài app — và không có cách nào gỡ bản đã cài.
  - Giảm thiểu: Secret đặt ở environment `release` có required reviewer (mỗi lần dùng đều có người bấm approve), không phải repo secret; cấm tuyệt đối `pull_request_target`; `GITHUB_TOKEN` mặc định read-only; ruleset tag `v*` bất biến; 2FA bắt buộc; ghim action theo commit SHA trước khi public repo. Sổ tay sự cố ở mục 8.4 có kill switch dưới 60 giây (`gh release edit --prerelease` đưa endpoint về bản sạch) và quy trình phát hành bản cầu nối để xoay khóa. Ghi rõ trong tài liệu: chữ ký một mình không đủ, kẻ tấn công vẫn phải chiếm được Releases của repo.
- [medium] Job QEMU (`kernel-matrix`) dựa vào hai thứ ngoài tầm kiểm soát: image `ghcr.io/cilium/ci-kernels` (có thể đổi layout hoặc bỏ tag cũ) và `virtme-ng` (yêu cầu kernel có 9p/virtio; kernel 4.19 có thể boot lỗi). Job đỏ vì hạ tầng chứ không vì code sẽ nhanh chóng bị bỏ qua.
  - Giảm thiểu: Job chạy `schedule` + label, **không** chặn merge; 4.19 để `continue-on-error: true`; artifact `vm.log` luôn được upload để phân biệt "boot hỏng" với "test hỏng". Chỉ nâng một kernel lên mức chặn sau khi nó xanh liên tục 2 tuần. Bằng chứng bắt buộc cho kernel cũ nằm ở tầng L2 (`release-evidence/`), không phụ thuộc job này. Nếu `ci-kernels` biến mất: mirror `vmlinuz` cần dùng vào một release riêng của repo và tải từ đó.
- [high] `fs-integration` chạy dưới `sudo -E env "PATH=$PATH"`. Test chạy quyền root trên runner có thể chạm vào filesystem của runner (không phải loop image) nếu một đường dẫn bị tính sai — và cùng đoạn code đó sẽ chạy root trên NAS chứa dữ liệu thật.
  - Giảm thiểu: Mọi test integration mở file qua `dirfd` của root với `RESOLVE_BENEATH` (spec 5.6), và helper `it::mount()` `panic!` nếu `NASDEDUP_IT_MOUNT` không phải mount point riêng (`findmnt --target` phải trả về đúng đường dẫn đó, không phải `/`). Thêm assert `statfs.f_type` khớp loại filesystem mong đợi trước khi ghi bất cứ gì. Bước `umount` chạy `if: always()`.
- [medium] `ci/guard-file-size.sh` chặn file > 400 dòng, nhưng dễ bị lách bằng cách chia một God Component thành ba file 399 dòng phụ thuộc lẫn nhau — chỉ tiêu số dòng không đo được ranh giới trách nhiệm.
  - Giảm thiểu: Guard là sàn tối thiểu, không thay thế review. Bổ sung `CODEOWNERS` bắt buộc maintainer duyệt các file nhạy cảm (`dedupe.rs`, `ioctl.rs`, `lease.rs`, `undo.rs`, `hash.rs`) và một guard nữa cấm gọi syscall mới ngoài `compat.rs` (mục 11.3) — loại guard theo *ranh giới kiến trúc* này bắt được thứ mà đếm dòng bỏ sót. Checklist PR có mục "module mới có một trách nhiệm duy nhất?".
- [low] Coverage `--fail-under-lines 80` dễ trở thành nghi lễ: người ta viết test dễ để nâng số mà không tăng độ an toàn, và ngưỡng cứng làm PR sửa lỗi nhỏ bị đỏ vì lý do không liên quan.
  - Giảm thiểu: Ngưỡng chỉ áp cho `nasdedup-core` + `nasdedup-db` — hai crate thuần, deterministic, nơi test rẻ và có ý nghĩa thật; báo cáo toàn workspace chạy `continue-on-error`. Cái thực sự chặn merge là `ci/required-tests.txt`: danh sách kịch bản cụ thể theo spec mục 10, không phải một con số. Nếu ngưỡng gây ma sát lặp lại, hạ nó và thêm kịch bản vào `required-tests.txt` thay vì nới cả hai.
- [medium] Chuỗi `prepare-release → PR → merge → tag-on-merge → gh workflow run release.yml` có nhiều mắt xích; nếu `tag-on-merge` tạo tag xong rồi lỗi ở bước dispatch, tag tồn tại nhưng không có release — và ruleset tag bất biến khiến không xóa tag đi làm lại được.
  - Giảm thiểu: `release.yml` nhận cả `workflow_dispatch` với input `tag`, nên chạy lại thủ công bằng một lệnh: `gh workflow run release.yml --ref main -f tag=v1.2.3`. `tag-on-merge` kiểm tra tag đã tồn tại chưa trước khi tạo (không ghi đè) và chạy `guard-version-sync.sh` trước khi chạm vào git. `gate` idempotent — chạy lại an toàn vì Release chỉ được tạo ở bước cuối và `gh release create` sẽ lỗi nếu đã tồn tại.
- [high] App Windows tự cập nhật còn daemon trên NAS thì cập nhật thủ công, nên trong thực tế hai bên sẽ lệch version thường xuyên. Nếu xử lý lệch version không tốt, người dùng có thể bật chế độ `dedup` từ app mới lên daemon cũ hiểu sai tham số.
  - Giảm thiểu: `PROTOCOL_VERSION` trong `nasdedup-proto` là hợp đồng cứng; `guard-proto-version.sh` bắt buộc bump khi crate đổi; `crates/proto/tests/compat/` giữ fixture JSON của mọi version trước và test round-trip chạy trên cả Windows lẫn Linux. Khi proto không tương thích, app **chặn mọi thao tác ghi** (bật dedup, undo) và chỉ cho xem báo cáo, kèm lệnh cập nhật daemon copy-paste được. Đây là mặc định fail-closed, khớp với nguyên tắc report-only của spec.
- [low] CI hiện chạy `cargo install cross --locked` (2–3 phút mỗi job) và sẽ còn chậm hơn khi thêm job; pipeline chậm dẫn tới thói quen merge khi CI chưa xong.
  - Giảm thiểu: Thay toàn bộ `cargo install` bằng `taiki-e/install-action@v2` (tải binary sẵn, ~10 s); tách `shared-key` của `rust-cache` theo lane để không đập cache lẫn nhau; lọc job bằng `dorny/paths-filter`; `concurrency` hủy run cũ của cùng PR. Kết quả ước tính: ~8 phút khi cache ấm, ~3 phút cho PR chỉ chạm frontend. Nếu vẫn chậm, chuyển `coverage` sang chạy hằng đêm thay vì mỗi PR — nó là job đắt nhất mà giá trị biên thấp nhất.
