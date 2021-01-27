use async_trait::async_trait;

#[async_trait]
pub trait RandomAccessFile {
    async fn read(offset: u64, n: usize);
}
