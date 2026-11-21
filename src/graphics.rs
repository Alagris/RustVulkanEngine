use crate::render_pipeline::{CullMode, RenderQueue};
use anyhow::{Result, anyhow, bail};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3A, Vec4};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vulkanalia::prelude::v1_0::*;
use vulkanalia::vk::{ExtDebugUtilsExtensionInstanceCommands, KhrSurfaceExtensionInstanceCommands};

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MATERIAL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: Vec3A,
    pub color: Vec4,
}

pub type Transform = Mat4;

pub(crate) struct GpuContextInner {
    _entry: Entry,
    pub instance: Instance,
    pub device: Device,
    pub physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    debug_messenger: vk::DebugUtilsMessengerEXT,
}

impl Drop for GpuContextInner {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.instance.destroy_surface_khr(self.surface, None);
            if !self.debug_messenger.is_null() {
                self.instance
                    .destroy_debug_utils_messenger_ext(self.debug_messenger, None);
            }
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[derive(Clone)]
pub struct GpuContext(Arc<GpuContextInner>);

impl GpuContext {
    pub(crate) fn new(
        entry: Entry,
        instance: Instance,
        device: Device,
        physical_device: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
        debug_messenger: vk::DebugUtilsMessengerEXT,
    ) -> Self {
        Self(Arc::new(GpuContextInner {
            _entry: entry,
            instance,
            device,
            physical_device,
            surface,
            debug_messenger,
        }))
    }

