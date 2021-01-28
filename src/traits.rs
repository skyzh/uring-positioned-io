use async_trait::async_trait;
use std::io;

#[async_trait]
pub trait RandomAccessFiles {
    async fn read(&self, id: u32, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}
