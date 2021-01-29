# uring-positioned-io

Fully asynchronized positioned I/O with io_uring.

## Limitations

Read buffer must be valid until a read is complete. This means that you must
poll a read future until completion. You could not abort a read future.

## Benchmark

First, generate benchmark 64 files, with 1GB (262144 * 4K block) size each. Each block is filled
with `hash(file_id, block_id)`.

```bash
cargo run -p uring_bench --release generate --nf 64 --nb 262144 --dir ~/Work/uring_bench
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

* io_uring (16 concurrent reads)
```
[2021-01-29T13:15:29Z INFO  uring_bench] rate: 165345.16352563383 samples per second
[2021-01-29T13:15:29Z INFO  uring_bench] latency (ns): p50: 99353 p90: 155714 p999: 346555 p9999: 1485833 max: 1698694
```
* io_uring (512 concurrent reads)
```
[2021-01-29T12:53:29Z INFO  uring_bench] rate: 209186.86677437404 samples per second
[2021-01-29T12:53:29Z INFO  uring_bench] latency (ns): p50: 2049967 p90: 2443183 p999: 3628073 p9999: 4659872 max: 5062525
```
* mmap (16 threads, with madvise random)
```
[2021-01-29T13:12:52Z INFO  uring_bench] rate: 258887.85388082318 samples per second
[2021-01-29T13:12:52Z INFO  uring_bench] latency (ns): p50: 78775 p90: 123143 p999: 267650 p9999: 4798284 max: 13254001
```

**Latency**

* io_uring (16 concurrent reads)
  ![write_waterfall_16_new](https://user-images.githubusercontent.com/4198311/106279470-6d574c00-6277-11eb-87aa-b988e2bfed4f.png)
* io_uring (512 concurrent reads)
  ![write_waterfall_uring](https://user-images.githubusercontent.com/4198311/106277572-53683a00-6274-11eb-9e3e-91d119b6e663.png)
* mmap
  ![write_waterfall_mmap_random](https://user-images.githubusercontent.com/4198311/106279197-03d73d80-6277-11eb-8d17-b06da04c2c3a.png)

See Grafana screenshot [here](https://github.com/skyzh/uring-positioned-io/issues/2).

It seems that `io_uring` has less throughput, more latency, but less CPU usage and more efficient disk read compared with mmap.
