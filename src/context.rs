use crossbeam_channel::unbounded;
use io::ErrorKind;
use io_uring::{opcode, types::Fixed, IoUring};
use pin_project::pin_project;
use std::fs::File;
use std::future::Future;
use std::io;
use std::os::unix::io::AsRawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::oneshot::{channel, Receiver, Sender};

struct UringTask {
    complete: Option<Sender<i32>>,
}

struct UringPollFuture {
    ring: Arc<io_uring::concurrent::IoUring>,
    finish: crossbeam_channel::Receiver<()>,
}

impl Future for UringPollFuture {
    type Output = io::Result<()>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.finish.try_recv() {
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) | Ok(_) => {
                return Poll::Ready(Ok(()))
            }
        }

        // first, submit all requests
        if let Err(err) = self.ring.submit() {
            return Poll::Ready(Err(err));
        }

        // then, polling completed requests
        loop {
            if let Some(entry) = self.ring.completion().pop() {
                let uring_task: &mut UringTask = unsafe { std::mem::transmute(entry.user_data()) };
                uring_task
                    .complete
                    .take()
                    .unwrap()
                    .send(entry.result())
                    .unwrap();
            } else {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }
    }
}

#[pin_project]
pub struct UringReadFuture<T: AsMut<[u8]>> {
    ring: Arc<io_uring::concurrent::IoUring>,
    buf: Option<T>,
    id: u32,
    offset: u64,
    #[pin]
    rx: Receiver<i32>,
    submitted: bool,
    #[pin]
    task: UringTask,
    per_submit: bool,
}

impl<T: AsMut<[u8]>> UringReadFuture<T> {
    fn new(
        ring: Arc<io_uring::concurrent::IoUring>,
        buf: T,
        id: u32,
        offset: u64,
        per_submit: bool,
    ) -> Self {
        let (tx, rx) = channel();
        let task = UringTask { complete: Some(tx) };
        Self {
            ring,
            buf: Some(buf),
            id,
            offset,
            rx,
            task,
            submitted: false,
            per_submit,
        }
    }

