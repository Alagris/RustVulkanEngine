use super::gpu_context::GpuContext;
use super::vertex::Vertex;
use super::vertex_layout::{VertexLayout, validate_input_layout};
use crate::render_pipeline::{ShaderInputAttribute, ShaderInputFormat};
use anyhow::{Result, anyhow, bail};
use bytemuck::Pod;
use glam::Mat4;
use std::marker::PhantomData;
use std::sync::Arc;
use vulkanalia::prelude::v1_0::*;

struct GpuAllocation {
    context: GpuContext,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u8,
    byte_capacity: usize,
}

impl Drop for GpuAllocation {
    fn drop(&mut self) {
        unsafe {
            self.context.0.device.unmap_memory(self.memory);
            self.context.0.device.destroy_buffer(self.buffer, None);
            self.context.0.device.free_memory(self.memory, None);
        }
    }
}

pub struct Buffer<T: Pod> {
    allocation: Arc<GpuAllocation>,
    len: usize,
    marker: PhantomData<T>,
}

impl<T: Pod> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        Self {
            allocation: Arc::clone(&self.allocation),
            len: self.len,
            marker: PhantomData,
        }
    }
}

impl<T: Pod> Buffer<T> {
    pub(crate) fn new(
        context: &GpuContext,
        values: &[T],
        usage: vk::BufferUsageFlags,
    ) -> Result<Self> {
        assert!(
            std::mem::size_of::<T>() > 0,
            "GPU element types must be sized"
        );
        let valid_usage = [
            vk::BufferUsageFlags::VERTEX_BUFFER,
            vk::BufferUsageFlags::INDEX_BUFFER,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER,
        ];
        assert!(
            valid_usage.contains(&usage),
            "GPU buffer wrapper only permits typed vertex, index, uniform, or storage usage"
        );
        if values.is_empty() {
            bail!("GPU buffers cannot be empty");
        }
        let bytes: &[u8] = bytemuck::cast_slice(values);
        let byte_capacity = bytes.len();
        let info = vk::BufferCreateInfo::builder()
            .size(byte_capacity as u64)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let device = context.device();
        let buffer = unsafe { device.create_buffer(&info, None)? };
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let properties = unsafe {
            context
                .0
                .instance
                .get_physical_device_memory_properties(context.physical_device())
        };
        let memory_type = (0..properties.memory_type_count)
            .find(|index| {
                let flags = properties.memory_types[*index as usize].property_flags;
                requirements.memory_type_bits & (1 << index) != 0
                    && flags.contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )
            })
            .ok_or_else(|| anyhow!("no host-visible coherent Vulkan memory type"))?;
        let allocation_info = vk::MemoryAllocateInfo::builder()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        let memory = match unsafe { device.allocate_memory(&allocation_info, None) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { device.destroy_buffer(buffer, None) };
                return Err(error.into());
            }
        };
        if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            unsafe {
                device.free_memory(memory, None);
                device.destroy_buffer(buffer, None);
            }
            return Err(error.into());
        }
        let mapped = match unsafe {
            device.map_memory(memory, 0, requirements.size, vk::MemoryMapFlags::empty())
        } {
            Ok(mapped) => mapped as *mut u8,
            Err(error) => {
                unsafe {
                    device.free_memory(memory, None);
                    device.destroy_buffer(buffer, None);
                }
                return Err(error.into());
            }
        };
        let allocation = Arc::new(GpuAllocation {
            context: context.clone(),
            buffer,
            memory,
            mapped,
            byte_capacity,
        });
        let result = Self {
            allocation,
            len: values.len(),
            marker: PhantomData,
        };
        result.update(0, values)?;
        Ok(result)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn update(&self, offset: usize, values: &[T]) -> Result<()> {
        assert!(
            offset <= self.len,
            "update offset must be within element capacity"
        );
        assert!(
            values.len() <= self.len - offset,
            "update range must be within element capacity"
        );
        let end = offset
            .checked_add(values.len())
            .ok_or_else(|| anyhow!("GPU buffer update range overflow"))?;
        if end > self.len {
            bail!("GPU buffer update exceeds allocated element count");
        }
        let start = offset
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| anyhow!("GPU buffer byte offset overflow"))?;
        let bytes = bytemuck::cast_slice(values);
        let end = start
            .checked_add(bytes.len())
            .ok_or_else(|| anyhow!("GPU buffer byte range overflow"))?;
        if end > self.allocation.byte_capacity {
            bail!("GPU buffer update exceeds allocated byte capacity");
        }
        unsafe {
            self.allocation.context.0.device.device_wait_idle()?;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.allocation.mapped.add(start),
                bytes.len(),
            );
        }
        Ok(())
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.allocation.buffer
    }
}

pub struct UniformBuffer<T: Pod>(Buffer<T>);

