//! The overlay in VR space can be seen as a "portal" to the real world. By projecting it to the
//! Index camera's clipping space, using the camera's projection matrix, we can decide which
//! portion of the camera's view can be seen through this portal.
//!
//! Overlay vertex * Overlay Model * HMD View * Camera Project -> Texture coordinates used to
//! sample the camera's view.
//!
//! Overlay vertex: calculate based on Overlay width we set
//! Overlay Model: the overlay transform matrix we set
//! HMD View: inverse of HMD pose
//! Camera Project: estimated from camera calibration.
use anyhow::Result;
use std::sync::Arc;
use vulkano::{
    buffer::{
        AllocateBufferError, Buffer, BufferContents, BufferCreateInfo, BufferUsage, Subbuffer,
    },
    command_buffer::{
        allocator::CommandBufferAllocator, AutoCommandBufferBuilder, CommandBufferExecError,
        CommandBufferUsage::OneTimeSubmit, RenderPassBeginInfo, SubpassBeginInfo, SubpassContents,
        SubpassEndInfo,
    },
    descriptor_set::{allocator::DescriptorSetAllocator, DescriptorSet, WriteDescriptorSet},
    device::{Device, Queue},
    image::view::{ImageView, ImageViewCreateInfo},
    image::{
        sampler::{Filter, Sampler, SamplerCreateInfo},
        Image, ImageLayout,
    },
    memory::allocator::{
        AllocationCreateInfo, MemoryAllocatePreference, MemoryAllocator, MemoryTypeFilter,
    },
    pipeline::{
        graphics::{
            color_blend::ColorBlendState,
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            multisample::MultisampleState,
            rasterization::RasterizationState,
            vertex_input::{Vertex as VertexTrait, VertexDefinition},
            viewport::{Viewport, ViewportState},
            GraphicsPipelineCreateInfo,
        },
        layout::{IntoPipelineLayoutCreateInfoError, PipelineDescriptorSetLayoutCreateInfo},
        DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint, PipelineLayout,
        PipelineShaderStageCreateInfo,
    },
    render_pass::{Framebuffer, RenderPass, Subpass},
    sync::{GpuFuture, HostAccessError},
    Validated, VulkanError,
};
mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "shaders/projection.vert",
        custom_derives: [Copy, Clone, Debug, Default],
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "shaders/projection.frag",
        custom_derives: [Copy, Clone, Debug],
    }
}

#[derive(PartialEq, Debug)]
pub struct ProjectionParameters {
    pub overlay_width: f32,
    /// MVP matrices for the left and right eye, respectively.
    pub mvps: [Matrix4<f32>; 2],
}

struct Uniforms {
    transforms: [Subbuffer<vs::Transform>; 2],
}

impl std::fmt::Debug for Uniforms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Uniforms")
            .field("transforms", &())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct Projection {
    extent: [u32; 2],
    pipeline: Arc<GraphicsPipeline>,
    render_pass: Arc<RenderPass>,
    // [0: left, 1: right]
    uniforms: Uniforms,
    saved_parameters: ProjectionParameters,
    rectification: Option<Rectification>,
    mvps_changed: bool,
    desc_sets: [Arc<DescriptorSet>; 2],
}
use crate::{rectification::Rectification, utils::Array};
#[derive(VertexTrait, Default, Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Vertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
    #[format(R32G32B32_SFLOAT)]
    in_tex_coord: [f32; 3],
}

#[allow(dead_code)]
fn format_matrix<
    A: Scalar + ToString,
    B: nalgebra::Dim,
    C: nalgebra::Dim,
    D: RawStorage<A, B, C>,
>(
    m: &nalgebra::Matrix<A, B, C, D>,
) -> String {
    use itertools::Itertools;
    format!(
        "numpy.matrix([{}])",
        m.row_iter()
            .map(|r| {
                let it = r.into_iter();
                format!("[{}]", it.map(|v| v.to_string()).join(","))
            })
            .join(",")
    )
}

