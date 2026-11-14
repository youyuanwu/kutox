// SPDX-License-Identifier: MIT
//! Bounded sector-backed reads for the admitted read-only ext2 profile.

extern crate alloc;

use super::{Error, Ext4Read, IMAGE_BYTES};
use alloc::{boxed::Box, rc::Rc};
use core::{
    cell::{Cell, RefCell},
    error::Error as CoreError,
};

pub const MAX_READ_BYTES: usize = 256 * 1024;
pub const MAX_REQUESTS: usize = 512;
pub const MAX_REQUEST_BYTES: usize = 4096;
pub const BUFFER_BYTES: usize = 512;

pub trait ReadSectors {
    type Error: CoreError + Send + Sync + 'static;

    fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Statistics {
    pub bytes: usize,
    pub requests: usize,
    pub max_request: usize,
}

pub struct Reader<D> {
    device: RefCell<D>,
    statistics: Rc<Cell<Statistics>>,
}

impl<D: ReadSectors> Reader<D> {
    pub fn new(device: D) -> Self {
        Self {
            device: RefCell::new(device),
            statistics: Rc::new(Cell::new(Statistics::default())),
        }
    }

    pub fn statistics(&self) -> Rc<Cell<Statistics>> {
        Rc::clone(&self.statistics)
    }

    fn request(
        &self,
        device: &mut D,
        sector: u64,
        output: &mut [u8],
    ) -> Result<(), Box<dyn CoreError + Send + Sync>> {
        let mut stats = self.statistics.get();
        if stats.requests == MAX_REQUESTS || output.len() > MAX_READ_BYTES - stats.bytes {
            return Err(Box::new(Error::ReadLimit));
        }
        stats.requests += 1;
        stats.bytes += output.len();
        stats.max_request = stats.max_request.max(output.len());
        self.statistics.set(stats);
        device
            .read_sectors(sector, output)
            .map_err(|error| Box::new(error) as Box<dyn CoreError + Send + Sync>)
    }
}

impl<D: ReadSectors> Ext4Read for Reader<D> {
    fn read(
        &self,
        offset: u64,
        mut output: &mut [u8],
    ) -> Result<(), Box<dyn CoreError + Send + Sync>> {
        let mut offset = usize::try_from(offset).map_err(|_| Error::Range)?;
        let end = offset.checked_add(output.len()).ok_or(Error::Range)?;
        if end > IMAGE_BYTES {
            return Err(Box::new(Error::Range));
        }
        if output.is_empty() {
            return Ok(());
        }
        let mut device = self
            .device
            .try_borrow_mut()
            .map_err(|_| Error::ReaderBusy)?;
        let mut sector_buffer = [0; BUFFER_BYTES];
        while !output.is_empty() {
            let within = offset % BUFFER_BYTES;
            let count = if within != 0 || output.len() < BUFFER_BYTES {
                self.request(
                    &mut device,
                    (offset / BUFFER_BYTES) as u64,
                    &mut sector_buffer,
                )?;
                let count = output.len().min(BUFFER_BYTES - within);
                output[..count].copy_from_slice(&sector_buffer[within..within + count]);
                count
            } else {
                let count = output.len().min(MAX_REQUEST_BYTES) / BUFFER_BYTES * BUFFER_BYTES;
                self.request(
                    &mut device,
                    (offset / BUFFER_BYTES) as u64,
                    &mut output[..count],
                )?;
                count
            };
            offset += count;
            output = &mut output[count..];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    struct Device {
        fail: bool,
    }

    impl ReadSectors for Device {
        type Error = Error;

        fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Error> {
            assert_eq!(output.len() % 512, 0);
            assert!(output.len() <= MAX_REQUEST_BYTES);
            if self.fail {
                return Err(Error::ShortRead);
            }
            for (index, byte) in output.iter_mut().enumerate() {
                *byte = ((sector as usize * 512 + index) % 251) as u8;
            }
            Ok(())
        }
    }

    #[test]
    fn unaligned_reads_span_sectors_and_chunk_large_requests() {
        let reader = Reader::new(Device { fail: false });
        let mut output = [0; 9000];
        reader.read(511, &mut output).unwrap();
        let expected: Vec<_> = (511..9511).map(|index| (index % 251) as u8).collect();
        assert_eq!(output.as_slice(), expected);
        assert_eq!(
            reader.statistics.get(),
            Statistics {
                bytes: 9728,
                requests: 5,
                max_request: 4096,
            }
        );
        reader
            .read((IMAGE_BYTES - 1) as u64, &mut output[..1])
            .unwrap();
        assert_eq!(output[0], ((IMAGE_BYTES - 1) % 251) as u8);
        reader.read(IMAGE_BYTES as u64, &mut []).unwrap();
    }

    #[test]
    fn invalid_ranges_do_not_touch_output_or_issue_io() {
        let reader = Reader::new(Device { fail: false });
        for offset in [IMAGE_BYTES as u64 - 1, IMAGE_BYTES as u64, u64::MAX] {
            let mut bytes = [9; 2];
            let error = reader.read(offset, &mut bytes).unwrap_err();
            assert!(matches!(error.downcast_ref::<Error>(), Some(Error::Range)));
            assert_eq!(bytes, [9; 2]);
        }
        assert_eq!(reader.statistics.get(), Statistics::default());
    }

    #[test]
    fn malformed_superblock_is_rejected_after_only_its_admission_read() {
        let reader = Reader::new(Device { fail: false });
        let statistics = reader.statistics();
        assert!(matches!(
            super::super::Filesystem::mount_reader(reader),
            Err(Error::Image)
        ));
        assert_eq!(
            statistics.get(),
            Statistics {
                bytes: 1024,
                requests: 1,
                max_request: 1024,
            }
        );
    }

    #[test]
    fn device_errors_and_read_budgets_are_not_silent() {
        let reader = Reader::new(Device { fail: true });
        let error = reader.read(0, &mut [0; 512]).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<Error>(),
            Some(Error::ShortRead)
        ));
        let reader = Reader::new(Device { fail: false });
        for _ in 0..MAX_READ_BYTES / MAX_REQUEST_BYTES {
            reader.read(0, &mut [0; MAX_REQUEST_BYTES]).unwrap();
        }
        let before = reader.statistics.get();
        let error = reader.read(0, &mut [0; 512]).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<Error>(),
            Some(Error::ReadLimit)
        ));
        assert_eq!(reader.statistics.get(), before);
        let reader = Reader::new(Device { fail: false });
        for _ in 0..MAX_REQUESTS {
            reader.read(0, &mut [0; 1]).unwrap();
        }
        assert!(matches!(
            reader
                .read(0, &mut [0; 1])
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::ReadLimit)
        ));
    }
}