impl<T: Pod> UniformBuffer<T> {
    pub fn new(context: &GpuContext, value: T) -> Result<Self> {
        Ok(Self(Buffer::new(
            context,
            &[value],
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?))
    }

    pub fn update(&self, value: T) -> Result<()> {
        self.0.update(0, &[value])
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.0.handle()
    }
}

pub trait ComputeBuffer {
    type Element: Pod;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn update(&self, offset: usize, values: &[Self::Element]) -> Result<()>;
}

#[derive(Clone)]
pub struct VertexBuffer {
    buffer: Buffer<u8>,
    pub layout: VertexLayout,
    vertex_count: usize,
}

impl VertexBuffer {
    pub fn new(context: &GpuContext, vertices: &[Vertex]) -> Result<Self> {
        Self::with_raw_data(
            context,
            bytemuck::cast_slice(vertices),
            vertices.len(),
            Vertex::layout(),
        )
    }

    pub fn with_raw_data(
        context: &GpuContext,
        bytes: &[u8],
        vertex_count: usize,
        layout: VertexLayout,
    ) -> Result<Self> {
        Self::with_raw_data_usage(
            context,
            bytes,
            vertex_count,
            layout,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )
    }

    pub(crate) fn with_raw_data_usage(
        context: &GpuContext,
        bytes: &[u8],
        vertex_count: usize,
        layout: VertexLayout,
        usage: vk::BufferUsageFlags,
    ) -> Result<Self> {
        validate_input_layout(layout.stride, &layout.attributes)?;
        let expected_bytes = (layout.stride as usize)
            .checked_mul(vertex_count)
            .ok_or_else(|| anyhow!("vertex buffer size overflow"))?;
        if vertex_count == 0 || bytes.len() != expected_bytes {
            bail!("vertex bytes must exactly match the declared layout and vertex count");
        }
        Ok(Self {
            buffer: Buffer::new(context, bytes, usage)?,
            layout,
            vertex_count,
        })
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.buffer.handle()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    pub fn update_bytes(&self, byte_offset: usize, bytes: &[u8]) -> Result<()> {
        self.buffer.update(byte_offset, bytes)
    }
}

impl ComputeBuffer for VertexBuffer {
    type Element = u8;
    fn len(&self) -> usize {
        self.buffer.len()
    }
    fn update(&self, offset: usize, values: &[u8]) -> Result<()> {
        self.buffer.update(offset, values)
    }
}

#[derive(Clone)]
pub struct IndexBuffer(Buffer<u32>);

impl IndexBuffer {
    pub fn new(context: &GpuContext, indices: &[u32]) -> Result<Self> {
        Ok(Self(Buffer::new(
            context,
            indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?))
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.0.handle()
    }
}

impl ComputeBuffer for IndexBuffer {
    type Element = u32;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn update(&self, offset: usize, values: &[u32]) -> Result<()> {
        self.0.update(offset, values)
    }
}

/// Instance data must contain the same four-column matrix layout consumed by the renderer.
pub trait InstanceData: Pod + Send + Sync + 'static {
    fn layout() -> VertexLayout;
}

impl InstanceData for Mat4 {
    fn layout() -> VertexLayout {
        VertexLayout {
            stride: std::mem::size_of::<Self>() as u32,
            attributes: (0..4)
                .map(|column| ShaderInputAttribute {
                    location: 2 + column,
                    format: ShaderInputFormat::Float32x4,
                    offset: column * 16,
                })
                .collect(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct InstanceBufferHandle {
    allocation: Arc<GpuAllocation>,
    len: usize,
    layout: VertexLayout,
}

impl InstanceBufferHandle {
    pub(crate) fn handle(&self) -> vk::Buffer {
        self.allocation.buffer
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn layout(&self) -> &VertexLayout {
        &self.layout
    }
}

#[derive(Clone)]
pub struct InstanceBuffer<T: InstanceData> {
    buffer: Buffer<T>,
    layout: VertexLayout,
}

impl<T: InstanceData> InstanceBuffer<T> {
    pub fn new(context: &GpuContext, instances: &[T]) -> Result<Self> {
        assert!(!instances.is_empty(), "instance buffers must not be empty");
        let layout = T::layout();
        validate_input_layout(layout.stride, &layout.attributes)?;
        if layout.stride as usize != std::mem::size_of::<T>() {
            bail!("instance layout stride must equal the instance element size");
        }
        Ok(Self {
            buffer: Buffer::new(context, instances, vk::BufferUsageFlags::VERTEX_BUFFER)?,
            layout,
        })
    }

    pub(crate) fn erase(&self) -> InstanceBufferHandle {
        InstanceBufferHandle {
            allocation: Arc::clone(&self.buffer.allocation),
            len: self.buffer.len,
            layout: self.layout.clone(),
        }
    }
}

impl<T: InstanceData> ComputeBuffer for InstanceBuffer<T> {
    type Element = T;
    fn len(&self) -> usize {
        self.buffer.len()
    }
    fn update(&self, offset: usize, values: &[T]) -> Result<()> {
        self.buffer.update(offset, values)
    }
}
