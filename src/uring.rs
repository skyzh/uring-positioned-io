use bytes::Bytes;
use io_uring::{opcode, squeue, types, IoUring};
use std::future::Future;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fs::File, unimplemented};
use std::sync::atomic::AtomicU64;

/*
pub struct UringRandomAccessFile {
    ring: io_uring::concurrent::IoUring,
    file: File,
    id:  AtomicU64
}

impl UringRandomAccessFile {
    pub fn new(file: File) -> Self {
        let ring = IoUring::new(256).unwrap();
        ring.submitter()
            .register_files(&[file.as_raw_fd()])
            .unwrap();
        let ring = ring.concurrent();
        Self {
            file,
            ring
        }
    }

    pub fn read<'a, 'b>(
        &'a self,
        offset: u64,
        output: &'b mut [u8],
    ) -> io::Result<UringRandomAccessFileReadFuture<'a, 'b>> {
        let read_op = opcode::Read::new(types::Fixed(0), output.as_mut_ptr(), output.len() as _);
        let read = read_op.build().user_data(0).flags(squeue::Flags::IO_LINK);
        let mut squeue = self.ring.submission();

        let mut op = unsafe { squeue.push(read) };
        loop {
            if let Err(entry) = op {
                op = unsafe { squeue.push(entry) };
            } else {
                break;
            }
        }
        assert_eq!(self.ring.submit()?, 1);
        Ok(UringRandomAccessFileReadFuture {
            ring: &self.ring,
            output: &mut output
        })
    }
}

pub struct UringRandomAccessFileReadFuture<'a, 'b> {
    ring: &'a io_uring::concurrent::IoUring,
    output: &'b mut [u8],
}

impl<'a, 'b> Future for UringRandomAccessFileReadFuture<'a, 'b> {
    type Output = io::Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.ring.completion().
    }
}
*/
