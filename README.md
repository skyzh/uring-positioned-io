# uring-positioned-io

Fully asynchronized positioned I/O with io_uring.

## Limitations

Read buffer must be valid until a read is complete. This means that you must
poll a read future until completion. You could not abort a read future.

## Benchmark

First, generate benchmark 128 files, with 1GB (262144 * 4K block) size each. Each block is filled
with `hash(file_id, block_id)`.

```bash
cargo run -p uring_bench --release generate --nf 128 --nb 262144 --dir ~/Work/uring_bench
```

Then, benchmark with `io_uring`. Benchmark script will read 4K block and verify it.

```bash
RUST_LOG=info cargo run --release -- read --nf 128 --nb 262144 --dir ~/Work/uring_bench --duration 60 --concurrent 32 --ql 512
RUST_LOG=info cargo run --release -- read --nf 128 --nb 262144 --dir ~/Work/uring_bench --duration 60 --concurrent 512 --ql 512
```

Finally, compare `io_uring` with `mmap` by benchmarking.

```bash
RUST_LOG=info cargo run -p uring_bench --release -- read_mmap --nf 128 --nb 262144 --dir ~/Work/uring_bench --duration 60 --threads 8
RUST_LOG=info cargo run -p uring_bench --release -- read_mmap --nf 128 --nb 262144 --dir ~/Work/uring_bench --duration 60 --threads 32
```

Benchmark result can be found on [my blog](https://www.skyzh.dev/posts/articles/2021-01-30-async-random-read-with-rust/).
