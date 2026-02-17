use std::io;

use crate::streamed::{Record, current_software_version};

#[derive(Debug)]
pub struct StreamWriter<W>
where
    W: io::Write,
{
    inner: io::BufWriter<W>,
    record_count: usize,
}

impl<W> StreamWriter<W>
where
    W: io::Write,
{
    pub fn new(inner: W) -> Self {
        let mut buf_writer = io::BufWriter::new(inner);

        let version = format!("{}", current_software_version());
        postcard::to_io(&version, &mut buf_writer).unwrap();

        Self {
            inner: buf_writer,
            record_count: 0,
        }
    }

    pub fn write_record(&mut self, record: Record) {
        postcard::to_io(&record, &mut self.inner).unwrap();
        self.record_count += 1;
    }

    pub fn record_count(&self) -> usize {
        self.record_count
    }

    pub fn flush(&mut self) -> io::Result<()> {
        use std::io::Write;
        self.inner.flush()
    }
}
