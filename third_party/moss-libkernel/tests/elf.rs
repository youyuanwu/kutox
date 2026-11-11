use moss_libkernel::{
    memory::{
        PAGE_SIZE,
        address_space::{OwnedPage, PageInfo, UserAddressSpace},
        permissions::PtePermissions,
    },
    task::{
        elf::{AT_PHDR, AT_PHENT, AT_PHNUM, Error, Image, LayoutError, MAX_LOAD_PAGES},
        user_stack::{AT_ENTRY, AT_NULL, AT_PAGESZ},
    },
};
use std::{cell::Cell, collections::BTreeMap};

const BASE: u64 = 0x1000_0000_0000;
const END: u64 = BASE + 16 * PAGE_SIZE as u64;
const ENTRY: u64 = BASE + 0x1080;
const DATA: u64 = BASE + 0x3ff0;

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn segment(bytes: &mut [u8], index: usize, flags: u32, values: [u64; 4]) {
    let start = 64 + index * 56;
    put32(bytes, start, 1);
    put32(bytes, start + 4, flags);
    for (field, value) in [8, 16, 32, 40].into_iter().zip(values) {
        put64(bytes, start + field, value);
    }
    put64(bytes, start + 48, PAGE_SIZE as u64);
}
fn binary() -> Vec<u8> {
    let mut bytes = vec![0; 0x3010];
    bytes[..9].copy_from_slice(b"\x7fELF\x02\x01\x01\x00\x00");
    put16(&mut bytes, 16, 2);
    put16(&mut bytes, 18, 62);
    put32(&mut bytes, 20, 1);
    put64(&mut bytes, 24, ENTRY);
    put64(&mut bytes, 32, 64);
    put16(&mut bytes, 52, 64);
    put16(&mut bytes, 54, 56);
    put16(&mut bytes, 56, 3);
    segment(&mut bytes, 0, 4, [0, BASE, 232, 232]);
    segment(&mut bytes, 1, 5, [0x1080, ENTRY, 16, 256]);
    segment(&mut bytes, 2, 6, [0x2ff0, DATA, 32, 0x2020]);
    bytes[0x1000..0x1080].fill(0xdd);
    bytes[0x1080..0x1090].fill(0xcc);
    for (index, byte) in bytes[0x2ff0..].iter_mut().enumerate() {
        *byte = index as u8 + 1;
    }
    bytes
}

fn rejects(bytes: &[u8], error: LayoutError) {
    assert_eq!(Image::parse(bytes, BASE, END).err(), Some(error));
}

#[test]
fn truncated_inputs_and_wrong_elf_identity_are_rejected_without_panics() {
    let bytes = binary();
    for length in 0..bytes.len() {
        assert!(
            Image::parse(&bytes[..length], BASE, END).is_err(),
            "{length}"
        );
    }
    for offset in [0, 1, 2, 3, 4, 5, 6, 7, 8, 20, 52] {
        let mut bad = bytes.clone();
        bad[offset] ^= 0xff;
        rejects(&bad, LayoutError::Format);
    }
    for offset in [16, 18, 48] {
        let mut bad = bytes.clone();
        bad[offset] ^= 0xff;
        rejects(&bad, LayoutError::Unsupported);
    }
}

#[test]
fn header_table_sizes_counts_ranges_and_extended_numbering_are_checked() {
    for (offset, value, error) in [
        (32, u64::MAX, LayoutError::FileRange),
        (32, 0, LayoutError::Format),
    ] {
        let mut bad = binary();
        put64(&mut bad, offset, value);
        rejects(&bad, error);
    }
    for (offset, value, error) in [
        (54, 55, LayoutError::Format),
        (54, 57, LayoutError::Format),
        (56, 0, LayoutError::Capacity),
        (56, 17, LayoutError::Capacity),
        (56, u16::MAX, LayoutError::Capacity),
    ] {
        let mut bad = binary();
        put16(&mut bad, offset, value);
        rejects(&bad, error);
    }
}

