//! Run the lens correction of the pipeline on a saved camera frame, without a VR
//! runtime, to inspect the rectified images.

use std::{path::Path, sync::Arc};

use anyhow::{anyhow, Context, Result};
use vulkano::{
    buffer::{BufferCreateInfo, BufferUsage, Subbuffer},
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder, CommandBufferUsage,
        CopyImageToBufferInfo, PrimaryCommandBufferAbstract,
    },
    descriptor_set::allocator::{
        StandardDescriptorSetAllocator, StandardDescriptorSetAllocatorCreateInfo,
    },
    device::{Device, DeviceCreateInfo, QueueCreateInfo, QueueFlags},
    instance::{Instance, InstanceCreateInfo},
    memory::allocator::MemoryTypeFilter,
    sync::GpuFuture,
};

use crate::{utils::DeviceExt, CAMERA_SIZE};

/// Rectify `input`, the left and right camera images side by side as captured from
/// the camera, and write the result to `output`.
pub fn rectify_image(input: &Path, output: &Path) -> Result<()> {
    let camera_config = crate::steam::find_steam_config().context("no camera calibration found")?;
    let frame = image::open(input)
        .with_context(|| format!("cannot read {}", input.display()))?
        .into_rgba8();
    if frame.dimensions() != (CAMERA_SIZE * 2, CAMERA_SIZE) {
        return Err(anyhow!(
            "{} is {}x{}, expected {}x{}",
            input.display(),
            frame.width(),
            frame.height(),
            CAMERA_SIZE * 2,
            CAMERA_SIZE
        ));
    }

    let library = crate::vrapi::get_vulkan_library().clone();
    let instance = Instance::new(
        library.clone(),
        InstanceCreateInfo {
            // For the debug utils object names set by the pipeline.
            enabled_extensions: *library.supported_extensions(),
            ..Default::default()
        },
    )?;
    let (physical_device, queue_family) = instance
        .enumerate_physical_devices()?
        .find_map(|device| {
            let family = device
                .queue_family_properties()
                .iter()
                .position(|family| family.queue_flags.contains(QueueFlags::GRAPHICS))?;
            Some((device, family as u32))
        })
        .context("no Vulkan device with a graphics queue")?;
    log::info!("Using {}", physical_device.properties().device_name);
    let (device, mut queues) = Device::new(
        physical_device,
        DeviceCreateInfo {
            queue_create_infos: vec![QueueCreateInfo {
                queue_family_index: queue_family,
                ..Default::default()
            }],
            ..Default::default()
        },
    )?;
    let queue = queues.next().unwrap();
    let allocator = Arc::new(device.clone().host_to_device_allocator());
    let descriptor_set_allocator = Arc::new(StandardDescriptorSetAllocator::new(
        device.clone(),
        StandardDescriptorSetAllocatorCreateInfo::default(),
    ));
    let cmdbuf_allocator = Arc::new(StandardCommandBufferAllocator::new(
        device.clone(),
        Default::default(),
    ));

    let mut pipeline = crate::pipeline::Pipeline::new(
        device.clone(),
        allocator.clone(),
        descriptor_set_allocator,
        false,
        Some(camera_config),
    )?;
    let rectified = crate::create_submittable_image(device.clone())?;
    pipeline
        .run(
            &queue,
            allocator.clone(),
            cmdbuf_allocator.clone(),
            frame.as_raw(),
            rectified.clone(),
        )?
        .then_signal_fence_and_flush()?
        .wait(None)?;

    let buffer = Device::new_buffer(
        device,
        BufferCreateInfo {
            size: frame.as_raw().len() as u64,
            usage: BufferUsage::TRANSFER_DST,
            ..Default::default()
        },
        MemoryTypeFilter::HOST_RANDOM_ACCESS,
    )?;
    let buffer = Subbuffer::new(buffer);
    let mut cmdbuf = AutoCommandBufferBuilder::primary(
        cmdbuf_allocator,
        queue.queue_family_index(),
        CommandBufferUsage::OneTimeSubmit,
    )?;
    cmdbuf.copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(
        rectified,
        buffer.clone(),
    ))?;
    cmdbuf
        .build()?
        .execute(queue)?
        .then_signal_fence_and_flush()?
        .wait(None)?;

    image::save_buffer(
        output,
        &buffer.read()?,
        CAMERA_SIZE * 2,
        CAMERA_SIZE,
        image::ColorType::Rgba8,
    )
    .with_context(|| format!("cannot write {}", output.display()))?;
    Ok(())
}
