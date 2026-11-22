use crate::graphics::{
    ComputeBuffer, DrawCommand, FrameUniform, GpuContext, Graphics, InstanceBuffer,
    InstanceBufferHandle, Mesh, UniformBuffer,
};
use crate::material::Material;
use crate::postprocess::PostProcessPass;
use crate::render_pipeline::{
    ColorFormat, DepthFormat, PresentMode, RenderPipeline, ShaderInputFormat,
};
use anyhow::{Result, anyhow, bail};
use glam::Mat4;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use vulkanalia::loader::{LIBRARY, LibloadingLoader};
use vulkanalia::prelude::v1_0::*;
use vulkanalia::vk::{
    ExtDebugUtilsExtensionInstanceCommands, KhrSurfaceExtensionInstanceCommands,
    KhrSwapchainExtensionDeviceCommands,
};
use vulkanalia::window as vk_window;
use winit::window::Window;

const INSTANCE_CAPACITY: usize = 4096;
const VALIDATION_REQUESTED: bool = cfg!(debug_assertions);
const VALIDATION_LAYER: vk::ExtensionName =
    vk::ExtensionName::from_bytes(b"VK_LAYER_KHRONOS_validation");

extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    type_: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _: *mut c_void,
) -> vk::Bool32 {
    let data = unsafe { *data };
    let message = unsafe { CStr::from_ptr(data.message) }.to_string_lossy();
    if severity >= vk::DebugUtilsMessageSeverityFlagsEXT::ERROR {
        log::error!("({:?}) {}", type_, message);
    } else if severity >= vk::DebugUtilsMessageSeverityFlagsEXT::WARNING {
        log::warn!("({:?}) {}", type_, message);
    }
    vk::FALSE
}

#[derive(Clone, Copy)]
struct QueueFamilies {
    graphics: u32,
    present: u32,
}

pub struct App {
    _entry: Entry,
    device: Device,
    graphics_queue: vk::Queue,
    present_queue: vk::Queue,
    swapchain: vk::SwapchainKHR,
    image_views: Vec<vk::ImageView>,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    framebuffers: Vec<vk::Framebuffer>,
    render_pass: vk::RenderPass,
    shadow_render_pass: vk::RenderPass,
    scene_color_target: Option<ColorTarget>,
    post_process: Option<PostProcessState>,
    material_pipelines: HashMap<u64, (vk::PipelineLayout, vk::Pipeline)>,
    graphics_pipeline_layout: vk::PipelineLayout,
    shadow_pipeline_layout: vk::PipelineLayout,
    shadow_pipeline: vk::Pipeline,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    shadow_sampler: vk::Sampler,
    shadow_image: vk::Image,
    shadow_memory: vk::DeviceMemory,
    shadow_array_view: vk::ImageView,
    shadow_layer_views: Vec<vk::ImageView>,
    shadow_framebuffers: Vec<vk::Framebuffer>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    frame_fence: vk::Fence,
    gpu_context: GpuContext,
    frame_instances: InstanceBuffer<Mat4>,
    frame_uniform: UniformBuffer<FrameUniform>,
    frame_data: FrameUniform,
    shadow_extent: vk::Extent2D,
    shadow_cascade_count: u32,
    extent: vk::Extent2D,
    clear_color: [f32; 4],
}