#[test]
fn load_ranges_alignment_permissions_and_page_overlap_are_checked() {
    let ph = 64 + 56;
    for (offset, value, error) in [
        (ph + 8, u64::MAX - 15, LayoutError::FileRange),
        (ph + 16, BASE - 0xf80, LayoutError::Address),
        (ph + 16, END - 0xf80, LayoutError::Address),
        (ph + 16, u64::MAX - 127, LayoutError::Address),
        (ph + 16, ENTRY + 1, LayoutError::Address),
        (ph + 32, 257, LayoutError::FileRange),
        (ph + 40, u64::MAX, LayoutError::Address),
        (ph + 48, 3, LayoutError::Address),
        (ph + 48, 0x2000_0000_0000, LayoutError::Address),
    ] {
        let mut bad = binary();
        put64(&mut bad, offset, value);
        rejects(&bad, error);
    }
    for flags in [0, 1, 2, 3, 7, 8, u32::MAX] {
        let mut bad = binary();
        put32(&mut bad, ph + 4, flags);
        rejects(&bad, LayoutError::Permissions);
    }
    let mut overlap = binary();
    segment(&mut overlap, 2, 6, [0x2ff0, BASE + 0x1ff0, 32, 32]);
    rejects(&overlap, LayoutError::Overlap);
    let mut empty = binary();
    put64(&mut empty, ph + 32, 0);
    put64(&mut empty, ph + 40, 0);
    rejects(&empty, LayoutError::Entry);
}

#[test]
fn entry_must_be_in_executable_segment_file_bytes_not_bss_or_page_padding() {
    for entry in [0, BASE, ENTRY - 1, ENTRY + 16, DATA, END, u64::MAX] {
        let mut bad = binary();
        put64(&mut bad, 24, entry);
        rejects(&bad, LayoutError::Entry);
    }
}

#[test]
fn dynamic_interpreter_tls_and_unknown_headers_are_rejected() {
    for kind in [2, 3, 7, 8, 0x6474_e552, u32::MAX] {
        let mut bad = binary();
        put32(&mut bad, 64, kind);
        rejects(&bad, LayoutError::Unsupported);
    }
    let mut stack = binary();
    put32(&mut stack, 64, 0x6474_e551);
    put32(&mut stack, 68, 7);
    rejects(&stack, LayoutError::Permissions);
    put32(&mut stack, 68, 6);
    assert!(Image::parse(&stack, BASE, END).is_ok());
}

#[test]
fn window_and_page_capacity_are_bounded_with_a_separate_stack_page() {
    let bytes = binary();
    for (start, end) in [
        (0, END),
        (BASE + 1, END),
        (BASE, END - 1),
        (BASE, BASE + PAGE_SIZE as u64),
        (BASE, 0),
        (BASE, 0x0000_8000_0000_1000),
    ] {
        assert_eq!(
            Image::parse(&bytes, start, end).err(),
            Some(LayoutError::Address)
        );
    }
    let mut full = binary();
    put32(&mut full, 64, 0);
    put32(&mut full, 64 + 112, 0);
    segment(&mut full, 1, 5, [0x1000, BASE, 16, 15 * PAGE_SIZE as u64]);
    put64(&mut full, 24, BASE);
    let state = State::default();
    let loaded = Image::parse(&full, BASE, END)
        .unwrap()
        .load(Space::new(&state), &[], &[])
        .unwrap();
    assert_eq!(loaded.load_pages, MAX_LOAD_PAGES);
    assert_eq!(state.live.get(), 16);
    drop(loaded);
    assert_eq!(state.live.get(), 0);
    put64(&mut full, 64 + 56 + 40, 16 * PAGE_SIZE as u64);
    assert_eq!(
        Image::parse(&full, BASE, END + PAGE_SIZE as u64).err(),
        Some(LayoutError::Capacity)
    );
}

