use super::*;
use moss_libkernel::memory::{PAGE_SIZE, allocators::frame::Frame};

pub fn allocation(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let device = Device::new(boot, frames)?;
    device.set_mask(u64::MAX).unwrap();
    device.set_mask(u32::MAX as u64).unwrap();
    let before = frames.stats();
    for (size, align) in [
        (1, 1),
        (16, 16),
        (4097, 4096),
        (1, 16384),
        (65536, 65536),
        (dma_state::MAX_BYTES, dma_state::MAX_BYTES),
    ] {
        let mut buffer = device.allocate(size, align).unwrap();
        let cpu = buffer.cpu_address().unwrap().as_ptr() as u64;
        let dma = buffer.address().value();
        assert_ne!(cpu, dma);
        assert_eq!(cpu, boot.hhdm_offset + dma);
        assert!(cpu.is_multiple_of(align as u64) && dma.is_multiple_of(align as u64));
        assert_eq!(buffer.size(), size);
        assert_eq!(
            buffer.capacity(),
            Layout::new(size, align).unwrap().capacity
        );
        assert!(dma + buffer.capacity() as u64 - 1 <= u32::MAX as u64);
        assert!(buffer.bytes().unwrap().iter().all(|&byte| byte == 0));
        // The complete buddy block is exclusively CPU-owned and fully initialized.
        let backing = unsafe {
            core::slice::from_raw_parts(buffer.cpu_address().unwrap().as_ptr(), buffer.capacity())
        };
        assert!(backing.iter().all(|&byte| byte == 0));
        buffer.bytes_mut().unwrap().fill(0x5a);
        assert!(buffer.bytes().unwrap().iter().all(|&byte| byte == 0x5a));
        assert_eq!(
            device.set_mask(u64::MAX),
            Err(Error::Policy(dma_state::Error::Busy))
        );
        buffer.try_release().unwrap();
        assert_eq!(
            buffer.cpu_address(),
            Err(Error::Policy(dma_state::Error::Released))
        );
        assert_eq!(
            buffer.try_release(),
            Err(Error::Policy(dma_state::Error::Released))
        );
        assert_eq!(frames.stats(), before);
    }
    for (size, align) in [(0, 1), (1, 0), (1, 3), (dma_state::MAX_BYTES + 1, 1)] {
        assert!(matches!(
            device.allocate(size, align),
            Err(Error::Policy(dma_state::Error::Layout))
        ));
        assert_eq!(frames.stats(), before);
    }
    let state = device.snapshot();
    assert_eq!(
        device.set_mask(0xffff),
        Err(Error::Policy(dma_state::Error::Mask))
    );
    assert_eq!(device.snapshot(), state);

    let donated = frames
        .alloc_frames(2)
        .map_err(|error| Failure::new("dma_donate", error as i32))?;
    let mut metadata = [const { Frame::EMPTY }; 4];
    let limited = memory::donated_allocator(donated.region(), &mut metadata)?;
    let bounded = Device::bind(&limited, device.inner.profile);
    let before = limited.stats();
    let mut buffers: [Option<Buffer<'_, '_>>; 4] =
        core::array::from_fn(|_| Some(bounded.allocate(16, 16).unwrap()));
    assert!(matches!(
        bounded.allocate(16, 16),
        Err(Error::Policy(dma_state::Error::NoMemory))
    ));
    assert_eq!(bounded.snapshot().buffers, 4);
    let expected = buffers[1].as_ref().unwrap().address();
    buffers[1].as_mut().unwrap().bytes_mut().unwrap().fill(0xa5);
    drop(buffers[1].take());
    let reused = bounded.allocate(16, 16).unwrap();
    assert_eq!(reused.address(), expected);
    assert!(reused.bytes().unwrap().iter().all(|&byte| byte == 0));
    drop(reused);
    drop(buffers);
    assert_eq!(limited.stats(), before);
    assert_eq!(bounded.snapshot().buffers, 0);
    limited
        .check_invariants()
        .map_err(|error| Failure::new("dma_frame_invariants", error as i32))?;
    assert!(device.inner.profile.device.bus_mastering_disabled());
    Ok(())
}

pub fn ownership(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let device = Device::new(boot, frames)?;
    let other = Device::bind(frames, device.inner.profile);
    let before = frames.stats();
    let mut first = device.allocate(64, 16).unwrap();
    let mut second = device.allocate(PAGE_SIZE, PAGE_SIZE).unwrap();
    first.bytes_mut().unwrap().fill(0x5a);
    first.publish().unwrap();
    assert_eq!(
        first.cpu_address(),
        Err(Error::Policy(dma_state::Error::Busy))
    );
    assert_eq!(
        first.try_release(),
        Err(Error::Policy(dma_state::Error::Busy))
    );
    assert_eq!(first.publish(), Err(Error::Policy(dma_state::Error::Busy)));
    // Bus mastering has never been enabled; these test publications cannot DMA.
    let old = unsafe { device.quiesced() }.unwrap();
    second.publish().unwrap();
    assert_eq!(
        first.reclaim(&old),
        Err(Error::Policy(dma_state::Error::Stale))
    );
    let foreign = unsafe { other.quiesced() }.unwrap();
    assert_eq!(
        first.reclaim(&foreign),
        Err(Error::Policy(dma_state::Error::Stale))
    );
    let witness = unsafe { device.quiesced() }.unwrap();
    first.reclaim(&witness).unwrap();
    second.reclaim(&witness).unwrap();
    assert!(first.bytes().unwrap().iter().all(|&byte| byte == 0x5a));
    assert_eq!(
        first.reclaim(&witness),
        Err(Error::Policy(dma_state::Error::Stale))
    );
    drop(first);
    drop(second);
    assert_eq!(device.snapshot().buffers, 0);
    assert_eq!(frames.stats(), before);
    assert!(device.inner.profile.device.bus_mastering_disabled());
    Ok(())
}
