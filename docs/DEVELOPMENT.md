# DEVELOPMENT.md

## 环境要求

- Rust stable（rustup 安装，`~/.cargo/env`）
- macOS（剪切板/快捷键依赖系统 API）

## 常用命令

```bash
cargo build              # 编译（BIZ_CFG build_cmd）
cargo run                # 本地运行
cargo test               # 单元测试
cargo clippy --all-targets -- -D warnings   # lint，CI 红线：零警告
cargo fmt --check        # 格式检查
```

## 验证管道

编码完成的门禁：`cargo build && cargo test && cargo clippy --all-targets -- -D warnings` 全部通过。