#[derive(thiserror::Error, Debug)]
pub enum ProjectorError {
    #[error("input image is not square {0}x{1}")]
    NotSquare(u32, u32),
    #[error("vulkan error {0}")]
    Vulkan(#[from] Validated<VulkanError>),
    #[error("{0}")]
    CreateInfo(#[from] IntoPipelineLayoutCreateInfoError),
    #[error("buffer allocation error: {0}")]
    BufferAlloc(#[from] Validated<AllocateBufferError>),
    #[error("host access error: {0}")]
    HostAccess(#[from] HostAccessError),
    #[error("command buffer execution error: {0}")]
    CommandBuffer(#[from] CommandBufferExecError),
}

impl From<Box<vulkano::ValidationError>> for ProjectorError {
    fn from(value: Box<vulkano::ValidationError>) -> Self {
        Self::Vulkan(Validated::from(value))
    }
}

use nalgebra::{matrix, Matrix4, Point3, RawStorage, Scalar};

/// Transform, in the frame of the overlay, from the points of the overlay to the points
/// of the scene that `eye` sees through them, when the scene is a plane parallel to the
/// overlay through `target`. Both points are given in world space.
///
/// Seen from the eye, the overlay and the scene plane are the same picture at different
/// scales: this is a scaling around the eye.
fn scene_plane(
    overlay_transform: &Matrix4<f32>,
    eye: &Point3<f32>,
    target: &Point3<f32>,
) -> Matrix4<f32> {
    let world_to_overlay = overlay_transform
        .try_inverse()
        .expect("overlay transform not invertible");
    let eye = world_to_overlay.transform_point(eye);
    let target = world_to_overlay.transform_point(target);
    // The overlay faces +z. An eye behind it, or a scene plane behind the eye, cannot be
    // projected.
    if eye.z <= 0.0 || target.z >= eye.z {
        return Matrix4::identity();
    }
    let scale = (eye.z - target.z) / eye.z;
    Matrix4::new_translation(&eye.coords)
        * Matrix4::new_scaling(scale)
        * Matrix4::new_translation(&-eye.coords)
}
impl Projection {
    /// Calculate the MVP of the rectified camera images, for each eye.
    ///
    /// # Arguments
    ///
    /// - overlay_transform: pose of the overlay in world space
    /// - fov: focal length of the rectified images divided by their size
    /// - hmd_transform: pose of the Hmd in world space
    ///
    /// Each eye sees, through each point of the overlay, the scene on a plane parallel
    /// to the overlay: the overlay itself, or with `depth`, the plane through the point
    /// at that distance straight ahead of the left rectified camera. Objects on that
    /// plane are shown where they really are.
    pub(crate) fn update_mvps(
        &mut self,
        overlay_transform: &Matrix4<f32>,
        fov: &[[f32; 2]; 2],
        hmd_transform: &Matrix4<f32>,
        eyes: &[Point3<f32>; 2],
        depth: Option<f32>,
    ) -> Result<(), ProjectorError> {
        // Poses of the rectified cameras in the Hmd frame. Without calibration, assume
        // cameras at the center of the Hmd, looking straight ahead.
        let cameras = self
            .rectification
            .map(|r| {
                r.camera_to_head
                    .map(|pose| pose.to_homogeneous().cast::<f32>())
            })
            .unwrap_or([Matrix4::identity(); 2]);
        let scene_planes = match depth {
            Some(depth) => {
                let target =
                    (hmd_transform * cameras[0]).transform_point(&Point3::new(0.0, 0.0, -depth));
                eyes.map(|eye| scene_plane(overlay_transform, &eye, &target))
            }
            None => [Matrix4::identity(); 2],
        };
        let [left_eye, right_eye] = cameras.map(|camera| hmd_transform * camera);
        let left_view = left_eye
            .try_inverse()
            .expect("HMD transform not invertable?");
        let right_view = right_eye
            .try_inverse()
            .expect("HMD transform not invertable?");

        // X gets fov / 2.0 because the source texture is a side-by-side stereo texture
        // X translation element is used to map them to left/right side of the texture,
        // respectively.
        //
        let camera_projection_left = matrix![
            fov[0][0] / 2.0, 0.0, 0.0, 0.0;
            0.0, fov[0][1], 0.0, 0.0;
            0.0, 0.0, -1.0, 0.0;
            0.0, 0.0, 0.0, 1.0;
        ];
        let camera_projection_right = matrix![
            fov[1][0] / 2.0, 0.0, 0.0, 0.0;
            0.0, fov[1][1] , 0.0, 0.0;
            0.0, 0.0, -1.0, 0.0;
            0.0, 0.0, 0.0, 1.0;
        ];
        self.set_mvps([
            (camera_projection_left * left_view * overlay_transform * scene_planes[0]).cast(),
            (camera_projection_right * right_view * overlay_transform * scene_planes[1]).cast(),
        ]);
        Ok(())
    }
    fn set_mvps(&mut self, mvps: [Matrix4<f32>; 2]) {
        if self.saved_parameters.mvps == mvps {
            return;
        }
        self.saved_parameters.mvps = mvps;
        self.mvps_changed = true;
    }
    pub fn recalculate_uniforms(&mut self) -> Result<(), ProjectorError> {
        if !self.mvps_changed {
            return Ok(());
        }
        for (mvp, uniform) in self
            .saved_parameters
            .mvps
            .iter()
            .zip(&self.uniforms.transforms)
        {
            uniform.write()?.mvp = *mvp.as_ref();
        }
        self.mvps_changed = false;
        Ok(())
    }
    fn make_uniform_buffer<T: BufferContents>(
        allocator: Arc<dyn MemoryAllocator>,
        uniform: T,
    ) -> Result<Subbuffer<T>, Validated<AllocateBufferError>> {
        log::debug!("uniform buffer size {}", std::mem::size_of::<T>());
        Buffer::from_data(
            allocator,
            BufferCreateInfo {
                usage: BufferUsage::UNIFORM_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::HOST_SEQUENTIAL_WRITE
                    | MemoryTypeFilter::PREFER_DEVICE,
                allocate_preference: MemoryAllocatePreference::Unknown,
                ..Default::default()
            },
            uniform,
        )
    }
    pub fn new(
        device: Arc<Device>,
        allocator: Arc<dyn MemoryAllocator>,
        descriptor_set_allocator: Arc<dyn DescriptorSetAllocator>,
        source: &Arc<Image>,
        overlay_width: f32,
        camera_calib: &Option<crate::vrapi::StereoCamera>,
        final_layout: ImageLayout,
    ) -> Result<Self, ProjectorError> {
        let [w, h, _] = source.extent();
        if w != h * 2 {
            return Err(ProjectorError::NotSquare(w, h));
        }
        let vs = vs::load(device.clone())?;
        let fs = fs::load(device.clone())?;
        let render_pass = vulkano::single_pass_renderpass!(device.clone(),
            attachments: {
                color: {
                    format: vulkano::format::Format::R8G8B8A8_UNORM,
                    samples: 1,
                    load_op: Load,
                    store_op: Store,
                    final_layout: final_layout,
                }
            },
            pass: {
                color: [color],
                depth_stencil: {}
            }
        )
        .unwrap();
        let tex_offsets = (0..2)
            .map(|i| {
                Self::make_uniform_buffer(
                    allocator.clone(),
                    fs::Info {
                        texOffset: [0.5 * (i as f32), 0.0],
                    },
                )
            })
            .collect::<Result<Array<_, 2>, _>>()?
            .into_inner();
        let vs = vs.entry_point("main").unwrap();
        let fs = fs.entry_point("main").unwrap();
        let stages = [
            PipelineShaderStageCreateInfo::new(vs.clone()),
            PipelineShaderStageCreateInfo::new(fs),
        ];
        let layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
                .into_pipeline_layout_create_info(device.clone())?,
        )?;
        let mut transforms = (0..2)
            .map(|_| Self::make_uniform_buffer(allocator.clone(), vs::Transform::default()))
            .collect::<Result<Array<_, 2>, _>>()?
            .into_inner();
        for transform in &mut transforms {
            let mut transform_write = transform.write()?;
            transform_write.overlayWidth = overlay_width.into();
        }
        log::info!("before");
        let pipeline = GraphicsPipeline::new(
            device.clone(),
            None,
            GraphicsPipelineCreateInfo {
                vertex_input_state: Some(
                    Vertex::per_vertex()
                        .definition(&vs)
                        .map_err(Validated::<VulkanError>::from)?,
                ),
                stages: stages.into_iter().collect(),
                input_assembly_state: Some(InputAssemblyState {
                    topology: PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                }),
                viewport_state: Some(ViewportState::default()),
                dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                subpass: Some(Subpass::from(render_pass.clone(), 0).unwrap().into()),
                rasterization_state: Some(RasterizationState::default()),
                multisample_state: Some(MultisampleState::default()),
                color_blend_state: Some(ColorBlendState::with_attachment_states(
                    1,
                    Default::default(),
                )),
                ..GraphicsPipelineCreateInfo::layout(layout)
            },
        )?;
        log::info!("after");
        let init_params = ProjectionParameters {
            overlay_width,
            mvps: [Matrix4::identity(), Matrix4::identity()],
        };
        let layout = pipeline.layout().set_layouts().first().unwrap();
        let sampler = Sampler::new(
            device,
            SamplerCreateInfo {
                min_filter: Filter::Linear,
                mag_filter: Filter::Linear,
                ..Default::default()
            },
        )?;
        let desc_sets = (0..2)
            .map(|i| {
                DescriptorSet::new(
                    descriptor_set_allocator.clone(),
                    layout.clone(),
                    [
                        WriteDescriptorSet::buffer(0, transforms[i].clone()),
                        WriteDescriptorSet::image_view_sampler(
                            1,
                            ImageView::new(
                                source.clone(),
                                ImageViewCreateInfo::from_image(source),
                            )?,
                            sampler.clone(),
                        ),
                        WriteDescriptorSet::buffer(2, tex_offsets[i].clone()),
                    ],
                    None,
                )
                .map_err(ProjectorError::from)
            })
            .collect::<Result<Array<_, 2>, _>>()?
            .into_inner();
        let source_extent = source.extent();
        Ok(Self {
            saved_parameters: init_params,
            uniforms: Uniforms { transforms },
            desc_sets,
            render_pass,
            pipeline,
            extent: [source_extent[0], source_extent[1]],
            rectification: camera_calib
                .as_ref()
                .map(|calib| Rectification::new(calib, crate::rectification::RECTIFIED_FOV)),
            mvps_changed: true,
        })
    }
    pub fn project(
        &mut self,
        allocator: Arc<dyn MemoryAllocator>,
        cmdbuf_allocator: Arc<dyn CommandBufferAllocator>,
        after: impl GpuFuture,
        queue: &Arc<Queue>,
        output: Arc<Image>,
    ) -> Result<impl GpuFuture, ProjectorError> {
        self.recalculate_uniforms()?;
        let framebuffer = Framebuffer::new(
            self.render_pass.clone(),
            vulkano::render_pass::FramebufferCreateInfo {
                attachments: vec![ImageView::new(
                    output.clone(),
                    ImageViewCreateInfo::from_image(&output),
                )?],
                ..Default::default()
            },
        )?;
        let ProjectionParameters { overlay_width, .. } = &self.saved_parameters;
        let [w, h] = self.extent;
        let mut cmdbuf = AutoCommandBufferBuilder::primary(
            cmdbuf_allocator,
            queue.queue_family_index(),
            OneTimeSubmit,
        )?;
        //cmdbuf.copy_image(CopyImageInfo::images(self.source.clone(), output.clone()))?;

        // Y is flipped from the vertex Y because texture coordinate is top-down
        let vertex_buffer = Buffer::from_iter::<Vertex, _>(
            allocator,
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::HOST_SEQUENTIAL_WRITE
                    | MemoryTypeFilter::PREFER_DEVICE,
                allocate_preference: MemoryAllocatePreference::Unknown,
                ..Default::default()
            },
            [
                Vertex {
                    position: [-1.0, -1.0],
                    in_tex_coord: [-overlay_width / 2.0, overlay_width / 2.0, 0.0],
                },
                Vertex {
                    position: [-1.0, 1.0],
                    in_tex_coord: [-overlay_width / 2.0, -overlay_width / 2.0, 0.0],
                },
                Vertex {
                    position: [1.0, -1.0],
                    in_tex_coord: [overlay_width / 2.0, overlay_width / 2.0, 0.0],
                },
                Vertex {
                    position: [1.0, 1.0],
                    in_tex_coord: [overlay_width / 2.0, -overlay_width / 2.0, 0.0],
                },
            ]
            .iter()
            .cloned(),
        )
        .unwrap();
        // Left

        let mut render_pass_begin_info = RenderPassBeginInfo::framebuffer(framebuffer.clone());
        render_pass_begin_info.clear_values = vec![None];
        cmdbuf
            .begin_render_pass(
                render_pass_begin_info,
                SubpassBeginInfo {
                    contents: SubpassContents::Inline,
                    ..Default::default()
                },
            )?
            .set_viewport(
                0,
                Some(Viewport {
                    offset: [0.0, 0.0],
                    extent: [(w / 2) as f32, h as f32],
                    depth_range: 0.0..=1.0,
                })
                .into_iter()
                .collect(),
            )?
            .bind_pipeline_graphics(self.pipeline.clone())?
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.pipeline.layout().clone(),
                0,
                self.desc_sets[0].clone(),
            )?
            .bind_vertex_buffers(0, vertex_buffer.clone())?;
        // The shaders only sample the bound images and read the bound vertex buffer.
        unsafe { cmdbuf.draw(vertex_buffer.len() as u32, 1, 0, 0) }?
            .end_render_pass(SubpassEndInfo::default())?;

        // Right
        let mut render_pass_begin_info = RenderPassBeginInfo::framebuffer(framebuffer);
        render_pass_begin_info.clear_values = vec![None];
        cmdbuf
            .begin_render_pass(
                render_pass_begin_info,
                SubpassBeginInfo {
                    contents: SubpassContents::Inline,
                    ..Default::default()
                },
            )?
            .set_viewport(
                0,
                Some(Viewport {
                    offset: [(w / 2) as f32, 0.0],
                    extent: [(w / 2) as f32, h as f32],
                    depth_range: 0.0..=1.0,
                })
                .into_iter()
                .collect(),
            )?
            .bind_pipeline_graphics(self.pipeline.clone())?
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.pipeline.layout().clone(),
                0,
                self.desc_sets[1].clone(),
            )?
            .bind_vertex_buffers(0, vertex_buffer.clone())?;
        // The shaders only sample the bound images and read the bound vertex buffer.
        unsafe { cmdbuf.draw(vertex_buffer.len() as u32, 1, 0, 0) }?
            .end_render_pass(SubpassEndInfo::default())?;
        Ok(after.then_execute(queue.clone(), cmdbuf.build()?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Vector3;

    /// A point of the scene plane seen through a point of the overlay is where the line
    /// from the eye through the overlay point meets the plane.
    #[test]
    fn scene_plane_is_seen_through_the_overlay() {
        let overlay = Matrix4::new_translation(&Vector3::new(0.1, -0.05, -1.0));
        for eye in [Point3::new(-0.03, 0.0, 0.0), Point3::new(0.03, 0.02, 0.01)] {
            for depth in [0.4, 1.0, 3.0] {
                let target = Point3::new(0.2, 0.1, -depth);
                let m = overlay * scene_plane(&overlay, &eye, &target);
                for local in [[0.0, 0.0], [0.5, -0.3], [-0.4, 0.2]] {
                    let on_overlay = overlay.transform_point(&Point3::new(local[0], local[1], 0.0));
                    let seen = m.transform_point(&Point3::new(local[0], local[1], 0.0));
                    // On the scene plane...
                    assert!((seen.z + depth).abs() < 1e-4, "{seen}");
                    // ...and on the line from the eye through the overlay point.
                    let a = (on_overlay - eye).normalize();
                    let b = (seen - eye).normalize();
                    assert!((a - b).norm() < 1e-4, "{a} {b}");
                }
            }
        }
    }

    /// At the distance of the overlay, the scene is the overlay itself.
    #[test]
    fn scene_plane_at_the_overlay() {
        let overlay = Matrix4::new_translation(&Vector3::new(0.0, 0.0, -1.0));
        let m = scene_plane(
            &overlay,
            &Point3::new(0.03, 0.0, 0.0),
            &Point3::new(0.5, 0.2, -1.0),
        );
        assert!((m - Matrix4::identity()).norm() < 1e-6);
    }
}
