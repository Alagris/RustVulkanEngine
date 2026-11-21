use crate::graphics::{
    ComputeBuffer, DrawCommand, GpuContext, Graphics, InstanceBuffer, InstanceBufferHandle,
    Material, Mesh, Vertex,
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
    material_pipelines: HashMap<u64, (vk::PipelineLayout, vk::Pipeline)>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    frame_fence: vk::Fence,
    gpu_context: GpuContext,
    frame_instances: InstanceBuffer<Mat4>,
    extent: vk::Extent2D,
    clear_color: [f32; 4],
}

impl App {
    pub fn create(window: &Window, material: &Material, clear_color: [f32; 4]) -> Result<Self> {
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
                surface,
                debug_messenger,
            );

            let capabilities =
                instance.get_physical_device_surface_capabilities_khr(physical_device, surface)?;
            let formats =
                instance.get_physical_device_surface_formats_khr(physical_device, surface)?;
            let present_modes =
                instance.get_physical_device_surface_present_modes_khr(physical_device, surface)?;
            let surface_format = formats
                .iter()
                .copied()
                .find(|format| {
                    format.format == vk::Format::B8G8R8A8_SRGB
                        && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
                .unwrap_or_else(|| formats[0]);
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
                capabilities.min_image_count + 1
            } else {
                (capabilities.min_image_count + 1).min(capabilities.max_image_count)
            };
            let families = [queue_families.graphics, queue_families.present];
            let (sharing_mode, indices) = if queue_families.graphics != queue_families.present {
                (vk::SharingMode::CONCURRENT, families.as_slice())
            } else {
                (vk::SharingMode::EXCLUSIVE, &families[..0])
            };
            let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
                vk::PresentModeKHR::MAILBOX
            } else {
                vk::PresentModeKHR::FIFO
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
            let (depth_image, depth_memory, depth_view, depth_format) =
                create_depth_resources(&instance, &device, physical_device, extent)?;
            let render_pass = create_render_pass(&device, surface_format.format, depth_format)?;
            let framebuffers =
                create_framebuffers(&device, render_pass, &image_views, depth_view, extent)?;
            log::info!("Created swapchain, depth target, framebuffers, and render pass.");
            let (pipeline_layout, pipeline) =
                create_graphics_pipeline(&device, render_pass, extent, material)?;
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
                material_pipelines,
                command_pool,
                command_buffer,
                image_available,
                render_finished,
                frame_fence,
                gpu_context,
                frame_instances,
                extent,
                clear_color,
            })
        }
    }

    pub fn gpu_context(&self) -> GpuContext {
        self.gpu_context.clone()
    }

    pub fn render(&mut self, graphics: &mut Graphics) -> Result<()> {
        unsafe {
            self.device
                .wait_for_fences(&[self.frame_fence], true, u64::MAX)?;
            let (image_index, _) = self.device.acquire_next_image_khr(
                self.swapchain,
                u64::MAX,
                self.image_available,
                vk::Fence::null(),
            )?;
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
            if !frame_transforms.is_empty() {
                self.frame_instances.update(0, &frame_transforms)?;
            }

            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)?;
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
                        self.device
                            .cmd_draw(self.command_buffer, vertices, instances, 0, 0);
                    }
                }
            }
            self.device.cmd_end_render_pass(self.command_buffer);
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
            create_graphics_pipeline(&self.device, self.render_pass, self.extent, material)?
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
            for framebuffer in &self.framebuffers {
                self.device.destroy_framebuffer(*framebuffer, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            for view in &self.image_views {
                self.device.destroy_image_view(*view, None);
            }
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth_image, None);
            self.device.free_memory(self.depth_memory, None);
            self.device.destroy_swapchain_khr(self.swapchain, None);
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
            if properties.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
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
    bail!("no Vulkan device supports graphics, presentation, and VK_KHR_swapchain")
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

unsafe fn create_depth_resources(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    extent: vk::Extent2D,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView, vk::Format)> {
    let format = [
        vk::Format::D32_SFLOAT,
        vk::Format::D32_SFLOAT_S8_UINT,
        vk::Format::D24_UNORM_S8_UINT,
    ]
    .into_iter()
    .find(|format| {
        instance
            .get_physical_device_format_properties(physical_device, *format)
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    })
    .ok_or_else(|| anyhow!("Vulkan device does not support a depth attachment format"))?;
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
    Ok((image, memory, view, format))
}

unsafe fn create_render_pass(
    device: &Device,
    format: vk::Format,
    depth_format: vk::Format,
) -> Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::builder()
        .format(format)
        .samples(vk::SampleCountFlags::_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)
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
    let dependency = vk::SubpassDependency::builder()
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
        );
    let attachments = [color_attachment, depth_attachment];
    let info = vk::RenderPassCreateInfo::builder()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(std::slice::from_ref(&dependency));
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
) -> Result<(vk::PipelineLayout, vk::Pipeline)> {
    let shader_pass = material
        .passes
        .first()
        .ok_or_else(|| anyhow!("material has no shader pass"))?;
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
    let binding = [
        vk::VertexInputBindingDescription::builder()
            .binding(0)
            .stride(std::mem::size_of::<Vertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX),
        vk::VertexInputBindingDescription::builder()
            .binding(1)
            .stride(std::mem::size_of::<Mat4>() as u32)
            .input_rate(vk::VertexInputRate::INSTANCE),
    ];
    let mut attributes = vec![
        vk::VertexInputAttributeDescription::builder()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(std::mem::offset_of!(Vertex, position) as u32),
        vk::VertexInputAttributeDescription::builder()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32B32A32_SFLOAT)
            .offset(std::mem::offset_of!(Vertex, color) as u32),
    ];
    for column in 0..4 {
        attributes.push(
            vk::VertexInputAttributeDescription::builder()
                .location(2 + column)
                .binding(1)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(column * 16),
        );
    }
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
    let blend_attachment = vk::PipelineColorBlendAttachmentState::builder()
        .color_write_mask(vk::ColorComponentFlags::all())
        .blend_enable(false);
    let blend = vk::PipelineColorBlendStateCreateInfo::builder()
        .logic_op_enable(false)
        .attachments(std::slice::from_ref(&blend_attachment));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dynamic_states);
    let layout_info = vk::PipelineLayoutCreateInfo::builder();
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