    pub(crate) fn device(&self) -> &Device {
        &self.0.device
    }

    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.0.physical_device
    }
}

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
    fn new(context: &GpuContext, values: &[T], usage: vk::BufferUsageFlags) -> Result<Self> {
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

pub trait ComputeBuffer {
    type Element: Pod;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn update(&self, offset: usize, values: &[Self::Element]) -> Result<()>;
}

#[derive(Clone)]
pub struct VertexBuffer(Buffer<Vertex>);

impl VertexBuffer {
    pub fn new(context: &GpuContext, vertices: &[Vertex]) -> Result<Self> {
        Ok(Self(Buffer::new(
            context,
            vertices,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?))
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.0.handle()
    }
}

impl ComputeBuffer for VertexBuffer {
    type Element = Vertex;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn update(&self, offset: usize, values: &[Vertex]) -> Result<()> {
        self.0.update(offset, values)
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
pub trait InstanceData: Pod + Send + Sync + 'static {}

impl InstanceData for Mat4 {}

#[derive(Clone)]
pub(crate) struct InstanceBufferHandle {
    allocation: Arc<GpuAllocation>,
    len: usize,
}

impl InstanceBufferHandle {
    pub(crate) fn handle(&self) -> vk::Buffer {
        self.allocation.buffer
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

#[derive(Clone)]
pub struct InstanceBuffer<T: InstanceData>(Buffer<T>);

impl<T: InstanceData> InstanceBuffer<T> {
    pub fn new(context: &GpuContext, instances: &[T]) -> Result<Self> {
        if std::mem::size_of::<T>() != std::mem::size_of::<Mat4>() {
            bail!("instance data must use the 64-byte matrix layout expected by the shader");
        }
        Ok(Self(Buffer::new(
            context,
            instances,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?))
    }

    pub(crate) fn erase(&self) -> InstanceBufferHandle {
        InstanceBufferHandle {
            allocation: Arc::clone(&self.0.allocation),
            len: self.0.len,
        }
    }
}

impl<T: InstanceData> ComputeBuffer for InstanceBuffer<T> {
    type Element = T;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn update(&self, offset: usize, values: &[T]) -> Result<()> {
        self.0.update(offset, values)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submesh {
    pub first_index: u32,
    pub index_count: u32,
    pub material_index: usize,
}

#[derive(Clone)]
pub struct Mesh {
    id: u64,
    pub vertices: VertexBuffer,
    pub indices: IndexBuffer,
    pub materials: Vec<Arc<Material>>,
    pub submeshes: Vec<Submesh>,
}

impl Mesh {
    pub fn new(
        context: &GpuContext,
        vertices: &[Vertex],
        indices: &[u32],
        materials: Vec<Arc<Material>>,
        submeshes: Vec<Submesh>,
    ) -> Result<Self> {
        if vertices.is_empty() || indices.is_empty() || materials.is_empty() || submeshes.is_empty()
        {
            bail!("meshes require vertices, indices, materials, and submeshes");
        }
        if indices
            .iter()
            .any(|&index| index as usize >= vertices.len())
        {
            bail!("mesh index is outside the vertex array");
        }
        for submesh in &submeshes {
            let end = submesh
                .first_index
                .checked_add(submesh.index_count)
                .ok_or_else(|| anyhow!("submesh index range overflow"))?;
            if submesh.index_count == 0
                || end as usize > indices.len()
                || submesh.material_index >= materials.len()
            {
                bail!("submesh has an invalid index range or material index");
            }
        }
        Ok(Self {
            id: NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed),
            vertices: VertexBuffer::new(context, vertices)?,
            indices: IndexBuffer::new(context, indices)?,
            materials,
            submeshes,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UniformValue {
    Float(f32),
    Int(i32),
    Vec2(Vec2),
    Vec3(glam::Vec3),
    Vec4(Vec4),
    Mat4(Mat4),
}

#[derive(Clone, Debug)]
pub struct ShaderPass {
    pub name: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    pub vertex_spirv: Arc<[u32]>,
    pub fragment_spirv: Arc<[u32]>,
    pub cull: CullMode,
    pub queue: RenderQueue,
}

#[derive(Clone, Debug)]
pub struct Material {
    id: u64,
    pub passes: Vec<ShaderPass>,
    uniforms: HashMap<String, UniformValue>,
}

impl Material {
    pub fn new(passes: Vec<ShaderPass>) -> Result<Self> {
        if passes.is_empty() {
            bail!("materials require at least one compiled shader pass");
        }
        Ok(Self {
            id: NEXT_MATERIAL_ID.fetch_add(1, Ordering::Relaxed),
            passes,
            uniforms: HashMap::new(),
        })
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn set_uniform(&mut self, name: impl Into<String>, value: UniformValue) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            bail!("uniform name must not be empty");
        }
        self.uniforms.insert(name, value);
        Ok(())
    }
    pub fn uniform(&self, name: &str) -> Option<&UniformValue> {
        self.uniforms.get(name)
    }
}

#[derive(Clone)]
pub(crate) enum DrawCommand {
    Mesh {
        mesh: Arc<Mesh>,
        submesh_index: usize,
        transforms: Vec<Transform>,
    },
    Instanced {
        mesh: Arc<Mesh>,
        submesh_index: usize,
        instances: InstanceBufferHandle,
    },
    Procedural {
        material: Arc<Material>,
        vertices: u32,
        instances: u32,
    },
}

impl DrawCommand {
    fn sort_key(&self) -> (u64, u64, usize) {
        match self {
            Self::Mesh {
                mesh,
                submesh_index,
                ..
            }
            | Self::Instanced {
                mesh,
                submesh_index,
                ..
            } => (
                mesh.materials[mesh.submeshes[*submesh_index].material_index].id(),
                mesh.id(),
                *submesh_index,
            ),
            Self::Procedural {
                material, vertices, ..
            } => (material.id(), u64::MAX, *vertices as usize),
        }
    }
    fn is_transparent(&self) -> bool {
        let material = match self {
            Self::Mesh {
                mesh,
                submesh_index,
                ..
            }
            | Self::Instanced {
                mesh,
                submesh_index,
                ..
            } => &mesh.materials[mesh.submeshes[*submesh_index].material_index],
            Self::Procedural { material, .. } => material,
        };
        material
            .passes
            .iter()
            .any(|pass| pass.queue == RenderQueue::Transparent)
    }
}

#[derive(Clone)]
pub(crate) struct DrawBatch {
    pub(crate) commands: Vec<DrawCommand>,
}

#[derive(Default)]
pub struct Graphics {
    queued: Vec<DrawCommand>,
}

impl Graphics {
    pub fn draw(&mut self, mesh: &Mesh, transform: Transform) {
        let mesh = Arc::new(mesh.clone());
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Mesh {
                mesh: Arc::clone(&mesh),
                submesh_index,
                transforms: vec![transform],
            });
        }
    }
    pub fn draw_procedural(
        &mut self,
        material: Arc<Material>,
        vertices: i32,
        instances: i32,
    ) -> Result<()> {
        if vertices <= 0 || instances <= 0 {
            bail!("procedural vertex and instance counts must be positive");
        }
        self.queued.push(DrawCommand::Procedural {
            material,
            vertices: vertices as u32,
            instances: instances as u32,
        });
        Ok(())
    }
    pub fn draw_instanced<T: InstanceData>(&mut self, mesh: &Mesh, instances: &InstanceBuffer<T>) {
        if instances.is_empty() {
            return;
        }
        let mesh = Arc::new(mesh.clone());
        let instances = instances.erase();
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Instanced {
                mesh: Arc::clone(&mesh),
                submesh_index,
                instances: instances.clone(),
            });
        }
    }
    pub(crate) fn optimize(&mut self) -> Vec<DrawBatch> {
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        for command in self.queued.drain(..) {
            if command.is_transparent() {
                transparent.push(command);
            } else {
                opaque.push(command);
            }
        }
        opaque.sort_by_key(DrawCommand::sort_key);
        let mut batches: Vec<DrawBatch> = Vec::new();
        for command in opaque.into_iter().chain(transparent) {
            let mergeable = matches!(command, DrawCommand::Mesh { .. });
            let merge = mergeable
                && batches.last().is_some_and(|batch| {
                    matches!(batch.commands.first(), Some(DrawCommand::Mesh { .. }))
                        && batch.commands[0].sort_key() == command.sort_key()
                });
            if merge {
                if let DrawCommand::Mesh { transforms, .. } = command {
                    if let DrawCommand::Mesh {
                        transforms: previous,
                        ..
                    } = &mut batches.last_mut().unwrap().commands[0]
                    {
                        previous.extend(transforms);
                    }
                }
            } else {
                batches.push(DrawBatch {
                    commands: vec![command],
                });
            }
        }
        batches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_uniforms_are_readable_by_name() {
        let mut material = Material::new(vec![ShaderPass {
            name: "Forward".to_owned(),
            vertex_entry: "VSMain".to_owned(),
            fragment_entry: "PSMain".to_owned(),
            vertex_spirv: Arc::from([]),
            fragment_spirv: Arc::from([]),
            cull: CullMode::Back,
            queue: RenderQueue::Opaque,
        }])
        .unwrap();
        material
            .set_uniform("roughness", UniformValue::Float(0.5))
            .unwrap();
        assert_eq!(
            material.uniform("roughness"),
            Some(&UniformValue::Float(0.5))
        );
    }
}
