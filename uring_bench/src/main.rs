use bytes::{Buf, BufMut};
use std::{collections::hash_map::DefaultHasher, time::Instant};
use std::{fs::File, hash::Hasher, io, path::Path};
use tokio::sync::oneshot::channel;
use uring_positioned_io::{RandomAccessFiles, UringRandomAccessFiles};

use async_stream::stream;
use clap::{App, Arg, SubCommand};
use futures::{pin_mut, stream::*};
use io::{BufWriter, Write};
use rand::prelude::*;
use tic::{Clocksource, Interest, Percentile, Receiver, Sample};

#[macro_use]
extern crate log;

fn gen_hash(id: usize, block: usize) -> u64 {
    let mut hasher = DefaultHasher::default();
    hasher.write_usize(id);
    hasher.write_usize(block);
    hasher.finish()
}

fn verify_buf(mut buf: &[u8], id: usize, block: usize) {
    assert_eq!(buf.len(), 4096);
    let val = gen_hash(id, block);
    while buf.len() > 0 {
        assert_eq!(buf.get_u64(), val);
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Metric {
    BlockOp,
}

impl std::fmt::Display for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

unsafe fn madvise_rand(ptr: *const u8, len: usize) -> io::Result<()> {
    let result = libc::madvise(
        ptr as *mut libc::c_void,
        len,
        libc::MADV_RANDOM as libc::c_int,
    );

    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn report(
    receiver: &mut Receiver<Metric>,
    clocksource: &Clocksource,
    metric: Metric,
    total: &mut u64,
) {
    let t0 = clocksource.time();
    receiver.run_once();
    let t1 = clocksource.time();
    let m = receiver.clone_meters();

    let int = m.count(&metric).cloned().unwrap_or(0);
    let c = int - *total;
    *total = int;
    let r = c as f64 / ((t1 - t0) as f64 / 1_000_000_000.0);

    info!("rate: {} samples per second", r);
    info!(
        "latency (ns): p50: {} p90: {} p999: {} p9999: {} max: {}",
        m.latency_percentile(&metric, Percentile("p50".to_owned(), 50.0))
            .unwrap_or(&0),
        m.latency_percentile(&metric, Percentile("p90".to_owned(), 90.0))
            .unwrap_or(&0),
        m.latency_percentile(&metric, Percentile("p999".to_owned(), 99.9))
            .unwrap_or(&0),
        m.latency_percentile(&metric, Percentile("p9999".to_owned(), 99.99))
            .unwrap_or(&0),
        m.latency_percentile(&metric, Percentile("max".to_owned(), 100.0))
            .unwrap_or(&0)
    );
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let matches = App::new("uring")
        .about("A benchmark utility for io_uring")
        .version("1.0")
        .author("Alex Chi")
        .subcommand(
            SubCommand::with_name("generate")
                .about("generate files for testing")
                .arg(
                    Arg::with_name("dir")
                        .long("dir")
                        .takes_value(true)
                        .help("directory of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nf")
                        .long("nf")
                        .takes_value(true)
                        .help("number of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nb")
                        .long("nb")
                        .takes_value(true)
                        .help("number of 4K blocks")
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("read")
                .about("randomly read files with io_uring")
                .arg(
                    Arg::with_name("dir")
                        .long("dir")
                        .takes_value(true)
                        .help("directory of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nf")
                        .long("nf")
                        .takes_value(true)
                        .help("number of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nb")
                        .long("nb")
                        .takes_value(true)
                        .help("number of 4K blocks")
                        .required(true),
                )
                .arg(
                    Arg::with_name("duration")
                        .long("duration")
                        .takes_value(true)
                        .help("benchmark duration in seconds")
                        .required(true),
                )
                .arg(
                    Arg::with_name("ql")
                        .long("ql")
                        .takes_value(true)
                        .help("maximum queue length")
                        .required(true),
                )
                .arg(
                    Arg::with_name("con")
                        .long("con")
                        .takes_value(true)
                        .help("concurrent tasks")
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("read_mmap")
                .about("randomly read files with mmap")
                .arg(
                    Arg::with_name("dir")
                        .long("dir")
                        .takes_value(true)
                        .help("directory of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nf")
                        .long("nf")
                        .takes_value(true)
                        .help("number of files")
                        .required(true),
                )
                .arg(
                    Arg::with_name("nb")
                        .long("nb")
                        .takes_value(true)
                        .help("number of 4K blocks")
                        .required(true),
                )
                .arg(
                    Arg::with_name("duration")
                        .long("duration")
                        .takes_value(true)
                        .help("benchmark duration in seconds")
                        .required(true),
                )
                .arg(
                    Arg::with_name("threads")
                        .long("threads")
                        .takes_value(true)
                        .help("threads to read")
                        .required(true),
                ),
        )
        .get_matches();

    env_logger::init();

    let mut receiver = Receiver::configure()
        .duration(1)
        .capacity(16384)
        .windows(60)
        .batch_size(1)
        .build();
    receiver.add_interest(Interest::Count(Metric::BlockOp));
    receiver.add_interest(Interest::LatencyPercentile(Metric::BlockOp));
    receiver.add_interest(Interest::LatencyWaterfall(
        Metric::BlockOp,
        "write_waterfall.png".to_owned(),
    ));

    let sender = receiver.get_sender();

    // spawn reporter thread
    let clocksource_ = receiver.get_clocksource();
    let clocksource = receiver.get_clocksource();
    let (report_tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut total = 0;
        for _ in 0..rx.recv().unwrap() {
            report(&mut receiver, &clocksource, Metric::BlockOp, &mut total);
        }
        info!("saving files...");
        receiver.save_files();
    });
    let clocksource = clocksource_;

    match matches.subcommand() {
        ("generate", Some(sub_matches)) => {
            let nf: usize = sub_matches.value_of("nf").unwrap().parse().unwrap();
            let nb: usize = sub_matches.value_of("nb").unwrap().parse().unwrap();
            let dir = Path::new(sub_matches.value_of("dir").unwrap());
            for id in 0..nf {
                info!("populating file #{}", id);
                let f = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(dir.join(format!("{}.blk", id)))?;
                let mut writer = BufWriter::with_capacity(4 * 1024 * 1024, f); // 4M buffer
                for block in 0..nb {
                    let val = gen_hash(id, block);
                    let mut buf = vec![];
                    buf.put_u64(val);
                    for _ in 0..(4096 / buf.len()) {
                        writer.write(&buf)?;
                    }
                }
            }

            report_tx.send(0).unwrap();
        }
        ("read", Some(sub_matches)) => {
            let nf: usize = sub_matches.value_of("nf").unwrap().parse().unwrap();
            let nb: usize = sub_matches.value_of("nb").unwrap().parse().unwrap();
            let dir = Path::new(sub_matches.value_of("dir").unwrap());
            let duration: u64 = sub_matches.value_of("duration").unwrap().parse().unwrap();
            let ql: usize = sub_matches.value_of("ql").unwrap().parse().unwrap();
            let con: usize = sub_matches.value_of("con").unwrap().parse().unwrap();
            let files = (0..nf)
                .map(|id| dir.join(format!("{}.blk", id)))
                .map(|name| File::open(name))
                .collect::<io::Result<Vec<_>>>()?;

            let (tx, mut rx) = channel();
            let mut tx = Some(tx);

            let ctx = UringRandomAccessFiles::new(files, ql)?;

            let pos_stream = stream! {
                let mut rng = thread_rng();
                loop {
                    let pos = (rng.gen_range(0..nf), rng.gen_range(0..nb));
                    yield pos;
                    if let Ok(()) = rx.try_recv() {
                        break;
                    }
                }
            };

            info!("begin running");
            report_tx.send(duration).unwrap();

            pin_mut!(pos_stream);

            let mut stream = pos_stream
                .map(|(fid, blkid)| {
                    let ctx = ctx.clone();
                    let clocksource = clocksource.clone();
                    let mut sender = sender.clone();
                    async move {
                        let mut buf = [0; 4096];
                        let start = clocksource.counter();
                        let size = ctx.read(fid as u32, blkid as u64 * 4096, &mut buf).await?;
                        assert_eq!(size, 4096);
                        verify_buf(&buf, fid, blkid);
                        let stop = clocksource.counter();
                        sender
                            .send(Sample::new(start, stop, Metric::BlockOp))
                            .unwrap();
                        Ok::<(), io::Error>(())
                    }
                })
                .buffer_unordered(con);

            let start = Instant::now();
            let mut cnt: usize = 0;

            while let Some(result) = stream.next().await {
                result?;
                cnt += 1;
                if cnt % 10000 == 0 {
                    let elsped = Instant::now().duration_since(start).as_secs_f64();
                    if elsped >= duration as f64 {
                        if let Some(tx) = tx.take() {
                            tx.send(()).unwrap();
                        };
                    }
                }
            }
        }
        ("read_mmap", Some(sub_matches)) => {
            let nf: usize = sub_matches.value_of("nf").unwrap().parse().unwrap();
            let nb: usize = sub_matches.value_of("nb").unwrap().parse().unwrap();
            let dir = Path::new(sub_matches.value_of("dir").unwrap());
            let duration: u64 = sub_matches.value_of("duration").unwrap().parse().unwrap();
            let threads: usize = sub_matches.value_of("threads").unwrap().parse().unwrap();
            let files = (0..nf)
                .map(|id| dir.join(format!("{}.blk", id)))
                .map(|name| File::open(name))
                .collect::<io::Result<Vec<_>>>()?;
            let mmap_files = std::sync::Arc::new(
                files
                    .iter()
                    .map(|f| unsafe { memmap::Mmap::map(f) })
                    .collect::<io::Result<Vec<_>>>()
                    .unwrap(),
            );

            for mmap_file in &*mmap_files {
                unsafe {
                    madvise_rand(mmap_file.as_ptr(), mmap_file.len()).unwrap();
                }
            }

            let (tx, rx) = crossbeam_channel::unbounded();

            info!("begin running");
            report_tx.send(duration).unwrap();

            for _i in 0..threads {
                let mmap_files = mmap_files.clone();
                let rx = rx.clone();
                let clocksource = clocksource.clone();
                let mut sender = sender.clone();
                std::thread::spawn(move || {
                    let mut rng = thread_rng();
                    let mut cnt = 0;
                    loop {
                        let (fid, blkid) = (rng.gen_range(0..nf), rng.gen_range(0..nb));
                        let start = clocksource.counter();
                        let mut buf = vec![0; 4096];
                        buf.copy_from_slice(&mmap_files[fid][(blkid * 4096)..((blkid + 1) * 4096)]);
                        verify_buf(&buf, fid, blkid);
                        let stop = clocksource.counter();
                        sender
                            .send(Sample::new(start, stop, Metric::BlockOp))
                            .unwrap();
                        cnt += 1;
                        if cnt % 10000 == 0 {
                            if matches!(
                                rx.try_recv(),
                                Err(crossbeam_channel::TryRecvError::Disconnected)
                            ) {
                                break;
                            }
                        }
                    }
                });
            }
            std::thread::sleep(std::time::Duration::from_secs(duration));
            tx.send(()).unwrap();
        }
        _ => panic!("unsupported command"),
    }

    handle.join().unwrap();
    Ok(())
}
