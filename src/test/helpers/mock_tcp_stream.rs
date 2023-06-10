use std::{
    cmp::min,
    io::{Read, Write},
    task::Poll,
};

use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Default)]
pub struct MockTcpStream {
    read_data: Vec<u8>,
    write_data: Vec<u8>,
}

impl MockTcpStream {
    pub fn from_input(input: &str) -> Self {
        MockTcpStream {
            read_data: input.as_bytes().to_vec(),
            write_data: Vec::new(),
        }
    }

    pub fn get_output(self) -> String {
        String::from_utf8(self.write_data).unwrap()
    }
}

impl Read for MockTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let size: usize = min(self.read_data.len(), buf.len());
        buf[..size].copy_from_slice(&self.read_data[..size]);

        self.read_data.drain(..size);
        Ok(size)
    }
}

impl Write for MockTcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_data.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AsyncRead for MockTcpStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let size: usize = min(self.read_data.len(), buf.remaining());
        buf.put_slice(&self.read_data[..size]);
        self.get_mut().read_data.drain(..size);
        std::task::Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockTcpStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        self.get_mut().write_data.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}