struct ColorTarget {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct PostProcessState {
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_sets: Vec<vk::DescriptorSet>,
    sampler: vk::Sampler,
    pipelines: Vec<vk::Pipeline>,
    targets: Vec<ColorTarget>,
    target_framebuffers: Vec<vk::Framebuffer>,
    swapchain_framebuffers: Vec<vk::Framebuffer>,
    swapchain_images: Vec<vk::Image>,
}

impl App {
    pub fn create(
        window: &Window,
        material: &Material,
        clear_color: [f32; 4],
        pipeline_settings: &RenderPipeline,
        frame_uniform: FrameUniform,
    ) -> Result<Self> {
        assert!(
            !material.passes.is_empty(),
            "renderer material must have at least one shader pass"
        );
        unsafe {
            let loader = LibloadingLoader::new(LIBRARY)?;
            let entry = Entry::new(loader).map_err(|error| anyhow!("{error}"))?;

            let available_layers = entry
                .enumerate_instance_layer_properties()?
                .iter()
                .map(|layer| layer.layer_name)
                .collect::<Vec<_>>();
            let available_extensions = entry
                .enumerate_instance_extension_properties(None)?
                .iter()
                .map(|extension| extension.extension_name)
                .collect::<Vec<_>>();
            let validation_enabled = VALIDATION_REQUESTED
                && available_layers.contains(&VALIDATION_LAYER)
                && available_extensions.contains(&vk::EXT_DEBUG_UTILS_EXTENSION.name);
            if VALIDATION_REQUESTED && !validation_enabled {
                log::warn!(
                    "Vulkan validation layers/debug utils are unavailable; continuing without validation"
                );
            }
            let mut extension_names = vk_window::get_required_instance_extensions(window)
                .iter()
                .map(|extension| extension.as_ptr())
                .collect::<Vec<_>>();
            if validation_enabled {
                extension_names.push(vk::EXT_DEBUG_UTILS_EXTENSION.name.as_ptr());
            }
            let layers = if validation_enabled {
                vec![VALIDATION_LAYER.as_ptr()]
            } else {
                Vec::new()
            };
            let app_info = vk::ApplicationInfo::builder()
                .application_name(b"Rust Vulkan Game Engine\0")
                .application_version(vk::make_version(0, 1, 0))
                .engine_name(b"Game\0")
                .engine_version(vk::make_version(0, 1, 0))
                .api_version(vk::make_version(1, 0, 0));
            let create_info = vk::InstanceCreateInfo::builder()
                .application_info(&app_info)
                .enabled_extension_names(&extension_names)
                .enabled_layer_names(&layers);
            let instance = entry.create_instance(&create_info, None)?;

            let debug_messenger = if validation_enabled {
                let info = vk::DebugUtilsMessengerCreateInfoEXT::builder()
                    .message_severity(
                        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                            | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                    )
                    .message_type(
                        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                            | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                            | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                    )
                    .user_callback(Some(debug_callback));
                instance.create_debug_utils_messenger_ext(&info, None)?
            } else {
                vk::DebugUtilsMessengerEXT::null()
            };

            let surface = vk_window::create_surface(&instance, window, window)?;
            let (physical_device, queue_families) = pick_device(&instance, surface)?;
            let queue_priorities = [1.0_f32];
            let mut queue_infos = Vec::new();
            for family in [queue_families.graphics, queue_families.present] {
                if queue_infos
                    .iter()
                    .any(|info: &vk::DeviceQueueCreateInfo| info.queue_family_index == family)
                {
                    continue;
                }
                queue_infos.push(
                    vk::DeviceQueueCreateInfo::builder()
                        .queue_family_index(family)
                        .queue_priorities(&queue_priorities)
                        .build(),
                );
            }
            let device_extensions = [vk::KHR_SWAPCHAIN_EXTENSION.name.as_ptr()];
            let device_info = vk::DeviceCreateInfo::builder()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&device_extensions);
            let device = instance.create_device(physical_device, &device_info, None)?;
            let graphics_queue = device.get_device_queue(queue_families.graphics, 0);
            let present_queue = device.get_device_queue(queue_families.present, 0);
            let gpu_context = GpuContext::new(
                entry.clone(),
                instance.clone(),
                device.clone(),
                physical_device,
                graphics_queue,
                queue_families.graphics,
                surface,
                debug_messenger,
            );

            let capabilities =
                instance.get_physical_device_surface_capabilities_khr(physical_device, surface)?;
            let formats =
                instance.get_physical_device_surface_formats_khr(physical_device, surface)?;
            let present_modes =
                instance.get_physical_device_surface_present_modes_khr(physical_device, surface)?;
            assert!(
                !formats.is_empty(),
                "Vulkan surface must expose a swapchain format"
            );
            let requested_color_format = match pipeline_settings.swapchain.color_format {
                ColorFormat::Bgra8Srgb => vk::Format::B8G8R8A8_SRGB,
                ColorFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
                ColorFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
            };
            let surface_format = formats
                .iter()
                .copied()
                .find(|format| {
                    format.format == requested_color_format
                        && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
                .or_else(|| {
                    log::warn!("Requested swapchain color format is unavailable; using the first supported format.");
                    formats.first().copied()
                })
                .ok_or_else(|| anyhow!("Vulkan surface exposed no usable swapchain format"))?;
            let extent = if capabilities.current_extent.width != u32::MAX {
                capabilities.current_extent
            } else {
                let size = window.inner_size();
                vk::Extent2D::builder()
                    .width(size.width.clamp(
                        capabilities.min_image_extent.width,
                        capabilities.max_image_extent.width,
                    ))
                    .height(size.height.clamp(
                        capabilities.min_image_extent.height,
                        capabilities.max_image_extent.height,
                    ))
                    .build()
            };
            let image_count = if capabilities.max_image_count == 0 {
                pipeline_settings
                    .swapchain
                    .image_count
                    .max(capabilities.min_image_count)
            } else {
                pipeline_settings
                    .swapchain
                    .image_count
                    .clamp(capabilities.min_image_count, capabilities.max_image_count)
            };
            assert!(image_count >= capabilities.min_image_count);
            assert!(
                capabilities.max_image_count == 0 || image_count <= capabilities.max_image_count
            );
            let families = [queue_families.graphics, queue_families.present];
            let (sharing_mode, indices) = if queue_families.graphics != queue_families.present {
                (vk::SharingMode::CONCURRENT, families.as_slice())
            } else {
                (vk::SharingMode::EXCLUSIVE, &families[..0])
            };
            let requested_present_mode = match pipeline_settings.swapchain.present_mode {
                PresentMode::Fifo => vk::PresentModeKHR::FIFO,
                PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
            };
            let present_mode = if present_modes.contains(&requested_present_mode) {
                requested_present_mode
            } else if present_modes.contains(&vk::PresentModeKHR::FIFO) {
                log::warn!("Requested present mode is unavailable; using FIFO.");
                vk::PresentModeKHR::FIFO
            } else {
                bail!("Vulkan surface does not support FIFO presentation")
            };
            let swapchain_info = vk::SwapchainCreateInfoKHR::builder()
                .surface(surface)
                .min_image_count(image_count)
                .image_format(surface_format.format)
                .image_color_space(surface_format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .image_sharing_mode(sharing_mode)
                .queue_family_indices(indices)
                .pre_transform(capabilities.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(present_mode)
                .clipped(true);
            let swapchain = device.create_swapchain_khr(&swapchain_info, None)?;
            let swapchain_images = device.get_swapchain_images_khr(swapchain)?;
            let image_views =
                create_image_views(&device, &swapchain_images, surface_format.format)?;
            let depth_width = if pipeline_settings.render_buffers.depth.width == 0 {
                extent.width
            } else {
                pipeline_settings.render_buffers.depth.width
            };
            let depth_height = if pipeline_settings.render_buffers.depth.height == 0 {
                extent.height
            } else {
                pipeline_settings.render_buffers.depth.height
            };
            if depth_width < extent.width || depth_height < extent.height {
                bail!(
                    "configured depth render buffer must be at least as large as the swapchain extent"
                );
            }
            let depth_extent = vk::Extent2D::builder()
                .width(depth_width)
                .height(depth_height)
                .build();
            let depth_format = to_vk_depth_format(pipeline_settings.render_buffers.depth.format);
            let (depth_image, depth_memory, depth_view) = create_depth_resources(
                &instance,
                &device,
                physical_device,
                depth_extent,
                depth_format,
            )?;
            let scene_color_target = if pipeline_settings.post_processing.is_empty() {
                None
            } else {
                Some(create_color_target(
                    &instance,
                    &device,
                    physical_device,
                    extent,
                    surface_format.format,
                )?)
            };
            let render_views = scene_color_target
                .as_ref()
                .map(|target| vec![target.view; image_views.len()])
                .unwrap_or_else(|| image_views.clone());
            let color_final_layout = if scene_color_target.is_some() {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::PRESENT_SRC_KHR
            };
            let render_pass = create_render_pass(
                &device,
                surface_format.format,
                depth_format,
                color_final_layout,
            )?;
            let framebuffers =
                create_framebuffers(&device, render_pass, &render_views, depth_view, extent)?;
            let post_process = if let Some(scene_target) = &scene_color_target {
                Some(create_post_process_state(
                    &instance,
                    &device,
                    physical_device,
                    extent,
                    surface_format.format,
                    scene_target.view,
                    &image_views,
                    &swapchain_images,
                    &pipeline_settings.post_processing,
                    &pipeline_settings.output_dir,
                )?)
            } else {
                None
            };
            let shadow_extent = vk::Extent2D::builder()
                .width(pipeline_settings.render_buffers.shadow.width)
                .height(pipeline_settings.render_buffers.shadow.height)
                .build();
            let shadow_format = to_vk_depth_format(pipeline_settings.render_buffers.shadow.format);
            if shadow_format != vk::Format::D32_SFLOAT {
                bail!(
                    "cascaded shadow sampling currently requires render_buffers.shadow.format = \"d32_float\""
                );
            }
            let (
                shadow_image,
                shadow_memory,
                shadow_array_view,
                shadow_layer_views,
                shadow_sampler,
            ) = create_shadow_resources(
                &instance,
                &device,
                physical_device,
                shadow_extent,
                pipeline_settings.render_buffers.shadow_cascades,
                shadow_format,
            )?;
            let shadow_render_pass = create_shadow_render_pass(&device, shadow_format)?;
            let shadow_framebuffers = create_shadow_framebuffers(
                &device,
                shadow_render_pass,
                &shadow_layer_views,
                shadow_extent,
            )?;
            let descriptor_set_layout = create_descriptor_set_layout(&device)?;
            let descriptor_pool = create_descriptor_pool(&device)?;
            let descriptor_set =
                allocate_descriptor_set(&device, descriptor_pool, descriptor_set_layout)?;
            let gpu_frame_uniform = UniformBuffer::new(&gpu_context, frame_uniform)?;
            update_frame_descriptors(
                &device,
                descriptor_set,
                &gpu_frame_uniform,
                shadow_array_view,
                shadow_sampler,
            );
            log::info!("Created swapchain, depth target, framebuffers, and render pass.");
            let (pipeline_layout, pipeline) = create_graphics_pipeline(
                &device,
                render_pass,
                extent,
                material,
                "Forward",
                descriptor_set_layout,
            )?;
            let (shadow_pipeline_layout, shadow_pipeline) = create_graphics_pipeline(
                &device,
                shadow_render_pass,
                shadow_extent,
                material,
                "ShadowCascade",
                descriptor_set_layout,
            )?;
            log::info!("Created graphics pipeline.");
            let material_pipelines = HashMap::from([(material.id(), (pipeline_layout, pipeline))]);
            let pool_info = vk::CommandPoolCreateInfo::builder()
                .queue_family_index(queue_families.graphics)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
            let command_pool = device.create_command_pool(&pool_info, None)?;
            let command_info = vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let command_buffer = device.allocate_command_buffers(&command_info)?[0];
            let semaphore_info = vk::SemaphoreCreateInfo::builder();
            let image_available = device.create_semaphore(&semaphore_info, None)?;
            let render_finished = device.create_semaphore(&semaphore_info, None)?;
            let fence_info = vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED);
            let frame_fence = device.create_fence(&fence_info, None)?;
            let frame_instances =
                InstanceBuffer::new(&gpu_context, &vec![Mat4::IDENTITY; INSTANCE_CAPACITY])?;

            Ok(Self {
                _entry: entry,
                device,
                graphics_queue,
                present_queue,
                swapchain,
                image_views,
                depth_image,
                depth_memory,
                depth_view,
                framebuffers,
                render_pass,
                shadow_render_pass,
                scene_color_target,
                post_process,
                material_pipelines,
                graphics_pipeline_layout: pipeline_layout,
                shadow_pipeline_layout,
                shadow_pipeline,
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                shadow_sampler,
                shadow_image,
                shadow_memory,
                shadow_array_view,
                shadow_layer_views,
                shadow_framebuffers,
                command_pool,
                command_buffer,
                image_available,
                render_finished,
                frame_fence,
                gpu_context,
                frame_instances,
                frame_uniform: gpu_frame_uniform,
                frame_data: frame_uniform,
                shadow_extent,
                shadow_cascade_count: pipeline_settings.render_buffers.shadow_cascades,
                extent,
                clear_color,
            })
        }
    }

    pub fn gpu_context(&self) -> GpuContext {
        self.gpu_context.clone()
    }

    pub fn set_frame_uniform(&mut self, frame_uniform: FrameUniform) -> Result<()> {
        self.frame_uniform.update(frame_uniform)?;
        self.frame_data = frame_uniform;
        Ok(())
    }

    pub fn render(&mut self, graphics: &mut Graphics) -> Result<()> {
        assert!(
            !self.command_buffer.is_null(),
            "App::render requires an allocated command buffer"
        );
        assert!(
            self.frame_instances.len() >= 1,
            "frame instance buffer must exist"
        );
        unsafe {
            self.device
                .wait_for_fences(&[self.frame_fence], true, u64::MAX)?;
            let (image_index, _) = self.device.acquire_next_image_khr(
                self.swapchain,
                u64::MAX,
                self.image_available,
                vk::Fence::null(),
            )?;
            assert!(
                (image_index as usize) < self.framebuffers.len(),
                "acquired swapchain image must have a framebuffer"
            );
            enum RenderItem {
                Indexed {
                    mesh: std::sync::Arc<Mesh>,
                    submesh_index: usize,
                    instances: InstanceBufferHandle,
                    instance_count: u32,
                    first_instance: u32,
                    pipeline: vk::Pipeline,
                },
                Procedural {
                    vertices: u32,
                    instances: u32,
                    pipeline: vk::Pipeline,
                },
            }

            let batches = graphics.optimize();
            let mut frame_transforms = Vec::new();
            let mut draws = Vec::new();
            for batch in batches {
                for command in batch.commands {
                    match command {
                        DrawCommand::Mesh {
                            mesh,
                            submesh_index,
                            transforms,
                        } => {
                            assert!(
                                submesh_index < mesh.submeshes.len(),
                                "mesh draw submesh index must be valid"
                            );
                            let material =
                                &mesh.materials[mesh.submeshes[submesh_index].material_index];
                            let pipeline = self.pipeline_for(material)?;
                            let start = frame_transforms.len();
                            let count: u32 = transforms.len().try_into()?;
                            frame_transforms.extend(transforms);
                            draws.push(RenderItem::Indexed {
                                mesh,
                                submesh_index,
                                instances: self.frame_instances.erase(),
                                instance_count: count,
                                first_instance: start.try_into()?,
                                pipeline,
                            });
                        }
                        DrawCommand::Instanced {
                            mesh,
                            submesh_index,
                            instances,
                        } => {
                            assert!(
                                submesh_index < mesh.submeshes.len(),
                                "instanced submesh index must be valid"
                            );
                            let material =
                                &mesh.materials[mesh.submeshes[submesh_index].material_index];
                            let pipeline = self.pipeline_for(material)?;
                            draws.push(RenderItem::Indexed {
                                mesh,
                                submesh_index,
                                instance_count: instances.len().try_into()?,
                                instances,
                                first_instance: 0,
                                pipeline,
                            });
                        }
                        DrawCommand::Procedural {
                            material,
                            vertices,
                            instances,
                        } => {
                            let pipeline = self.pipeline_for(&material)?;
                            draws.push(RenderItem::Procedural {
                                vertices,
                                instances,
                                pipeline,
                            });
                        }
                    }
                }
            }
            if frame_transforms.len() > self.frame_instances.len() {
                bail!("frame exceeded the instance transform GPU buffer capacity");
            }
            assert!(
                frame_transforms.len() <= INSTANCE_CAPACITY,
                "frame instance count must remain within configured capacity"
            );
            if !frame_transforms.is_empty() {
                self.frame_instances.update(0, &frame_transforms)?;
            }

            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)?;
            assert_eq!(
                self.shadow_framebuffers.len(),
                self.shadow_cascade_count as usize,
                "each cascade needs one depth framebuffer"
            );
            for cascade in 0..self.shadow_cascade_count {
                self.frame_data.cascade_index = cascade;
                self.frame_uniform.update(self.frame_data)?;
                let clear = [vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.0,
                        stencil: 0,
                    },
                }];
                let area = vk::Rect2D::builder()
                    .offset(vk::Offset2D::builder().x(0).y(0))
                    .extent(self.shadow_extent)
                    .build();
                let begin = vk::RenderPassBeginInfo::builder()
                    .render_pass(self.shadow_render_pass)
                    .framebuffer(self.shadow_framebuffers[cascade as usize])
                    .render_area(area)
                    .clear_values(&clear);
                self.device.cmd_begin_render_pass(
                    self.command_buffer,
                    &begin,
                    vk::SubpassContents::INLINE,
                );
                let viewport = vk::Viewport::builder()
                    .x(0.0)
                    .y(0.0)
                    .width(self.shadow_extent.width as f32)
                    .height(self.shadow_extent.height as f32)
                    .min_depth(0.0)
                    .max_depth(1.0);
                let scissor = vk::Rect2D::builder()
                    .offset(vk::Offset2D::builder().x(0).y(0))
                    .extent(self.shadow_extent);
                self.device
                    .cmd_set_viewport(self.command_buffer, 0, &[viewport]);
                self.device
                    .cmd_set_scissor(self.command_buffer, 0, &[scissor]);
                self.device.cmd_bind_pipeline(
                    self.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.shadow_pipeline,
                );
                let descriptor_sets = [self.descriptor_set];
                self.device.cmd_bind_descriptor_sets(
                    self.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.shadow_pipeline_layout,
                    0,
                    &descriptor_sets,
                    &[],
                );
                for draw in &draws {
                    if let RenderItem::Indexed {
                        mesh,
                        submesh_index,
                        instances,
                        instance_count,
                        first_instance,
                        ..
                    } = draw
                    {
                        assert!(*instance_count > 0);
                        let submesh = mesh.submeshes[*submesh_index];
                        self.device.cmd_bind_vertex_buffers(
                            self.command_buffer,
                            0,
                            &[mesh.vertices.handle(), instances.handle()],
                            &[0, 0],
                        );
                        self.device.cmd_bind_index_buffer(
                            self.command_buffer,
                            mesh.indices.handle(),
                            0,
                            vk::IndexType::UINT32,
                        );
                        self.device.cmd_draw_indexed(
                            self.command_buffer,
                            submesh.index_count,
                            *instance_count,
                            submesh.first_index,
                            0,
                            *first_instance,
                        );
                    }
                }
                self.device.cmd_end_render_pass(self.command_buffer);
            }
            self.frame_data.cascade_index = 0;
            self.frame_uniform.update(self.frame_data)?;
            let clear_values = [
                vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: self.clear_color,
                    },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.0,
                        stencil: 0,
                    },
                },
            ];
            let render_area = vk::Rect2D::builder()
                .offset(vk::Offset2D::builder().x(0).y(0))
                .extent(self.extent)
                .build();
            let render_info = vk::RenderPassBeginInfo::builder()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(render_area)
                .clear_values(&clear_values);
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &render_info,
                vk::SubpassContents::INLINE,
            );
            let viewport = vk::Viewport::builder()
                .x(0.0)
                .y(0.0)
                .width(self.extent.width as f32)
                .height(self.extent.height as f32)
                .min_depth(0.0)
                .max_depth(1.0);
            let scissor = vk::Rect2D::builder()
                .offset(vk::Offset2D::builder().x(0).y(0))
                .extent(self.extent);
            self.device
                .cmd_set_viewport(self.command_buffer, 0, &[viewport]);
            self.device
                .cmd_set_scissor(self.command_buffer, 0, &[scissor]);
            for draw in draws {
                match draw {
                    RenderItem::Indexed {
                        mesh,
                        submesh_index,
                        instances,
                        instance_count,
                        first_instance,
                        pipeline,
                    } => {
                        self.device.cmd_bind_pipeline(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipeline,
                        );
                        let descriptor_sets = [self.descriptor_set];
                        self.device.cmd_bind_descriptor_sets(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            self.graphics_pipeline_layout,
                            0,
                            &descriptor_sets,
                            &[],
                        );
                        let submesh = mesh.submeshes[submesh_index];
                        self.device.cmd_bind_vertex_buffers(
                            self.command_buffer,
                            0,
                            &[mesh.vertices.handle(), instances.handle()],
                            &[0, 0],
                        );
                        self.device.cmd_bind_index_buffer(
                            self.command_buffer,
                            mesh.indices.handle(),
                            0,
                            vk::IndexType::UINT32,
                        );
                        self.device.cmd_draw_indexed(
                            self.command_buffer,
                            submesh.index_count,
                            instance_count,
                            submesh.first_index,
                            0,
                            first_instance,
                        );
                    }
                    RenderItem::Procedural {
                        vertices,
                        instances,
                        pipeline,
                    } => {
                        self.device.cmd_bind_pipeline(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipeline,
                        );
                        let descriptor_sets = [self.descriptor_set];
                        self.device.cmd_bind_descriptor_sets(
                            self.command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            self.graphics_pipeline_layout,
                            0,
                            &descriptor_sets,
                            &[],
                        );
                        self.device
                            .cmd_draw(self.command_buffer, vertices, instances, 0, 0);
                    }
                }
            }
            self.device.cmd_end_render_pass(self.command_buffer);
            if let Some(post_process) = &self.post_process {
                for (index, pipeline) in post_process.pipelines.iter().enumerate() {
                    let is_final = index + 1 == post_process.pipelines.len();
                    let framebuffer = if is_final {
                        post_process.swapchain_framebuffers[image_index as usize]
                    } else {
                        post_process.target_framebuffers[index]
                    };
                    let clear_values = [vk::ClearValue {
                        color: vk::ClearColorValue {
                            float32: [0.0, 0.0, 0.0, 1.0],
                        },
                    }];
                    let render_info = vk::RenderPassBeginInfo::builder()
                        .render_pass(post_process.render_pass)
                        .framebuffer(framebuffer)
                        .render_area(render_area)
                        .clear_values(&clear_values);
                    self.device.cmd_begin_render_pass(
                        self.command_buffer,
                        &render_info,
                        vk::SubpassContents::INLINE,
                    );
                    self.device
                        .cmd_set_viewport(self.command_buffer, 0, &[viewport]);
                    self.device
                        .cmd_set_scissor(self.command_buffer, 0, &[scissor]);
                    self.device.cmd_bind_pipeline(
                        self.command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        *pipeline,
                    );
                    self.device.cmd_bind_descriptor_sets(
                        self.command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        post_process.pipeline_layout,
                        0,
                        &[post_process.descriptor_sets[index]],
                        &[],
                    );
                    self.device.cmd_draw(self.command_buffer, 3, 1, 0, 0);
                    self.device.cmd_end_render_pass(self.command_buffer);

                    let (image, new_layout, destination_stage, destination_access) = if is_final {
                        (
                            post_process.swapchain_images[image_index as usize],
                            vk::ImageLayout::PRESENT_SRC_KHR,
                            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                            vk::AccessFlags::empty(),
                        )
                    } else {
                        (
                            post_process.targets[index].image,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::PipelineStageFlags::FRAGMENT_SHADER,
                            vk::AccessFlags::SHADER_READ,
                        )
                    };
                    let barrier = vk::ImageMemoryBarrier::builder()
                        .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                        .dst_access_mask(destination_access)
                        .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                        .new_layout(new_layout)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(image)
                        .subresource_range(
                            vk::ImageSubresourceRange::builder()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .base_mip_level(0)
                                .level_count(1)
                                .base_array_layer(0)
                                .layer_count(1),
                        )
                        .build();
                    let memory_barriers: [vk::MemoryBarrier; 0] = [];
                    let buffer_barriers: [vk::BufferMemoryBarrier; 0] = [];
                    self.device.cmd_pipeline_barrier(
                        self.command_buffer,
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                        destination_stage,
                        vk::DependencyFlags::empty(),
                        &memory_barriers,
                        &buffer_barriers,
                        &[barrier],
                    );
                }
            }
            self.device.end_command_buffer(self.command_buffer)?;

            let wait_semaphores = [self.image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let command_buffers = [self.command_buffer];
            let signal_semaphores = [self.render_finished];
            let submit_info = vk::SubmitInfo::builder()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores);
            self.device.reset_fences(&[self.frame_fence])?;
            self.device
                .queue_submit(self.graphics_queue, &[submit_info], self.frame_fence)?;
            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::builder()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            self.device
                .queue_present_khr(self.present_queue, &present_info)?;
            self.device.queue_wait_idle(self.present_queue)?;
        }
        Ok(())
    }

    fn pipeline_for(&mut self, material: &Material) -> Result<vk::Pipeline> {
        if let Some((_, pipeline)) = self.material_pipelines.get(&material.id()) {
            return Ok(*pipeline);
        }
        let (layout, pipeline) = unsafe {
            create_graphics_pipeline(
                &self.device,
                self.render_pass,
                self.extent,
                material,
                "Forward",
                self.descriptor_set_layout,
            )?
        };
        self.material_pipelines
            .insert(material.id(), (layout, pipeline));
        Ok(pipeline)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.frame_fence, None);
            self.device.destroy_semaphore(self.image_available, None);
            self.device.destroy_semaphore(self.render_finished, None);
            self.device.destroy_command_pool(self.command_pool, None);
            for (layout, pipeline) in self.material_pipelines.values() {
                self.device.destroy_pipeline(*pipeline, None);
                self.device.destroy_pipeline_layout(*layout, None);
            }
            if let Some(post_process) = &self.post_process {
                for framebuffer in &post_process.target_framebuffers {
                    self.device.destroy_framebuffer(*framebuffer, None);
                }
                for framebuffer in &post_process.swapchain_framebuffers {
                    self.device.destroy_framebuffer(*framebuffer, None);
                }
                for pipeline in &post_process.pipelines {
                    self.device.destroy_pipeline(*pipeline, None);
                }
                self.device
                    .destroy_pipeline_layout(post_process.pipeline_layout, None);
                self.device
                    .destroy_descriptor_pool(post_process.descriptor_pool, None);
                self.device.destroy_sampler(post_process.sampler, None);
                self.device
                    .destroy_descriptor_set_layout(post_process.descriptor_set_layout, None);
                self.device
                    .destroy_render_pass(post_process.render_pass, None);
                for target in &post_process.targets {
                    destroy_color_target(&self.device, target);
                }
            }
            self.device.destroy_pipeline(self.shadow_pipeline, None);
            self.device
                .destroy_pipeline_layout(self.shadow_pipeline_layout, None);
            for framebuffer in &self.framebuffers {
                self.device.destroy_framebuffer(*framebuffer, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            if let Some(target) = &self.scene_color_target {
                destroy_color_target(&self.device, target);
            }
            self.device
                .destroy_render_pass(self.shadow_render_pass, None);
            for view in &self.image_views {
                self.device.destroy_image_view(*view, None);
            }
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth_image, None);
            self.device.free_memory(self.depth_memory, None);
            self.device.destroy_swapchain_khr(self.swapchain, None);
            for framebuffer in &self.shadow_framebuffers {
                self.device.destroy_framebuffer(*framebuffer, None);
            }
            for view in &self.shadow_layer_views {
                self.device.destroy_image_view(*view, None);
            }
            self.device.destroy_image_view(self.shadow_array_view, None);
            self.device.destroy_sampler(self.shadow_sampler, None);
            self.device.destroy_image(self.shadow_image, None);
            self.device.free_memory(self.shadow_memory, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

unsafe fn pick_device(
    instance: &Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, QueueFamilies)> {
    for physical_device in instance.enumerate_physical_devices()? {
        let queue_properties =
            instance.get_physical_device_queue_family_properties(physical_device);
        let mut graphics = None;
        let mut present = None;
        for (index, properties) in queue_properties.iter().enumerate() {
            if properties
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            {
                graphics = Some(index as u32);
            }
            if instance.get_physical_device_surface_support_khr(
                physical_device,
                index as u32,
                surface,
            )? {
                present = Some(index as u32);
            }
        }
        if let (Some(graphics), Some(present)) = (graphics, present) {
            let extensions =
                instance.enumerate_device_extension_properties(physical_device, None)?;
            if extensions
                .iter()
                .any(|extension| extension.extension_name == vk::KHR_SWAPCHAIN_EXTENSION.name)
            {
                return Ok((physical_device, QueueFamilies { graphics, present }));
            }
        }
    }
    bail!("no Vulkan device supports graphics, compute, presentation, and VK_KHR_swapchain")
}

unsafe fn create_image_views(
    device: &Device,
    images: &[vk::Image],
    format: vk::Format,
) -> Result<Vec<vk::ImageView>> {
    images
        .iter()
        .map(|image| {
            let info = vk::ImageViewCreateInfo::builder()
                .image(*image)
                .view_type(vk::ImageViewType::_2D)
                .format(format)
                .components(vk::ComponentMapping::builder())
                .subresource_range(
                    vk::ImageSubresourceRange::builder()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            device.create_image_view(&info, None).map_err(Into::into)
        })
        .collect()
}

unsafe fn create_color_target(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    extent: vk::Extent2D,
    format: vk::Format,
) -> Result<ColorTarget> {
    let features = instance
        .get_physical_device_format_properties(physical_device, format)
        .optimal_tiling_features;
    if !features
        .contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::SAMPLED_IMAGE)
    {
        bail!("Vulkan device does not support sampled color targets in format {format:?}");
    }
    let image_info = vk::ImageCreateInfo::builder()
        .image_type(vk::ImageType::_2D)
        .format(format)
        .extent(
            vk::Extent3D::builder()
                .width(extent.width)
                .height(extent.height)
                .depth(1),
        )
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = device.create_image(&image_info, None)?;
    let requirements = device.get_image_memory_requirements(image);
    let memory_properties = instance.get_physical_device_memory_properties(physical_device);
    let memory_type = (0..memory_properties.memory_type_count)
        .find(|index| {
            requirements.memory_type_bits & (1 << index) != 0
                && memory_properties.memory_types[*index as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .ok_or_else(|| anyhow!("no device-local Vulkan memory type for color target"))?;
    let allocation_info = vk::MemoryAllocateInfo::builder()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    let memory = match device.allocate_memory(&allocation_info, None) {
        Ok(memory) => memory,
        Err(error) => {
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    if let Err(error) = device.bind_image_memory(image, memory, 0) {
        device.free_memory(memory, None);
        device.destroy_image(image, None);
        return Err(error.into());
    }
    let view_info = vk::ImageViewCreateInfo::builder()
        .image(image)
        .view_type(vk::ImageViewType::_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::builder()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let view = match device.create_image_view(&view_info, None) {
        Ok(view) => view,
        Err(error) => {
            device.free_memory(memory, None);
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    Ok(ColorTarget {
        image,
        memory,
        view,
    })
}

unsafe fn destroy_color_target(device: &Device, target: &ColorTarget) {
    device.destroy_image_view(target.view, None);
    device.destroy_image(target.image, None);
    device.free_memory(target.memory, None);
}

unsafe fn create_post_process_state(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    extent: vk::Extent2D,
    format: vk::Format,
    scene_view: vk::ImageView,
    swapchain_views: &[vk::ImageView],
    swapchain_images: &[vk::Image],
    passes: &[std::sync::Arc<dyn PostProcessPass>],
    output_dir: &std::path::Path,
) -> Result<PostProcessState> {
    assert!(!passes.is_empty());
    let mut targets = Vec::with_capacity(passes.len().saturating_sub(1));
    for _ in 1..passes.len() {
        targets.push(create_color_target(
            instance,
            device,
            physical_device,
            extent,
            format,
        )?);
    }
    let render_pass = create_post_process_render_pass(device, format)?;
    let target_framebuffers = targets
        .iter()
        .map(|target| create_color_framebuffer(device, render_pass, target.view, extent))
        .collect::<Result<Vec<_>>>()?;
    let swapchain_framebuffers =
        create_color_framebuffers(device, render_pass, swapchain_views, extent)?;
    let descriptor_set_layout = create_post_process_descriptor_set_layout(device)?;
    let descriptor_pool = create_post_process_descriptor_pool(device, passes.len())?;
    let descriptor_sets = allocate_post_process_descriptor_sets(
        device,
        descriptor_pool,
        descriptor_set_layout,
        passes.len(),
    )?;
    let sampler_info = vk::SamplerCreateInfo::builder()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .anisotropy_enable(false)
        .max_anisotropy(1.0)
        .compare_enable(false)
        .min_lod(0.0)
        .max_lod(0.0)
        .unnormalized_coordinates(false);
    let sampler = device.create_sampler(&sampler_info, None)?;
    for (index, descriptor_set) in descriptor_sets.iter().enumerate() {
        let source_view = if index == 0 {
            scene_view
        } else {
            targets[index - 1].view
        };
        let image_info = [vk::DescriptorImageInfo::builder()
            .image_view(source_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::builder().sampler(sampler)];
        let writes = [
            vk::WriteDescriptorSet::builder()
                .dst_set(*descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::builder()
                .dst_set(*descriptor_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        ];
        let copies: [vk::CopyDescriptorSet; 0] = [];
        device.update_descriptor_sets(&writes, &copies);
    }
    let set_layouts = [descriptor_set_layout];
    let layout_info = vk::PipelineLayoutCreateInfo::builder().set_layouts(&set_layouts);
    let pipeline_layout = device.create_pipeline_layout(&layout_info, None)?;
    let mut pipelines = Vec::with_capacity(passes.len());
    for (index, _) in passes.iter().enumerate() {
        let vertex_words = read_shader_words(&output_dir.join(format!("post_{index}.vert.spv")))?;
        let fragment_words = read_shader_words(&output_dir.join(format!("post_{index}.frag.spv")))?;
        pipelines.push(create_post_process_pipeline(
            device,
            render_pass,
            extent,
            pipeline_layout,
            &vertex_words,
            &fragment_words,
        )?);
    }
    Ok(PostProcessState {
        render_pass,
        pipeline_layout,
        descriptor_set_layout,
        descriptor_pool,
        descriptor_sets,
        sampler,
        pipelines,
        targets,
        target_framebuffers,
        swapchain_framebuffers,
        swapchain_images: swapchain_images.to_vec(),
    })
}

unsafe fn create_post_process_render_pass(
    device: &Device,
    format: vk::Format,
) -> Result<vk::RenderPass> {
    let attachment = vk::AttachmentDescription::builder()
        .format(format)
        .samples(vk::SampleCountFlags::_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let color_ref = vk::AttachmentReference::builder()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));
    let dependencies = [
        vk::SubpassDependency::builder()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
        vk::SubpassDependency::builder()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::builder()
        .attachments(std::slice::from_ref(&attachment))
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    device.create_render_pass(&info, None).map_err(Into::into)
}

unsafe fn create_color_framebuffer(
    device: &Device,
    render_pass: vk::RenderPass,
    view: vk::ImageView,
    extent: vk::Extent2D,
) -> Result<vk::Framebuffer> {
    let attachments = [view];
    let info = vk::FramebufferCreateInfo::builder()
        .render_pass(render_pass)
        .attachments(&attachments)
        .width(extent.width)
        .height(extent.height)
        .layers(1);
    device.create_framebuffer(&info, None).map_err(Into::into)
}

unsafe fn create_color_framebuffers(
    device: &Device,
    render_pass: vk::RenderPass,
    views: &[vk::ImageView],
    extent: vk::Extent2D,
) -> Result<Vec<vk::Framebuffer>> {
    views
        .iter()
        .map(|view| create_color_framebuffer(device, render_pass, *view, extent))
        .collect()
}

unsafe fn create_post_process_descriptor_set_layout(
    device: &Device,
) -> Result<vk::DescriptorSetLayout> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    device
        .create_descriptor_set_layout(&info, None)
        .map_err(Into::into)
}

unsafe fn create_post_process_descriptor_pool(
    device: &Device,
    count: usize,
) -> Result<vk::DescriptorPool> {
    let count: u32 = count.try_into()?;
    let sizes = [
        vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(count),
        vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::SAMPLER)
            .descriptor_count(count),
    ];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .max_sets(count)
        .pool_sizes(&sizes);
    device
        .create_descriptor_pool(&info, None)
        .map_err(Into::into)
}

unsafe fn allocate_post_process_descriptor_sets(
    device: &Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    count: usize,
) -> Result<Vec<vk::DescriptorSet>> {
    let layouts = vec![layout; count];
    let info = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    device.allocate_descriptor_sets(&info).map_err(Into::into)
}

unsafe fn create_post_process_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    layout: vk::PipelineLayout,
    vertex_words: &[u32],
    fragment_words: &[u32],
) -> Result<vk::Pipeline> {
    let vertex_module = create_shader_module(device, vertex_words)?;
    let fragment_module = match create_shader_module(device, fragment_words) {
        Ok(module) => module,
        Err(error) => {
            device.destroy_shader_module(vertex_module, None);
            return Err(error);
        }
    };
    let vertex_entry = CString::new("VSMain")?;
    let fragment_entry = CString::new("PSMain")?;
    let stages = [
        vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module)
            .name(vertex_entry.as_bytes_with_nul()),
        vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module)
            .name(fragment_entry.as_bytes_with_nul()),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder();
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport = vk::Viewport::builder()
        .x(0.0)
        .y(0.0)
        .width(extent.width as f32)
        .height(extent.height as f32)
        .min_depth(0.0)
        .max_depth(1.0);
    let scissor = vk::Rect2D::builder()
        .offset(vk::Offset2D::builder().x(0).y(0))
        .extent(extent);
    let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
        .viewports(std::slice::from_ref(&viewport))
        .scissors(std::slice::from_ref(&scissor));
    let rasterization = vk::PipelineRasterizationStateCreateInfo::builder()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::empty())
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
        .rasterization_samples(vk::SampleCountFlags::_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
        .depth_test_enable(false)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::ALWAYS)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState::builder()
        .color_write_mask(vk::ColorComponentFlags::all())
        .blend_enable(false)
        .build()];
    let blend = vk::PipelineColorBlendStateCreateInfo::builder()
        .logic_op_enable(false)
        .attachments(&blend_attachment);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dynamic_states);
    let info = vk::GraphicsPipelineCreateInfo::builder()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);
    let pipeline = match device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
    {
        Ok((pipelines, _)) => pipelines[0],
        Err(error) => {
            device.destroy_shader_module(vertex_module, None);
            device.destroy_shader_module(fragment_module, None);
            return Err(error.into());
        }
    };
    device.destroy_shader_module(vertex_module, None);
    device.destroy_shader_module(fragment_module, None);
    Ok(pipeline)
}

fn read_shader_words(path: &std::path::Path) -> Result<Vec<u32>> {
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow!("Failed to read {}: {error}", path.display()))?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        bail!(
            "SPIR-V file {} must contain whole, non-empty words",
            path.display()
        );
    }
    let words = bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().expect("four-byte SPIR-V word")))
        .collect::<Vec<_>>();
    if words.first() != Some(&0x0723_0203) {
        bail!("{} is not a SPIR-V module", path.display());
    }
    Ok(words)
}

unsafe fn create_shadow_resources(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    extent: vk::Extent2D,
    layers: u32,
    format: vk::Format,
) -> Result<(
    vk::Image,
    vk::DeviceMemory,
    vk::ImageView,
    Vec<vk::ImageView>,
    vk::Sampler,
)> {
    assert!(extent.width > 0 && extent.height > 0);
    assert!((1..=4).contains(&layers));
    let properties = instance.get_physical_device_format_properties(physical_device, format);
    if !properties.optimal_tiling_features.contains(
        vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT | vk::FormatFeatureFlags::SAMPLED_IMAGE,
    ) {
        bail!("Vulkan device does not support sampled D32_SFLOAT depth images");
    }
    let image_info = vk::ImageCreateInfo::builder()
        .image_type(vk::ImageType::_2D)
        .format(format)
        .extent(
            vk::Extent3D::builder()
                .width(extent.width)
                .height(extent.height)
                .depth(1),
        )
        .mip_levels(1)
        .array_layers(layers)
        .samples(vk::SampleCountFlags::_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = device.create_image(&image_info, None)?;
    let requirements = device.get_image_memory_requirements(image);
    let properties = instance.get_physical_device_memory_properties(physical_device);
    let memory_type = (0..properties.memory_type_count)
        .find(|index| {
            requirements.memory_type_bits & (1 << index) != 0
                && properties.memory_types[*index as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .ok_or_else(|| anyhow!("no device-local memory type for shadow cascade image"))?;
    let allocation = vk::MemoryAllocateInfo::builder()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    let memory = match device.allocate_memory(&allocation, None) {
        Ok(memory) => memory,
        Err(error) => {
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    if let Err(error) = device.bind_image_memory(image, memory, 0) {
        device.free_memory(memory, None);
        device.destroy_image(image, None);
        return Err(error.into());
    }
    let array_view_info = vk::ImageViewCreateInfo::builder()
        .image(image)
        .view_type(vk::ImageViewType::_2D_ARRAY)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::builder()
                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(layers),
        );
    let array_view = match device.create_image_view(&array_view_info, None) {
        Ok(view) => view,
        Err(error) => {
            device.free_memory(memory, None);
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    let mut layer_views = Vec::with_capacity(layers as usize);
    for layer in 0..layers {
        let view_info = vk::ImageViewCreateInfo::builder()
            .image(image)
            .view_type(vk::ImageViewType::_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::builder()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(layer)
                    .layer_count(1),
            );
        match device.create_image_view(&view_info, None) {
            Ok(view) => layer_views.push(view),
            Err(error) => {
                for view in layer_views {
                    device.destroy_image_view(view, None);
                }
                device.destroy_image_view(array_view, None);
                device.free_memory(memory, None);
                device.destroy_image(image, None);
                return Err(error.into());
            }
        }
    }
    let sampler_info = vk::SamplerCreateInfo::builder()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_BORDER)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_BORDER)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_BORDER)
        .mip_lod_bias(0.0)
        .anisotropy_enable(false)
        .max_anisotropy(1.0)
        .compare_enable(true)
        .compare_op(vk::CompareOp::LESS_OR_EQUAL)
        .min_lod(0.0)
        .max_lod(0.0)
        .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
        .unnormalized_coordinates(false);
    let sampler = match device.create_sampler(&sampler_info, None) {
        Ok(sampler) => sampler,
        Err(error) => {
            for view in layer_views {
                device.destroy_image_view(view, None);
            }
            device.destroy_image_view(array_view, None);
            device.free_memory(memory, None);
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    Ok((image, memory, array_view, layer_views, sampler))
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(2)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    device
        .create_descriptor_set_layout(&info, None)
        .map_err(Into::into)
}

unsafe fn create_descriptor_pool(device: &Device) -> Result<vk::DescriptorPool> {
    let sizes = [
        vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1),
        vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1),
        vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::SAMPLER)
            .descriptor_count(1),
    ];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .max_sets(1)
        .pool_sizes(&sizes);
    device
        .create_descriptor_pool(&info, None)
        .map_err(Into::into)
}

unsafe fn allocate_descriptor_set(
    device: &Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> Result<vk::DescriptorSet> {
    let layouts = [layout];
    let info = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    let sets = device.allocate_descriptor_sets(&info)?;
    assert_eq!(
        sets.len(),
        1,
        "one descriptor set layout must allocate one set"
    );
    Ok(sets[0])
}

unsafe fn update_frame_descriptors(
    device: &Device,
    set: vk::DescriptorSet,
    frame_uniform: &UniformBuffer<FrameUniform>,
    shadow_view: vk::ImageView,
    shadow_sampler: vk::Sampler,
) {
    let buffer_info = [vk::DescriptorBufferInfo::builder()
        .buffer(frame_uniform.handle())
        .offset(0)
        .range(std::mem::size_of::<FrameUniform>() as u64)];
    let image_info = [vk::DescriptorImageInfo::builder()
        .image_view(shadow_view)
        .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)];
    let sampler_info = [vk::DescriptorImageInfo::builder().sampler(shadow_sampler)];
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&buffer_info),
        vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image_info),
        vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&sampler_info),
    ];
    let copies: [vk::CopyDescriptorSet; 0] = [];
    device.update_descriptor_sets(&writes, &copies);
}

