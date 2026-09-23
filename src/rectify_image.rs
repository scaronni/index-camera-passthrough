//! Run the processing of the pipeline on a saved camera frame, without a VR runtime,
//! to inspect the results.

use std::{path::Path, sync::Arc};

use anyhow::{anyhow, Context, Result};
use nalgebra::{Matrix4, Point3, Vector3};
use vulkano::{
    buffer::{BufferCreateInfo, BufferUsage, Subbuffer},
    command_buffer::{
        allocator::{CommandBufferAllocator, StandardCommandBufferAllocator},
        AutoCommandBufferBuilder, CommandBufferUsage, CopyImageToBufferInfo,
        PrimaryCommandBufferAbstract,
    },
    descriptor_set::allocator::{
        StandardDescriptorSetAllocator, StandardDescriptorSetAllocatorCreateInfo,
    },
    device::{Device, DeviceCreateInfo, Queue, QueueCreateInfo, QueueFlags},
    image::{Image, ImageLayout},
    instance::{Instance, InstanceCreateInfo},
    memory::allocator::MemoryTypeFilter,
    sync::GpuFuture,
};

use crate::{
    depth::{DepthEstimator, DEPTH_SIZE},
    utils::DeviceExt,
    CAMERA_SIZE,
};

/// Where to write the results of [`rectify_image`].
pub struct Outputs<'a> {
    /// The rectified frame.
    pub rectified: &'a Path,
    /// The disparity map, as a 16 bit image of the disparities in 1/16 pixels.
    pub depth: Option<&'a Path>,
    /// What each eye sees on an overlay 1 m ahead, with the scene at the distance of the
    /// overlay (top) and at the depth of the center of the view (bottom).
    pub projection: Option<&'a Path>,
}

/// Rectify `input`, the left and right camera images side by side as captured from
/// the camera, and write the results.
pub fn rectify_image(input: &Path, outputs: Outputs<'_>) -> Result<()> {
    let camera_config = crate::steam::find_steam_config().context("no camera calibration found")?;
    let image = image::open(input).with_context(|| format!("cannot read {}", input.display()))?;
    let frame = image.to_rgba8();
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

    let mut estimator = DepthEstimator::new(&camera_config)?;
    let start = std::time::Instant::now();
    let disparity = estimator.compute_gray(image.to_luma8().as_raw())?.to_vec();
    let center_depth = estimator.center_depth();
    log::info!("Disparity computed in {:?}", start.elapsed());
    let with_depth = disparity
        .iter()
        .filter(|&&d| estimator.depth(d).is_some())
        .count();
    log::info!(
        "{:.0}% of the points have a depth, the center of the view is at {center_depth:?} m",
        with_depth as f64 / disparity.len() as f64 * 100.0
    );
    if let Some(output) = outputs.depth {
        // Points without a match are 0.
        let pixels: Vec<u16> = disparity.iter().map(|&d| d.max(0) as u16).collect();
        image::ImageBuffer::<image::Luma<u16>, _>::from_raw(
            DEPTH_SIZE as u32,
            DEPTH_SIZE as u32,
            pixels,
        )
        .unwrap()
        .save(output)
        .with_context(|| format!("cannot write {}", output.display()))?;
    }

    let (device, queue) = create_device()?;
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
        descriptor_set_allocator.clone(),
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
    let pixels = read_image(&device, &queue, cmdbuf_allocator.clone(), rectified.clone())?;
    save(outputs.rectified, &pixels, CAMERA_SIZE)?;

    if let Some(output) = outputs.projection {
        // The overlay 1 m in front of the Hmd, with the eyes 63 mm apart.
        let hmd = Matrix4::identity();
        let overlay = Matrix4::new_translation(&Vector3::new(0.0, 0.0, -1.0));
        let eyes = [
            Point3::new(-0.0315, 0.0, 0.0),
            Point3::new(0.0315, 0.0, 0.0),
        ];
        let mut projection = crate::projection::Projection::new(
            device.clone(),
            allocator.clone(),
            descriptor_set_allocator,
            &rectified,
            1.0,
            &Some(camera_config),
            ImageLayout::TransferSrcOptimal,
        )?;
        let mut pixels = Vec::new();
        for depth in [None, center_depth] {
            let projected = crate::create_submittable_image(device.clone())?;
            projection.update_mvps(&overlay, &pipeline.fov(), &hmd, &eyes, depth)?;
            projection
                .project(
                    allocator.clone(),
                    cmdbuf_allocator.clone(),
                    vulkano::sync::now(device.clone()),
                    &queue,
                    projected.clone(),
                )?
                .then_signal_fence_and_flush()?
                .wait(None)?;
            pixels.extend(read_image(
                &device,
                &queue,
                cmdbuf_allocator.clone(),
                projected,
            )?);
        }
        save(output, &pixels, CAMERA_SIZE * 2)?;
    }
    Ok(())
}

fn create_device() -> Result<(Arc<Device>, Arc<Queue>)> {
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
    Ok((device, queues.next().unwrap()))
}

/// Pixels of an RGBA image of the size of a camera frame.
fn read_image(
    device: &Arc<Device>,
    queue: &Arc<Queue>,
    cmdbuf_allocator: Arc<dyn CommandBufferAllocator>,
    image: Arc<Image>,
) -> Result<Vec<u8>> {
    let buffer = device.clone().new_buffer(
        BufferCreateInfo {
            size: (CAMERA_SIZE * 2 * CAMERA_SIZE * 4) as u64,
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
    cmdbuf.copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(image, buffer.clone()))?;
    cmdbuf
        .build()?
        .execute(queue.clone())?
        .then_signal_fence_and_flush()?
        .wait(None)?;
    let pixels = buffer.read()?.to_vec();
    Ok(pixels)
}

fn save(output: &Path, pixels: &[u8], height: u32) -> Result<()> {
    image::save_buffer(
        output,
        pixels,
        CAMERA_SIZE * 2,
        height,
        image::ColorType::Rgba8,
    )
    .with_context(|| format!("cannot write {}", output.display()))
}
