mod context;
mod traits;
mod uring;

use bytes::{Buf, BufMut};
use std::{collections::hash_map::DefaultHasher, time::Instant};
use std::{fs::File, hash::Hasher, io, path::Path};
use traits::RandomAccessFiles;

use async_stream::stream;
use clap::{App, Arg, SubCommand};
use futures::{pin_mut, stream::*};
use io::{BufWriter, Write};
use rand::prelude::*;
use uring::UringRandomAccessFiles;

fn gen_hash(id: usize, block: usize) -> u64 {
    let mut hasher = DefaultHasher::default();
    hasher.write_usize(id);
    hasher.write_usize(block);
    hasher.finish()
}

fn verify_buf(mut buf: &[u8], id: usize, block: usize) {
    let val = gen_hash(id, block);
    while buf.len() > 0 {
        assert_eq!(buf.get_u64_le(), val);
    }
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
                    Arg::with_name("time")
                        .long("time")
                        .takes_value(true)
                        .help("benchmark time length (seconds)")
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
        .get_matches();

    match matches.subcommand() {
        ("generate", Some(sub_matches)) => {
            let nf: usize = sub_matches.value_of("nf").unwrap().parse().unwrap();
            let nb: usize = sub_matches.value_of("nb").unwrap().parse().unwrap();
            let dir = Path::new(sub_matches.value_of("dir").unwrap());
            for id in 0..nf {
                println!("populating file #{}", id);
                let f = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(dir.join(format!("{}.blk", id)))?;
                let mut writer = BufWriter::new(f);
                for block in 0..nb {
                    let val = gen_hash(id, block);
                    let mut buf = vec![];
                    buf.put_u64_le(val);
                    for _ in 0..(4096 / buf.len()) {
                        writer.write(&buf)?;
                    }
                }
            }
        }
        ("read", Some(sub_matches)) => {
            let nf: usize = sub_matches.value_of("nf").unwrap().parse().unwrap();
            let nb: usize = sub_matches.value_of("nb").unwrap().parse().unwrap();
            let dir = Path::new(sub_matches.value_of("dir").unwrap());
            let time: u64 = sub_matches.value_of("time").unwrap().parse().unwrap();
            let ql: usize = sub_matches.value_of("ql").unwrap().parse().unwrap();
            let con: usize = sub_matches.value_of("con").unwrap().parse().unwrap();
            let files = (0..nf)
                .map(|id| dir.join(format!("{}.blk", id)))
                .map(|name| File::open(name))
                .collect::<io::Result<Vec<_>>>()?;

            let ctx = UringRandomAccessFiles::new(files, ql)?;

            let pos_stream = stream! {
                let mut rng = thread_rng();
                loop {
                    let pos = (rng.gen_range(0..nf), rng.gen_range(0..nb));
                    yield pos;
                }
            };

            pin_mut!(pos_stream);

            let mut stream = pos_stream
                .map(|(fid, blkid)| {
                    let ctx = ctx.clone();
                    async move {
                        let mut buf = vec![0; 4096];
                        ctx.read(fid as u32, blkid as u64 * 4096, &mut buf).await?;
                        verify_buf(&buf, fid, blkid);
                        Ok::<(), io::Error>(())
                    }
                })
                .buffer_unordered(con);

            let start = Instant::now();
            let mut cnt = 0;
            while let Some(result) = stream.next().await {
                result?;
                cnt += 1;
                if cnt % 1000 == 0 {
                    if Instant::now().duration_since(start).as_secs() >= time {
                        break;
                    }
                }
            }
        }
        _ => panic!("unsupported command"),
    }
    Ok(())
}
