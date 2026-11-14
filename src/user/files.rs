// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Reduced Moss open/read/close dispatch using kutox's checked user-copy boundary.

use crate::{
    Failure, arch::x86_64::serial, memory::paging::user::AddressSpace, user::execute,
    user::state::FileRequest,
};
use moss_libkernel::fs::{
    Error, Filesystem,
    files::{DescriptorTable, MAX_FILES},
};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Statistics {
    pub opens: usize,
    pub reads: usize,
    pub closes: usize,
    pub errors: usize,
    pub bytes: usize,
    pub live: usize,
    pub peak: usize,
}

pub(crate) struct Service {
    filesystem: Filesystem,
    table: DescriptorTable,
    statistics: Statistics,
}

pub(crate) fn filesystem_errno(error: Error) -> i64 {
    if let Some(io) = error.io_error() {
        if let Some(failure) = io.downcast_ref::<Failure>() {
            serial::print(format_args!(
                "# USER_FILE_IO stage={} code={}\n",
                failure.stage, failure.code
            ));
            return i64::from(failure.code);
        }
        serial::print(format_args!(
            "# USER_FILE_IO stage=filesystem code={}\n",
            error.errno()
        ));
    }
    error.errno()
}

impl Service {
    pub fn new(filesystem: Filesystem) -> Self {
        Self {
            filesystem,
            table: DescriptorTable::new(),
            statistics: Statistics::default(),
        }
    }

    pub fn statistics(&self) -> Statistics {
        self.statistics
    }

    pub fn complete(
        &mut self,
        space: &AddressSpace<'_, '_>,
        request: FileRequest,
    ) -> Result<i64, Failure> {
        let result = match request {
            FileRequest::Open {
                dirfd: _,
                path,
                flags,
            } => {
                self.statistics.opens += 1;
                if flags != 0 {
                    if matches!(flags & 3, 1 | 2) { -30 } else { -22 }
                } else if self.table.len() == MAX_FILES {
                    -24
                } else {
                    let mut bytes = [0; 128];
                    match execute::copy_file_path(space, path, &mut bytes)? {
                        Err(error) => error,
                        Ok(0) => -2,
                        Ok(length) => match core::str::from_utf8(&bytes[..length]) {
                            Err(_) => -22,
                            Ok(path) => match self
                                .filesystem
                                .open(path)
                                .and_then(|file| self.table.insert(file))
                            {
                                Ok(fd) => i64::from(fd),
                                Err(error) => filesystem_errno(error),
                            },
                        },
                    }
                }
            }
            FileRequest::Read { fd, request } => {
                self.statistics.reads += 1;
                match i32::try_from(fd).ok().and_then(|fd| self.table.get_mut(fd)) {
                    None => -9,
                    Some(file) => {
                        let result = execute::complete_file_read(space, file, request)?;
                        match result {
                            Ok(length) => {
                                self.statistics.bytes += length;
                                length as i64
                            }
                            Err(error) => error,
                        }
                    }
                }
            }
            FileRequest::Close { fd } => {
                self.statistics.closes += 1;
                match i32::try_from(fd).ok().and_then(|fd| self.table.remove(fd)) {
                    Some(file) => {
                        drop(file);
                        0
                    }
                    None => -9,
                }
            }
        };
        if result < 0 {
            self.statistics.errors += 1;
        }
        self.statistics.live = self.table.len();
        self.statistics.peak = self.statistics.peak.max(self.statistics.live);
        Ok(result)
    }
}
