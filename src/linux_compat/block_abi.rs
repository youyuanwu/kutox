// SPDX-License-Identifier: GPL-2.0-only
//! Kutox-owned block-class C contracts; not private Linux driver structures.
use super::abi::Device;
use core::{
    ffi::{c_char, c_void},
    mem::{align_of, offset_of, size_of},
};

#[repr(C)]
pub struct Page {
    pub address: usize,
    pub length: usize,
}
#[repr(C)]
pub struct Scatterlist {
    pub page_link: usize,
    pub offset: u32,
    pub length: u32,
    pub dma_address: u64,
    pub dma_length: u32,
}
#[repr(C)]
pub struct SgTable {
    pub sgl: *mut Scatterlist,
    pub nents: u32,
    pub orig_nents: u32,
}
#[repr(C)]
pub struct Queue {
    pub queuedata: *mut c_void,
    pub core: *mut c_void,
}
#[repr(C)]
pub struct BioVec {
    pub page: *mut Page,
    pub len: u32,
    pub offset: u32,
}
#[repr(C)]
pub struct BioIter {
    pub sector: u64,
    pub size: u32,
}
#[repr(C)]
pub struct Bio {
    pub iter: BioIter,
    pub next: *mut Bio,
}
#[repr(C)]
pub struct Request {
    pub queue: *mut Queue,
    pub flags: u32,
    pub rq_flags: u32,
    pub sector: u64,
    pub hctx: *mut HwContext,
    pub next: *mut Request,
    pub bio: *mut Bio,
    pub special_vec: BioVec,
}
#[repr(C)]
pub struct HwContext {
    pub queue: *mut Queue,
    pub queue_num: u32,
    pub driver_data: *mut c_void,
}
#[repr(C)]
pub struct QueueData {
    pub request: *mut Request,
    pub last: bool,
}
#[repr(C)]
pub struct QueueMap {
    pub nr_queues: u32,
    pub queue_offset: u32,
}
#[repr(C)]
pub struct TagSet {
    pub ops: *const Ops,
    pub nr_hw_queues: u32,
    pub queue_depth: u32,
    pub cmd_size: u32,
    pub flags: u32,
    pub nr_maps: u32,
    pub numa_node: i32,
    pub reserved_tags: u32,
    pub driver_data: *mut c_void,
    pub map: [QueueMap; 3],
    pub core: *mut c_void,
}
#[repr(C)]
pub struct RequestList {
    pub head: *mut Request,
}
#[repr(C)]
pub struct Batch {
    pub nr: u32,
    pub list: RequestList,
}
#[repr(C)]
pub struct Ops {
    pub queue_rq: Option<unsafe extern "C" fn(*mut HwContext, *const QueueData) -> u8>,
    pub queue_rqs: Option<unsafe extern "C" fn(*mut RequestList)>,
    pub commit_rqs: Option<unsafe extern "C" fn(*mut HwContext)>,
    pub complete: Option<unsafe extern "C" fn(*mut Request)>,
    pub init_request: Option<unsafe extern "C" fn(*mut TagSet, *mut Request, u32, u32) -> i32>,
    pub poll: Option<unsafe extern "C" fn(*mut HwContext, *mut Batch) -> i32>,
    pub map_queues: Option<unsafe extern "C" fn(*mut TagSet)>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Limits {
    pub logical_block_size: u32,
    pub physical_block_size: u32,
    pub io_min: u32,
    pub io_opt: u32,
    pub alignment_offset: u32,
    pub max_hw_sectors: u32,
    pub max_segments: u32,
    pub max_segment_size: u32,
    pub max_discard_sectors: u32,
    pub max_hw_discard_sectors: u32,
    pub max_discard_segments: u32,
    pub discard_granularity: u32,
    pub discard_alignment: u32,
    pub max_write_zeroes_sectors: u32,
    pub max_secure_erase_sectors: u32,
    pub features: u32,
}
#[repr(C)]
pub struct Disk {
    pub major: i32,
    pub first_minor: i32,
    pub minors: i32,
    pub name: [c_char; 32],
    pub private_data: *mut c_void,
    pub fops: *const DiskOps,
    pub queue: *mut Queue,
    pub dev: Device,
}
#[repr(C)]
pub struct DiskOps {
    pub owner: *mut c_void,
    pub getgeo: Option<unsafe extern "C" fn(*mut Disk, *mut c_void) -> i32>,
    pub free_disk: Option<unsafe extern "C" fn(*mut Disk)>,
    pub report_zones: Option<ReportZones>,
}
pub type ReportZone = unsafe extern "C" fn(*mut c_void, u32, *mut c_void) -> i32;
pub type ReportZones =
    unsafe extern "C" fn(*mut Disk, u64, u32, Option<ReportZone>, *mut c_void) -> i32;
pub const STORAGE_REQUIRED: u64 = (1 << 32) | (1 << 33) | (1 << 5);
pub const STORAGE_FORBIDDEN: u64 =
    (1 << 34) | (1 << 28) | (1 << 41) | (1 << 17) | (1 << 11) | (1 << 13) | (1 << 14) | (1 << 16);
pub const LAYOUT: [usize; 135] = [
    size_of::<Page>(),
    align_of::<Page>(),
    offset_of!(Page, address),
    offset_of!(Page, length),
    size_of::<Scatterlist>(),
    align_of::<Scatterlist>(),
    offset_of!(Scatterlist, page_link),
    offset_of!(Scatterlist, offset),
    offset_of!(Scatterlist, length),
    offset_of!(Scatterlist, dma_address),
    offset_of!(Scatterlist, dma_length),
    size_of::<SgTable>(),
    align_of::<SgTable>(),
    offset_of!(SgTable, sgl),
    offset_of!(SgTable, nents),
    offset_of!(SgTable, orig_nents),
    size_of::<Queue>(),
    align_of::<Queue>(),
    offset_of!(Queue, queuedata),
    offset_of!(Queue, core),
    size_of::<BioVec>(),
    align_of::<BioVec>(),
    offset_of!(BioVec, page),
    offset_of!(BioVec, len),
    offset_of!(BioVec, offset),
    size_of::<BioIter>(),
    align_of::<BioIter>(),
    offset_of!(BioIter, sector),
    offset_of!(BioIter, size),
    size_of::<Bio>(),
    align_of::<Bio>(),
    offset_of!(Bio, iter),
    offset_of!(Bio, next),
    size_of::<Request>(),
    align_of::<Request>(),
    offset_of!(Request, queue),
    offset_of!(Request, flags),
    offset_of!(Request, rq_flags),
    offset_of!(Request, sector),
    offset_of!(Request, hctx),
    offset_of!(Request, next),
    offset_of!(Request, bio),
    offset_of!(Request, special_vec),
    size_of::<HwContext>(),
    align_of::<HwContext>(),
    offset_of!(HwContext, queue),
    offset_of!(HwContext, queue_num),
    offset_of!(HwContext, driver_data),
    size_of::<QueueData>(),
    align_of::<QueueData>(),
    offset_of!(QueueData, request),
    offset_of!(QueueData, last),
    size_of::<QueueMap>(),
    align_of::<QueueMap>(),
    offset_of!(QueueMap, nr_queues),
    offset_of!(QueueMap, queue_offset),
    size_of::<TagSet>(),
    align_of::<TagSet>(),
    offset_of!(TagSet, ops),
    offset_of!(TagSet, nr_hw_queues),
    offset_of!(TagSet, queue_depth),
    offset_of!(TagSet, cmd_size),
    offset_of!(TagSet, flags),
    offset_of!(TagSet, nr_maps),
    offset_of!(TagSet, numa_node),
    offset_of!(TagSet, reserved_tags),
    offset_of!(TagSet, driver_data),
    offset_of!(TagSet, map),
    offset_of!(TagSet, core),
    size_of::<RequestList>(),
    align_of::<RequestList>(),
    offset_of!(RequestList, head),
    size_of::<Batch>(),
    align_of::<Batch>(),
    offset_of!(Batch, nr),
    offset_of!(Batch, list),
    size_of::<Ops>(),
    align_of::<Ops>(),
    offset_of!(Ops, queue_rq),
    offset_of!(Ops, queue_rqs),
    offset_of!(Ops, commit_rqs),
    offset_of!(Ops, complete),
    offset_of!(Ops, init_request),
    offset_of!(Ops, poll),
    offset_of!(Ops, map_queues),
    size_of::<Limits>(),
    align_of::<Limits>(),
    offset_of!(Limits, logical_block_size),
    offset_of!(Limits, physical_block_size),
    offset_of!(Limits, io_min),
    offset_of!(Limits, io_opt),
    offset_of!(Limits, alignment_offset),
    offset_of!(Limits, max_hw_sectors),
    offset_of!(Limits, max_segments),
    offset_of!(Limits, max_segment_size),
    offset_of!(Limits, max_discard_sectors),
    offset_of!(Limits, max_hw_discard_sectors),
    offset_of!(Limits, max_discard_segments),
    offset_of!(Limits, discard_granularity),
    offset_of!(Limits, discard_alignment),
    offset_of!(Limits, max_write_zeroes_sectors),
    offset_of!(Limits, max_secure_erase_sectors),
    offset_of!(Limits, features),
    size_of::<Disk>(),
    align_of::<Disk>(),
    offset_of!(Disk, major),
    offset_of!(Disk, first_minor),
    offset_of!(Disk, minors),
    offset_of!(Disk, name),
    offset_of!(Disk, private_data),
    offset_of!(Disk, fops),
    offset_of!(Disk, queue),
    offset_of!(Disk, dev),
    size_of::<DiskOps>(),
    align_of::<DiskOps>(),
    offset_of!(DiskOps, owner),
    offset_of!(DiskOps, getgeo),
    offset_of!(DiskOps, free_disk),
    offset_of!(DiskOps, report_zones),
    0,
    1,
    2,
    10,
    9,
    13,
    14,
    15,
    0,
    0xff,
    1 << 10,
    4096,
    512,
    3,
    STORAGE_REQUIRED as usize,
    STORAGE_FORBIDDEN as usize,
];
