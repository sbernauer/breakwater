use std::{
    cmp::min,
    io::{Read, Write},
};

#[derive(Debug)]
pub struct MockTcpStream {
    read_data: Vec<u8>,
    write_data: Vec<u8>,
}

impl MockTcpStream {
    pub fn new() -> Self {
        MockTcpStream {
            read_data: Vec::new(),
            write_data: Vec::new(),
        }
    }

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

impl Default for MockTcpStream {
    fn default() -> Self {
        Self::new()
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