unsafe fn create_shadow_render_pass(
    device: &Device,
    depth_format: vk::Format,
) -> Result<vk::RenderPass> {
    let attachment = vk::AttachmentDescription::builder()
        .format(depth_format)
        .samples(vk::SampleCountFlags::_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
    let depth_ref = vk::AttachmentReference::builder()
        .attachment(0)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .depth_stencil_attachment(&depth_ref);
    let dependencies = [
        vk::SubpassDependency::builder()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE),
        vk::SubpassDependency::builder()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::builder()
        .attachments(std::slice::from_ref(&attachment))
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    device.create_render_pass(&info, None).map_err(Into::into)
}

unsafe fn create_shadow_framebuffers(
    device: &Device,
    render_pass: vk::RenderPass,
    layer_views: &[vk::ImageView],
    extent: vk::Extent2D,
) -> Result<Vec<vk::Framebuffer>> {
    assert!(!layer_views.is_empty());
    layer_views
        .iter()
        .map(|view| {
            let attachments = [*view];
            let info = vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            device.create_framebuffer(&info, None).map_err(Into::into)
        })
        .collect()
}

fn to_vk_depth_format(format: DepthFormat) -> vk::Format {
    match format {
        DepthFormat::D32Float => vk::Format::D32_SFLOAT,
        DepthFormat::D32FloatS8 => vk::Format::D32_SFLOAT_S8_UINT,
        DepthFormat::D24UnormS8 => vk::Format::D24_UNORM_S8_UINT,
    }
}

unsafe fn create_depth_resources(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    extent: vk::Extent2D,
    format: vk::Format,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    assert!(extent.width > 0 && extent.height > 0);
    if !instance
        .get_physical_device_format_properties(physical_device, format)
        .optimal_tiling_features
        .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    {
        bail!("Vulkan device does not support configured depth format {format:?}");
    }
    let image_info = vk::ImageCreateInfo::builder()
        .image_type(vk::ImageType::_2D)
        .format(format)
        .extent(
            vk::Extent3D::builder()
                .width(extent.width)
                .height(extent.height)
                .depth(1),
        )
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = device.create_image(&image_info, None)?;
    let requirements = device.get_image_memory_requirements(image);
    let memory_properties = instance.get_physical_device_memory_properties(physical_device);
    let memory_type = (0..memory_properties.memory_type_count)
        .find(|index| {
            requirements.memory_type_bits & (1 << index) != 0
                && memory_properties.memory_types[*index as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .ok_or_else(|| anyhow!("no device-local Vulkan memory type for depth attachment"))?;
    let allocation_info = vk::MemoryAllocateInfo::builder()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    let memory = match device.allocate_memory(&allocation_info, None) {
        Ok(memory) => memory,
        Err(error) => {
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    if let Err(error) = device.bind_image_memory(image, memory, 0) {
        device.free_memory(memory, None);
        device.destroy_image(image, None);
        return Err(error.into());
    }
    let view_info = vk::ImageViewCreateInfo::builder()
        .image(image)
        .view_type(vk::ImageViewType::_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::builder()
                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let view = match device.create_image_view(&view_info, None) {
        Ok(view) => view,
        Err(error) => {
            device.free_memory(memory, None);
            device.destroy_image(image, None);
            return Err(error.into());
        }
    };
    Ok((image, memory, view))
}

unsafe fn create_render_pass(
    device: &Device,
    format: vk::Format,
    depth_format: vk::Format,
    color_final_layout: vk::ImageLayout,
) -> Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::builder()
        .format(format)
        .samples(vk::SampleCountFlags::_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(color_final_layout)
        .build();
    let depth_attachment = vk::AttachmentDescription::builder()
        .format(depth_format)
        .samples(vk::SampleCountFlags::_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::DONT_CARE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
        .build();
    let color_ref = vk::AttachmentReference::builder()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .build();
    let depth_ref = vk::AttachmentReference::builder()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
        .build();
    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref))
        .depth_stencil_attachment(&depth_ref);
    let dependencies = [
        vk::SubpassDependency::builder()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
        vk::SubpassDependency::builder()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            )
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let attachments = [color_attachment, depth_attachment];
    let info = vk::RenderPassCreateInfo::builder()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    device.create_render_pass(&info, None).map_err(Into::into)
}

unsafe fn create_framebuffers(
    device: &Device,
    render_pass: vk::RenderPass,
    views: &[vk::ImageView],
    depth_view: vk::ImageView,
    extent: vk::Extent2D,
) -> Result<Vec<vk::Framebuffer>> {
    views
        .iter()
        .map(|view| {
            let attachments = [*view, depth_view];
            let info = vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            device.create_framebuffer(&info, None).map_err(Into::into)
        })
        .collect()
}

unsafe fn create_graphics_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    material: &Material,
    pass_name: &str,
    descriptor_set_layout: vk::DescriptorSetLayout,
) -> Result<(vk::PipelineLayout, vk::Pipeline)> {
    let shader_pass = material
        .passes
        .iter()
        .find(|pass| pass.name == pass_name)
        .ok_or_else(|| anyhow!("material has no shader pass named '{pass_name}'"))?;
    let vertex_module = create_shader_module(device, &shader_pass.vertex_spirv)?;
    let fragment_module = match create_shader_module(device, &shader_pass.fragment_spirv) {
        Ok(module) => module,
        Err(error) => {
            device.destroy_shader_module(vertex_module, None);
            return Err(error);
        }
    };
    log::info!("Created shader modules for pass '{}'.", shader_pass.name);
    let vertex_entry = CString::new(shader_pass.vertex_entry.as_str())?;
    let fragment_entry = CString::new(shader_pass.fragment_entry.as_str())?;
    let stages = [
        vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module)
            .name(vertex_entry.as_bytes_with_nul()),
        vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module)
            .name(fragment_entry.as_bytes_with_nul()),
    ];
    let vertex_stride = material
        .passes
        .iter()
        .map(|pass| pass.vertex_inputs.stride)
        .max()
        .unwrap_or(0);
    let instance_stride = material
        .passes
        .iter()
        .map(|pass| pass.instance_inputs.stride)
        .max()
        .unwrap_or(0);
    if vertex_stride == 0 || instance_stride == 0 {
        bail!("shader passes must declare vertex and instance inputs");
    }
    let binding = [
        vk::VertexInputBindingDescription::builder()
            .binding(0)
            .stride(vertex_stride)
            .input_rate(vk::VertexInputRate::VERTEX),
        vk::VertexInputBindingDescription::builder()
            .binding(1)
            .stride(instance_stride)
            .input_rate(vk::VertexInputRate::INSTANCE),
    ];
    let mut attributes = shader_pass
        .vertex_inputs
        .attributes
        .iter()
        .map(|attribute| {
            vk::VertexInputAttributeDescription::builder()
                .location(attribute.location)
                .binding(0)
                .format(shader_input_vk_format(attribute.format))
                .offset(attribute.offset)
                .build()
        })
        .collect::<Vec<_>>();
    attributes.extend(
        shader_pass
            .instance_inputs
            .attributes
            .iter()
            .map(|attribute| {
                vk::VertexInputAttributeDescription::builder()
                    .location(attribute.location)
                    .binding(1)
                    .format(shader_input_vk_format(attribute.format))
                    .offset(attribute.offset)
                    .build()
            }),
    );
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder()
        .vertex_binding_descriptions(&binding)
        .vertex_attribute_descriptions(&attributes);
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport = vk::Viewport::builder()
        .x(0.0)
        .y(0.0)
        .width(extent.width as f32)
        .height(extent.height as f32)
        .min_depth(0.0)
        .max_depth(1.0);
    let scissor = vk::Rect2D::builder()
        .offset(vk::Offset2D::builder().x(0).y(0))
        .extent(extent);
    let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
        .viewports(std::slice::from_ref(&viewport))
        .scissors(std::slice::from_ref(&scissor));
    let cull_mode = match shader_pass.cull {
        crate::render_pipeline::CullMode::None => vk::CullModeFlags::empty(),
        crate::render_pipeline::CullMode::Front => vk::CullModeFlags::FRONT,
        crate::render_pipeline::CullMode::Back => vk::CullModeFlags::BACK,
    };
    let rasterization = vk::PipelineRasterizationStateCreateInfo::builder()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(cull_mode)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
        .rasterization_samples(vk::SampleCountFlags::_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);
    let blend_attachments = if pass_name == "ShadowCascade" {
        Vec::new()
    } else {
        vec![
            vk::PipelineColorBlendAttachmentState::builder()
                .color_write_mask(vk::ColorComponentFlags::all())
                .blend_enable(false)
                .build(),
        ]
    };
    let blend = vk::PipelineColorBlendStateCreateInfo::builder()
        .logic_op_enable(false)
        .attachments(&blend_attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dynamic_states);
    let set_layouts = [descriptor_set_layout];
    let layout_info = vk::PipelineLayoutCreateInfo::builder().set_layouts(&set_layouts);
    let layout = device.create_pipeline_layout(&layout_info, None)?;
    let info = vk::GraphicsPipelineCreateInfo::builder()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);
    log::info!(
        "Creating graphics pipeline for pass '{}'.",
        shader_pass.name
    );
    let pipeline = match device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
    {
        Ok((pipelines, _)) => pipelines[0],
        Err(error) => {
            device.destroy_pipeline_layout(layout, None);
            device.destroy_shader_module(vertex_module, None);
            device.destroy_shader_module(fragment_module, None);
            return Err(error.into());
        }
    };
    device.destroy_shader_module(vertex_module, None);
    device.destroy_shader_module(fragment_module, None);
    Ok((layout, pipeline))
}

fn shader_input_vk_format(format: ShaderInputFormat) -> vk::Format {
    match format {
        ShaderInputFormat::Float32 => vk::Format::R32_SFLOAT,
        ShaderInputFormat::Float32x2 => vk::Format::R32G32_SFLOAT,
        ShaderInputFormat::Float32x3 => vk::Format::R32G32B32_SFLOAT,
        ShaderInputFormat::Float32x4 => vk::Format::R32G32B32A32_SFLOAT,
        ShaderInputFormat::Sint32 => vk::Format::R32_SINT,
        ShaderInputFormat::Sint32x2 => vk::Format::R32G32_SINT,
        ShaderInputFormat::Sint32x3 => vk::Format::R32G32B32_SINT,
        ShaderInputFormat::Sint32x4 => vk::Format::R32G32B32A32_SINT,
        ShaderInputFormat::Uint32 => vk::Format::R32_UINT,
        ShaderInputFormat::Uint32x2 => vk::Format::R32G32_UINT,
        ShaderInputFormat::Uint32x3 => vk::Format::R32G32B32_UINT,
        ShaderInputFormat::Uint32x4 => vk::Format::R32G32B32A32_UINT,
    }
}

unsafe fn create_shader_module(device: &Device, words: &[u32]) -> Result<vk::ShaderModule> {
    if words.is_empty() {
        bail!("shader module has no SPIR-V words");
    }
    let code_size = words
        .len()
        .checked_mul(std::mem::size_of::<u32>())
        .ok_or_else(|| anyhow!("shader module byte size overflow"))?;
    let info = vk::ShaderModuleCreateInfo::builder()
        .code_size(code_size)
        .code(words);
    device.create_shader_module(&info, None).map_err(Into::into)
}