#[test]
fn segment_capacity_and_empty_loads_are_checked() {
    let mut bytes = binary();
    put16(&mut bytes, 56, 9);
    put64(&mut bytes, 24, BASE);
    for index in 0..9 {
        segment(
            &mut bytes,
            index,
            5,
            [0, BASE + (index * PAGE_SIZE) as u64, 64, 64],
        );
    }
    rejects(&bytes, LayoutError::Capacity);
    put16(&mut bytes, 56, 8);
    assert!(Image::parse(&bytes, BASE, END).is_ok());
    put16(&mut bytes, 56, 16);
    for index in 1..16 {
        put32(&mut bytes, 64 + index * 56, 0);
    }
    assert!(Image::parse(&bytes, BASE, END).is_ok());
}

#[derive(Debug, Default)]
struct State {
    live: Cell<usize>,
    allocs: Cell<usize>,
    maps: Cell<usize>,
}

#[derive(Debug)]
struct Page<'a> {
    bytes: Box<[u8; PAGE_SIZE]>,
    state: &'a State,
}
impl OwnedPage for Page<'_> {
    fn as_slice_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.bytes
    }
}
impl Drop for Page<'_> {
    fn drop(&mut self) {
        self.state.live.set(self.state.live.get() - 1);
    }
}

#[derive(Debug)]
struct Space<'a> {
    state: &'a State,
    pages: BTreeMap<u64, (Page<'a>, PtePermissions)>,
    fail_alloc: Option<usize>,
    fail_map: Option<usize>,
}
impl<'a> Space<'a> {
    fn new(state: &'a State) -> Self {
        Self {
            state,
            pages: BTreeMap::new(),
            fail_alloc: None,
            fail_map: None,
        }
    }

    fn byte(&self, address: u64) -> u8 {
        self.pages[&(address & !4095)].0.bytes[(address & 4095) as usize]
    }

    fn word(&self, address: u64) -> u64 {
        u64::from_le_bytes(core::array::from_fn(|index| {
            self.byte(address + index as u64)
        }))
    }
}
impl<'a> UserAddressSpace for Space<'a> {
    type Error = u8;
    type Page = Page<'a>;
    fn allocate_zeroed_page(&self) -> Result<Self::Page, u8> {
        self.state.allocs.set(self.state.allocs.get() + 1);
        if self.fail_alloc == Some(self.state.allocs.get()) {
            return Err(1);
        }
        self.state.live.set(self.state.live.get() + 1);
        Ok(Page {
            bytes: Box::new([0; PAGE_SIZE]),
            state: self.state,
        })
    }
    fn map_page(
        &mut self,
        page: Self::Page,
        address: u64,
        permissions: PtePermissions,
    ) -> Result<(), u8> {
        self.state.maps.set(self.state.maps.get() + 1);
        if self.fail_map == Some(self.state.maps.get()) || self.pages.contains_key(&address) {
            return Err(2);
        }
        assert!(permissions.valid_user_mapping());
        self.pages.insert(address, (page, permissions));
        Ok(())
    }
    fn unmap(&mut self, address: u64) -> Result<Self::Page, u8> {
        self.pages.remove(&address).map(|(page, _)| page).ok_or(3)
    }
    fn protect(&mut self, address: u64, permissions: PtePermissions) -> Result<(), u8> {
        self.pages.get_mut(&address).ok_or(3)?.1 = permissions;
        Ok(())
    }
    fn translate(&self, address: u64) -> Result<Option<PageInfo>, u8> {
        Ok(self
            .pages
            .get(&(address & !4095))
            .map(|(_, permissions)| PageInfo {
                physical_address: 0,
                permissions: *permissions,
            }))
    }
}

