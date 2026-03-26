# Feedback Loops

Before committing, run:

```bash
cargo build 2>&1 && cargo clippy -- -D warnings 2>&1 && cargo test 2>&1
```
