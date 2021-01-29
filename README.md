# uring-positioned-io

Fully asynchronized positioned I/O with io_uring.

## Limitations

Read buffer must be valid until a read is complete. This means that you must
poll a read future until completion. You could not abort a read future.

## Benchmark

First, generate benchmark 64 files, with 1GB (262144 * 4K block) size each. Each block is filled
with `hash(file_id, block_id)`.

```bash
cargo run --release generate --nf 64 --nb 262144 --dir ~/Work/uring_bench
```

Then, benchmark with `io_uring`. Benchmark script will read 4K block and verify it.

```bash
RUST_LOG=info cargo run -p uring_bench --release -- read --nf 64 --nb 262144 --dir ~/Work/uring_bench --duration 60 --con 512 --ql 512
```

Finally, compare `io_uring` with `mmap` by benchmarking.

```bash
RUST_LOG=info cargo run -p uring_bench --release -- read_mmap --nf 64 --nb 262144 --dir ~/Work/uring_bench --duration 60 --threads 16
```

On my home server with NVME SSD, the result is as follows.

**Throughput**

* io_uring
```
[2021-01-29T12:53:29Z INFO  uring_bench] rate: 209186.86677437404 samples per second
[2021-01-29T12:53:29Z INFO  uring_bench] latency (ns): p50: 2049967 p90: 2443183 p999: 3628073 p9999: 4659872 max: 5062525
```
* mmap
```
[2021-01-29T12:52:09Z INFO  uring_bench] rate: 26744.72291273619 samples per second
[2021-01-29T12:52:09Z INFO  uring_bench] latency (ns): p50: 672662 p90: 1364198 p999: 2394948 p9999: 3057648 max: 3516924
```

**Latency**

* io_uring
  ![write_waterfall_uring](https://user-images.githubusercontent.com/4198311/106277572-53683a00-6274-11eb-9e3e-91d119b6e663.png)
* mmap
  ![write_waterfall_mmap](https://user-images.githubusercontent.com/4198311/106277551-4a776880-6274-11eb-9f2c-3981685f537f.png)

See Grafana screenshot [here](https://github.com/skyzh/uring-positioned-io/issues/2).

It seems that `io_uring` has 10x throughput, 4x latency, far less CPU usage and more efficient disk read compared with mmap.