#[test]
fn eager_pages_match_moss_file_prefix_bss_permissions_and_auxv() {
    let bytes = binary();
    let state = State::default();
    let loaded = Image::parse(&bytes, BASE, END)
        .unwrap()
        .load(Space::new(&state), &["probe", "arg"], &["KEY=value"])
        .unwrap();
    assert_eq!(loaded.entry, ENTRY);
    assert_eq!(loaded.load_pages, 6);
    assert_eq!(state.live.get(), 7);
    let space = &loaded.space;
    for (address, permissions) in [
        (BASE, PtePermissions::ro(true)),
        (ENTRY, PtePermissions::rx(true)),
        (DATA, PtePermissions::rw(true)),
        (END - 1, PtePermissions::rw(true)),
    ] {
        assert_eq!(
            space.translate(address).unwrap().unwrap().permissions,
            permissions
        );
    }
    assert_eq!(space.byte(ENTRY - 1), 0xdd);
    for offset in 0..16 {
        assert_eq!(space.byte(ENTRY + offset), 0xcc);
    }
    assert_eq!(space.byte(ENTRY + 16), 0);
    for offset in 0..32 {
        assert_eq!(space.byte(DATA + offset), offset as u8 + 1);
    }
    for address in DATA + 32..BASE + 0x7000 {
        assert_eq!(space.byte(address), 0);
    }
    assert_eq!(loaded.stack % 16, 0);
    assert_eq!(space.word(loaded.stack), 2);
    for (index, pair) in [
        (AT_PHNUM, 3),
        (AT_PHENT, 56),
        (AT_ENTRY, ENTRY),
        (AT_PHDR, BASE + 64),
        (AT_PAGESZ, 4096),
        (AT_NULL, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = loaded.stack + 48 + index as u64 * 16;
        assert_eq!((space.word(offset), space.word(offset + 8)), pair);
    }
    drop(loaded);
    assert_eq!(state.live.get(), 0);
}

#[test]
fn unmapped_header_table_is_not_advertised_as_at_phdr() {
    let mut bytes = binary();
    put32(&mut bytes, 64, 4);
    let state = State::default();
    let loaded = Image::parse(&bytes, BASE, END)
        .unwrap()
        .load(Space::new(&state), &[], &[])
        .unwrap();
    assert_eq!(loaded.space.word(loaded.stack + 24 + 3 * 16), AT_PAGESZ);
}

#[test]
fn every_allocation_and_map_failure_drops_the_unpublished_space_including_stack() {
    let bytes = binary();
    let image = Image::parse(&bytes, BASE, END).unwrap();
    for index in 1..=7 {
        for map_failure in [false, true] {
            let state = State::default();
            let mut space = Space::new(&state);
            if map_failure {
                space.fail_map = Some(index);
            } else {
                space.fail_alloc = Some(index);
            }
            assert_eq!(
                image.load(space, &[], &[]).err(),
                Some(Error::Memory(if map_failure { 2 } else { 1 }))
            );
            assert_eq!(state.live.get(), 0);
        }
    }
}

#[test]
fn invalid_stack_and_collision_reclaim_scratch_without_touching_a_live_image() {
    let bytes = binary();
    let image = Image::parse(&bytes, BASE, END).unwrap();
    let state = State::default();
    let old = image.load(Space::new(&state), &[], &[]).unwrap();
    for epoch in 0..16 {
        assert!(matches!(
            image.load(Space::new(&state), &["bad\0arg"], &[]),
            Err(Error::Stack(_))
        ));
        assert_eq!(state.live.get(), 7);
        let mut scratch = Space::new(&state);
        let page = scratch.allocate_zeroed_page().unwrap();
        scratch
            .map_page(page, BASE, PtePermissions::rw(true))
            .unwrap();
        assert_eq!(image.load(scratch, &[], &[]).err(), Some(Error::Memory(2)));
        assert_eq!(state.live.get(), 7, "{epoch}");
        assert_eq!(old.space.byte(ENTRY), 0xcc);
    }
    drop(old);
    assert_eq!(state.live.get(), 0);
}
