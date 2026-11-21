use super::buffer::{Buffer, VertexBuffer};
use super::gpu_context::GpuContext;
use super::vertex::Vertex;
use anyhow::{Context, Result, anyhow, bail};
use bytemuck::cast_slice;
use glam::Mat4;
use shaderc::{CompileOptions, Compiler, ShaderKind};
use vulkanalia::prelude::v1_0::*;

const WORKGROUP_SIZE: u32 = 64;

const SKINNING_SHADER: &str = r#"
#version 450
layout(local_size_x = 64, local_size_y = 1, local_size_z = 1) in;

struct Vertex {
    vec4 position;
    vec4 color;
};

layout(set = 0, binding = 0, std430) readonly buffer SourceVertices {
    Vertex vertices[];
} source_data;
layout(set = 0, binding = 1, std430) readonly buffer VertexTransforms {
    mat4 transforms[];
} transform_data;
layout(set = 0, binding = 2, std430) writeonly buffer DestinationVertices {
    Vertex vertices[];
} destination_data;

layout(push_constant) uniform Parameters {
    uint vertex_count;
} parameters;

void main() {
    uint index = gl_GlobalInvocationID.x;
    if (index >= parameters.vertex_count) {
        return;
    }
    Vertex source = source_data.vertices[index];
    destination_data.vertices[index].position =
        transform_data.transforms[index] * source.position;
    destination_data.vertices[index].color = source.color;
}
"#;

pub(crate) struct GpuSkinner {
    context: GpuContext,
    _source_vertices: Buffer<Vertex>,
    transforms: Buffer<Mat4>,
    vertex_count: u32,
    output: VertexBuffer,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
}

impl GpuSkinner {
    pub(crate) fn new(
        context: &GpuContext,
        source_vertices: &[Vertex],
        transforms: &[Mat4],
    ) -> Result<(VertexBuffer, Self)> {
        if source_vertices.is_empty() || source_vertices.len() != transforms.len() {
            bail!("GPU skinning requires matching non-empty vertex and transform arrays");
        }
        let vertex_count = u32::try_from(source_vertices.len())
            .context("GPU skinning vertex count exceeds u32::MAX")?;
        let limits = unsafe {
            context
                .0
                .instance
                .get_physical_device_properties(context.physical_device())
                .limits
        };
        let vertex_bytes = u64::from(vertex_count)
            .checked_mul(std::mem::size_of::<Vertex>() as u64)
            .ok_or_else(|| anyhow!("GPU skinning vertex buffer size overflow"))?;
        let transform_bytes = u64::from(vertex_count)
            .checked_mul(std::mem::size_of::<Mat4>() as u64)
            .ok_or_else(|| anyhow!("GPU skinning transform buffer size overflow"))?;
        if vertex_bytes > u64::from(limits.max_storage_buffer_range)
            || transform_bytes > u64::from(limits.max_storage_buffer_range)
        {
            bail!("GPU skinning buffers exceed the device's maximum storage-buffer range");
        }
        if vertex_count.div_ceil(WORKGROUP_SIZE) > limits.max_compute_work_group_count[0] {
            bail!("GPU skinning dispatch exceeds the device's compute workgroup limit");
        }
        let source = Buffer::new(
            context,
            source_vertices,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let transform_buffer =
            Buffer::new(context, transforms, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let output = VertexBuffer::with_raw_data_usage(
            context,
            cast_slice(source_vertices),
            source_vertices.len(),
            Vertex::layout(),
            vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let mut skinner = Self {
            context: context.clone(),
            _source_vertices: source,
            transforms: transform_buffer,
            vertex_count,
            output: output.clone(),
            descriptor_set_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
        };
        skinner.initialize()?;
        skinner.dispatch()?;
        Ok((output, skinner))
    }

    pub(crate) fn update_transforms(&mut self, transforms: &[Mat4]) -> Result<()> {
        if transforms.len() != self.vertex_count as usize {
            bail!("GPU skinning transform count changed after mesh creation");
        }
        self.transforms.update(0, transforms)?;
        self.dispatch()
    }

    fn initialize(&mut self) -> Result<()> {
        let device = self.context.device();
        let bindings = (0..3)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::builder()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
                    .build()
            })
            .collect::<Vec<_>>();
        let descriptor_layout_info =
            vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
        self.descriptor_set_layout =
            unsafe { device.create_descriptor_set_layout(&descriptor_layout_info, None)? };

        let pool_sizes = [vk::DescriptorPoolSize::builder()
            .type_(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(3)
            .build()];
        let descriptor_pool_info = vk::DescriptorPoolCreateInfo::builder()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        self.descriptor_pool =
            unsafe { device.create_descriptor_pool(&descriptor_pool_info, None)? };
        let set_layouts = [self.descriptor_set_layout];
        let descriptor_set_info = vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&set_layouts);
        self.descriptor_set = unsafe {
            device
                .allocate_descriptor_sets(&descriptor_set_info)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("Vulkan did not allocate the skinning descriptor set"))?
        };

        let matrix_bytes = (self.vertex_count as u64)
            .checked_mul(std::mem::size_of::<Mat4>() as u64)
            .ok_or_else(|| anyhow!("GPU skinning transform buffer size overflow"))?;
        let vertex_bytes = (self.vertex_count as u64)
            .checked_mul(std::mem::size_of::<Vertex>() as u64)
            .ok_or_else(|| anyhow!("GPU skinning vertex buffer size overflow"))?;
        let buffer_infos = [
            vk::DescriptorBufferInfo::builder()
                .buffer(self._source_vertices.handle())
                .offset(0)
                .range(vertex_bytes)
                .build(),
            vk::DescriptorBufferInfo::builder()
                .buffer(self.transforms.handle())
                .offset(0)
                .range(matrix_bytes)
                .build(),
            vk::DescriptorBufferInfo::builder()
                .buffer(self.output.handle())
                .offset(0)
                .range(vertex_bytes)
                .build(),
        ];
        let writes = (0..3)
            .map(|binding| {
                vk::WriteDescriptorSet::builder()
                    .dst_set(self.descriptor_set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&buffer_infos[binding]))
                    .build()
            })
            .collect::<Vec<_>>();
        unsafe {
            let copies: [vk::CopyDescriptorSet; 0] = [];
            device.update_descriptor_sets(&writes, &copies);
        }

