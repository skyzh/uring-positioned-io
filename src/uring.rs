use std::fs::File;

use crate::context::UringContext;
use std::io;

#[derive(Clone)]
pub struct UringRandomAccessFiles {
    context: UringContext,
}

impl UringRandomAccessFiles {
    pub fn new(
        files: Vec<File>,
        nr: usize,
        poll_futures: usize,
        kernel_poll: bool,
    ) -> io::Result<Self> {
        let context = UringContext::new(files, nr, poll_futures, kernel_poll)?;
        Ok(Self { context })
    }

    pub async fn read_submit(&self, id: u32, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let (_, sz) = self.context.read_submit(id, offset, buf).await?;
        Ok(sz)
    }

    pub async fn flush(&self) -> io::Result<()> {
        self.context.flush().await
    }

    pub async fn read(&self, id: u32, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let (_, sz) = self.context.read(id, offset, buf).await?;
        Ok(sz)
    }
}
