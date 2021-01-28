use std::fs::File;

use crate::context::UringContext;
use crate::traits::RandomAccessFiles;
use async_trait::async_trait;
use std::io;

#[derive(Clone)]
pub struct UringRandomAccessFiles {
    context: UringContext,
}

impl UringRandomAccessFiles {
    pub fn new(files: Vec<File>, nr: usize) -> io::Result<Self> {
        let context = UringContext::new(files, nr)?;
        Ok(Self { context })
    }
}

#[async_trait]
impl RandomAccessFiles for UringRandomAccessFiles {
    async fn read(&self, id: u32, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let (_, sz) = self.context.read(id, offset, buf).await?;
        Ok(sz)
    }
}