        let set_layouts = [self.descriptor_set_layout];
        let push_ranges = [vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<u32>() as u32)
            .build()];
        let pipeline_layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        self.pipeline_layout =
            unsafe { device.create_pipeline_layout(&pipeline_layout_info, None)? };

        let spirv = compile_skinning_shader()?;
        let shader_info = vk::ShaderModuleCreateInfo::builder().code(&spirv);
        let shader_module = unsafe { device.create_shader_module(&shader_info, None)? };
        let stage = vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(b"main\0");
        let pipeline_info = vk::ComputePipelineCreateInfo::builder()
            .stage(stage)
            .layout(self.pipeline_layout);
        let pipeline_result = unsafe {
            device.create_compute_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
        };
        unsafe {
            device.destroy_shader_module(shader_module, None);
        }
        self.pipeline = match pipeline_result {
            Ok((pipelines, _)) => *pipelines
                .first()
                .ok_or_else(|| anyhow!("Vulkan created no skinning compute pipeline"))?,
            Err(error) => return Err(error.into()),
        };

        let pool_info = vk::CommandPoolCreateInfo::builder()
            .queue_family_index(self.context.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        self.command_pool = unsafe { device.create_command_pool(&pool_info, None)? };
        let allocate_info = vk::CommandBufferAllocateInfo::builder()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        self.command_buffer = unsafe {
            device
                .allocate_command_buffers(&allocate_info)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("Vulkan did not allocate the skinning command buffer"))?
        };
        Ok(())
    }

    fn dispatch(&mut self) -> Result<()> {
        let device = self.context.device();
        unsafe {
            device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            device.begin_command_buffer(self.command_buffer, &begin_info)?;
            device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline,
            );
            device.cmd_bind_descriptor_sets(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_push_constants(
                self.command_buffer,
                self.pipeline_layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&self.vertex_count),
            );
            device.cmd_dispatch(
                self.command_buffer,
                self.vertex_count.div_ceil(WORKGROUP_SIZE),
                1,
                1,
            );
            let barriers = [vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::VERTEX_ATTRIBUTE_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.output.handle())
                .offset(0)
                .size(vk::WHOLE_SIZE)
                .build()];
            let memory_barriers: [vk::MemoryBarrier; 0] = [];
            let image_barriers: [vk::ImageMemoryBarrier; 0] = [];
            device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::VERTEX_INPUT,
                vk::DependencyFlags::empty(),
                &memory_barriers,
                &barriers,
                &image_barriers,
            );
            device.end_command_buffer(self.command_buffer)?;
            let command_buffers = [self.command_buffer];
            let submit_info = vk::SubmitInfo::builder().command_buffers(&command_buffers);
            device.queue_submit(self.context.queue(), &[submit_info], vk::Fence::null())?;
            device.queue_wait_idle(self.context.queue())?;
        }
        Ok(())
    }
}

impl Drop for GpuSkinner {
    fn drop(&mut self) {
        unsafe {
            let device = self.context.device();
            let _ = device.device_wait_idle();
            if !self.command_pool.is_null() {
                device.destroy_command_pool(self.command_pool, None);
            }
            if !self.pipeline.is_null() {
                device.destroy_pipeline(self.pipeline, None);
            }
            if !self.pipeline_layout.is_null() {
                device.destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if !self.descriptor_pool.is_null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if !self.descriptor_set_layout.is_null() {
                device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            }
        }
    }
}

fn compile_skinning_shader() -> Result<Vec<u32>> {
    let compiler =
        Compiler::new().context("Failed to initialize the embedded compute shader compiler")?;
    let options =
        CompileOptions::new().context("Failed to initialize compute shader compiler options")?;
    let artifact = compiler
        .compile_into_spirv(
            SKINNING_SHADER,
            ShaderKind::Compute,
            "builtin_skinning.comp",
            "main",
            Some(&options),
        )
        .map_err(|error| anyhow!("GPU skinning shader compilation failed: {error}"))?;
    if artifact.as_binary().is_empty() {
        bail!("GPU skinning shader compiler produced empty SPIR-V");
    }
    Ok(artifact.as_binary().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_the_gpu_skinning_compute_shader() {
        let spirv = compile_skinning_shader().unwrap();
        assert_eq!(spirv.first(), Some(&0x0723_0203));
    }
}