    fn submit(self: &mut Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().project();
        if !*this.submitted {
            let buf_ptr = this.buf.as_mut().unwrap().as_mut();
            let ptr = buf_ptr.as_mut_ptr();
            let len: u32 = buf_ptr.len() as _;
            let read_op = opcode::Read::new(Fixed(*this.id), ptr, len).offset(*this.offset as i64);
            let entry = read_op
                .build()
                .user_data(this.task.get_mut() as *mut _ as u64);
            if let Err(_) = unsafe { this.ring.submission().push(entry) } {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            *this.submitted = true;

            if *this.per_submit {
                if let Err(err) = this.ring.submit() {
                    return Poll::Ready(Err(err));
                }
            }
        }

        Poll::Ready(Ok(()))
    }

    fn retrieve(self: &mut Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<i32>> {
        let this = self.as_mut().project();
        let rx = this.rx;
        match rx.poll(cx) {
            Poll::Ready(Err(_)) => {
                Poll::Ready(Err(io::Error::new(ErrorKind::Other, "failed to receive")))
            }
            Poll::Ready(Ok(x)) => Poll::Ready(Ok(x)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: AsMut<[u8]>> Future for UringReadFuture<T> {
    type Output = io::Result<(T, usize)>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = self.submit(cx);

        match result {
            Poll::Ready(Ok(_)) => self.retrieve(cx).map(|res| {
                res.and_then(|x| {
                    if x >= 0 {
                        Ok((self.buf.take().unwrap(), x as usize))
                    } else {
                        Err(io::Error::from_raw_os_error(x))
                    }
                })
            }),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

struct UringContextInner {
    handles: Vec<tokio::task::JoinHandle<io::Result<()>>>,
    ring: Arc<io_uring::concurrent::IoUring>,
    poll_finish: Option<crossbeam_channel::Sender<()>>,
    _files: Vec<File>,
}

impl UringContextInner {
    fn new(files: Vec<File>, nr: usize) -> io::Result<Self> {
        let mut this = Self::new_ref(&files, nr)?;
        this._files = files;
        Ok(this)
    }

    fn new_ref(files: &[File], nr: usize) -> io::Result<Self> {
        let ring = IoUring::new(nr as u32)?;
        let fds = files
            .iter()
            .map(|file| file.as_raw_fd())
            .collect::<Vec<_>>();
        ring.submitter().register_files(&fds)?;
        let ring = Arc::new(ring.concurrent());
        Ok(Self {
            _files: vec![],
            ring,
            handles: vec![],
            poll_finish: None,
        })
    }

    async fn flush(&self) -> io::Result<()> {
        let nop_op = opcode::Nop::new();
        let (tx, rx) = channel();
        let mut task = Box::pin(UringTask { complete: Some(tx) });
        let entry = nop_op
            .build()
            .user_data(task.as_mut().get_mut() as *mut _ as u64)
            .flags(io_uring::squeue::Flags::IO_DRAIN);

        let mut res = unsafe { self.ring.submission().push(entry) };
        while let Err(entry) = res {
            res = unsafe { self.ring.submission().push(entry) };
        }

        rx.await
            .map_err(|_| io::Error::new(ErrorKind::Other, "failed to recv"))
            .and_then(|code| {
                if code == 0 {
                    Ok(())
                } else {
                    Err(io::Error::new(ErrorKind::Other, "failed to recv"))
                }
            })?;

        Ok(())
    }
}

impl Drop for UringContextInner {
    fn drop(&mut self) {
        self.poll_finish.take().unwrap();

        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }
}

#[derive(Clone)]
pub struct UringContext {
    inner: Arc<UringContextInner>,
}

impl UringContext {
    pub fn new(files: Vec<File>, nr: usize, poll: usize) -> io::Result<Self> {
        let inner = UringContextInner::new(files, nr)?;
        Self::from_inner(inner, poll)
    }

    pub unsafe fn new_ref(files: &[File], nr: usize, poll: usize) -> io::Result<Self> {
        let inner = UringContextInner::new_ref(files, nr)?;
        Self::from_inner(inner, poll)
    }

    fn from_inner(mut inner: UringContextInner, poll: usize) -> io::Result<Self> {
        let (tx, rx) = unbounded();
        inner.poll_finish = Some(tx);
        let handles = (0..poll)
            .map(|_| {
                tokio::spawn(UringPollFuture {
                    ring: inner.ring.clone(),
                    finish: rx.clone(),
                })
            })
            .collect::<Vec<_>>();
        inner.handles = handles;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub fn read<'a, T>(&self, id: u32, offset: u64, buf: T) -> UringReadFuture<T>
    where
        T: AsMut<[u8]>,
    {
        UringReadFuture::new(self.inner.ring.clone(), buf, id, offset, false)
    }

    pub fn read_submit<'a, T>(&self, id: u32, offset: u64, buf: T) -> UringReadFuture<T>
    where
        T: AsMut<[u8]>,
    {
        UringReadFuture::new(self.inner.ring.clone(), buf, id, offset, true)
    }

    pub async fn flush(&self) -> io::Result<()> {
        self.inner.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempfile;

    #[tokio::test]
    async fn test_new() {
        let files = (0..25)
            .map(|_| {
                let mut file = tempfile().unwrap();
                writeln!(file, "test").unwrap();
                file
            })
            .collect::<Vec<_>>();
        let _context = UringContext::new(files, 256, 1).unwrap();
    }

    #[tokio::test]
    async fn test_read_and_flush() {
        let files = (0..4)
            .map(|_| {
                let mut file = tempfile().unwrap();
                write!(file, "test").unwrap();
                file.flush().unwrap();
                file
            })
            .collect::<Vec<_>>();
        let context = UringContext::new(files, 256, 1).unwrap();
        let mut buf = vec![0; 4];
        for i in 0..4 {
            let (_, sz) = context.read(i, 0, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"test");
            let (_, sz) = context.read(i, 1, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"est");
            let (_, sz) = context.read(i, 0, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"test");
        }
        context.flush().await.unwrap();
    }
}
