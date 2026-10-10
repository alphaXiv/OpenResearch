# Các lệnh chạy OpenResearch

Chạy tất cả từ thư mục gốc repo: `cd /home/tuan/RESEARCH/OpenResearch`

## 1. Cài đặt lần đầu

```sh
# Rust (nếu chưa có)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node + pnpm (cho phần giao diện)
npm install -g pnpm
cd ui && pnpm install --frozen-lockfile && cd ..
```

## 2. Chạy ứng dụng

```sh
cargo run -- up                 # mở dashboard tại http://127.0.0.1:4791
cargo run --release -- up       # giống trên nhưng bản release (nhanh hơn)
```

Hoặc cài bản chính thức thay vì build:

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

## 3. Chạy bản dev tách biệt (khuyến nghị khi sửa code)

```sh
node scripts/dev-slot.mjs start --db empty --open   # DB mới, trống
node scripts/dev-slot.mjs start --db copy --open    # sao chép DB thật của bạn
node scripts/dev-slot.mjs status
node scripts/dev-slot.mjs stop
node scripts/dev-slot.mjs cleanup
```

## 4. Build

```sh
cargo build --locked                              # bản debug
cargo build --release --locked                    # bản release -> target/release/orx
cargo build --release --locked --features desktop # app desktop Linux (cần WebKitGTK)

cd ui && pnpm build && cd ..                      # build lại giao diện (sau khi sửa ui/)
```

## 5. Kiểm tra trước khi commit

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --locked

cd ui
pnpm typecheck
pnpm test
pnpm lint:i18n
pnpm lint:styles
cd ..
```

## 6. Lệnh `orx` hay dùng

```sh
orx projects                      # liệt kê project
orx project view <projectId>      # xem chi tiết và cây thí nghiệm
orx runs <projectId>              # liệt kê các lần chạy
orx logs <runId>                  # xem log một lần chạy
orx login / orx logout            # đăng nhập tài khoản openresearch.sh
orx telemetry off                 # tắt thống kê sử dụng
orx --help                        # xem tất cả lệnh
```

Nếu chưa cài `orx`, thay `orx` bằng `cargo run --`, ví dụ `cargo run -- projects`.
