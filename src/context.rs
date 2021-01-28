use futures::pin_mut;
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
use tokio::sync::oneshot::{channel, error::TryRecvError, Receiver, Sender};

struct UringTask {
    complete: Option<Sender<i32>>,
}

struct UringPollFuture {
    ring: Arc<io_uring::concurrent::IoUring>,
    finish: Receiver<()>,
}

impl Future for UringPollFuture {
    type Output = io::Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.finish.try_recv() {
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Closed) | Ok(_) => return Poll::Ready(Ok(())),
            }
            // first, submit all requests
            match self.ring.submit() {
                Ok(nr) => {
                    if nr != 0 {
                        // println!("{} submitted", nr);
                    }
                }
                Err(err) => return Poll::Ready(Err(err)),
            }
            // then, polling completed requests
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
}

impl<T: AsMut<[u8]>> UringReadFuture<T> {
    pub fn new(ring: Arc<io_uring::concurrent::IoUring>, buf: T, id: u32, offset: u64) -> Self {
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
        }
    }

    fn submit(self: &mut Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.submitted {
            let buf_ptr = self.buf.as_mut().unwrap().as_mut();
            let ptr = buf_ptr.as_mut_ptr();
            let len: u32 = buf_ptr.len() as _;
            let read_op = opcode::Read::new(Fixed(self.id), ptr, len).offset(self.offset as i64);
            let entry = read_op.build().user_data(&mut self.task as *mut _ as u64);
            if let Err(_) = unsafe { self.ring.submission().push(entry) } {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.submitted = true;
        }

        Poll::Ready(Ok(()))
    }

    fn retrieve(self: &mut Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<i32>> {
        let rx = &mut self.rx;
        pin_mut!(rx);
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
    handle: Option<tokio::task::JoinHandle<io::Result<()>>>,
    ring: Arc<io_uring::concurrent::IoUring>,
    poll_finish: Option<Sender<()>>,
    _files: Vec<File>,
}

impl UringContextInner {
    pub fn new(files: Vec<File>, nr: usize) -> io::Result<Self> {
        let ring = IoUring::new(nr as u32)?;
        let fds = files
            .iter()
            .map(|file| file.as_raw_fd())
            .collect::<Vec<_>>();
        ring.submitter().register_files(&fds)?;
        let ring = Arc::new(ring.concurrent());
        Ok(Self {
            _files: files,
            ring,
            handle: None,
            poll_finish: None,
        })
    }
}

impl Drop for UringContextInner {
    fn drop(&mut self) {
        let handle = self.handle.take().unwrap();
        let poll_finish = self.poll_finish.take().unwrap();
        poll_finish.send(()).unwrap();
        handle.abort();
        // TODO: wait until handle really aborts
    }
}

#[derive(Clone)]
pub struct UringContext {
    inner: Arc<UringContextInner>,
}

impl UringContext {
    pub fn new(files: Vec<File>, nr: usize) -> io::Result<Self> {
        let (tx, rx) = channel();
        let mut inner = UringContextInner::new(files, nr)?;
        inner.poll_finish = Some(tx);
        let handle = tokio::spawn(UringPollFuture {
            ring: inner.ring.clone(),
            finish: rx,
        });
        inner.handle = Some(handle);
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub fn read<'a, T>(&self, id: u32, offset: u64, buf: T) -> UringReadFuture<T>
    where
        T: AsMut<[u8]>,
    {
        UringReadFuture::new(self.inner.ring.clone(), buf, id, offset)
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
        let _context = UringContext::new(files, 256).unwrap();
    }

    #[tokio::test]
    async fn test_read() {
        let files = (0..4)
            .map(|_| {
                let mut file = tempfile().unwrap();
                write!(file, "test").unwrap();
                file.flush().unwrap();
                file
            })
            .collect::<Vec<_>>();
        let context = UringContext::new(files, 256).unwrap();
        let mut buf = vec![0; 4];
        for i in 0..4 {
            let (_, sz) = context.read(i, 0, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"test");
            let (_, sz) = context.read(i, 1, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"est");
            let (_, sz) = context.read(i, 0, &mut buf).await.unwrap();
            assert_eq!(&buf[..sz], b"test");
        }
    }
}
