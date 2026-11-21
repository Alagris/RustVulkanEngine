use crate::material::Material;
use crate::render_pipeline::{
    RenderQueue, ShaderInputAttribute, ShaderInputFormat, ShaderInputLayout,
};
use anyhow::{Result, anyhow, bail};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3A, Vec4};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vulkanalia::prelude::v1_0::*;
use vulkanalia::vk::{ExtDebugUtilsExtensionInstanceCommands, KhrSurfaceExtensionInstanceCommands};

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: Vec3A,
    pub color: Vec4,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexLayout {
    pub stride: u32,
    pub attributes: Vec<ShaderInputAttribute>,
}

impl VertexLayout {
    pub fn new(stride: u32, attributes: Vec<ShaderInputAttribute>) -> Result<Self> {
        validate_input_layout(stride, &attributes)?;
        Ok(Self { stride, attributes })
    }

    fn validate_shader_inputs(&self, expected: &ShaderInputLayout) -> Result<()> {
        validate_input_layout(self.stride, &self.attributes)?;
        for input in &expected.attributes {
            let actual = self
                .attributes
                .iter()
                .find(|attribute| attribute.location == input.location)
                .ok_or_else(|| {
                    anyhow!(
                        "vertex layout is missing shader location {}",
                        input.location
                    )
                })?;
            if actual.format != input.format || actual.offset != input.offset {
                bail!(
                    "vertex layout location {} expects {:?} at byte {}, got {:?} at byte {}",
                    input.location,
                    input.format,
                    input.offset,
                    actual.format,
                    actual.offset
                );
            }
        }
        Ok(())
    }
}

fn validate_input_layout(stride: u32, attributes: &[ShaderInputAttribute]) -> Result<()> {
    if stride == 0 || attributes.is_empty() {
        bail!("buffer layouts require a positive stride and at least one attribute");
    }
    let mut locations = std::collections::HashSet::new();
    for attribute in attributes {
        if !locations.insert(attribute.location) {
            bail!(
                "buffer layout repeats shader location {}",
                attribute.location
            );
        }
        let end = attribute
            .offset
            .checked_add(attribute.format.byte_size())
            .ok_or_else(|| anyhow!("buffer attribute range overflow"))?;
        if end > stride {
            bail!(
                "buffer attribute at location {} extends beyond its stride",
                attribute.location
            );
        }
    }
    Ok(())
}

impl Vertex {
    pub fn layout() -> VertexLayout {
        VertexLayout {
            stride: std::mem::size_of::<Self>() as u32,
            attributes: vec![
                ShaderInputAttribute {
                    location: 0,
                    format: ShaderInputFormat::Float32x3,
                    offset: std::mem::offset_of!(Self, position) as u32,
                },
                ShaderInputAttribute {
                    location: 1,
                    format: ShaderInputFormat::Float32x4,
                    offset: std::mem::offset_of!(Self, color) as u32,
                },
            ],
        }
    }
}

pub type Transform = Mat4;

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct FrameUniform {
    pub view_projection: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub shadow_view_projection: [[[f32; 4]; 4]; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_index: u32,
    pub padding: [u32; 3],
}

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
        assert!(
            std::mem::size_of::<T>() > 0,
            "GPU element types must be sized"
        );
        assert!(
            matches!(
                usage,
                vk::BufferUsageFlags::VERTEX_BUFFER
                    | vk::BufferUsageFlags::INDEX_BUFFER
                    | vk::BufferUsageFlags::UNIFORM_BUFFER
            ),
            "GPU buffer wrapper only permits typed vertex, index, or uniform usage"
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
        validate_input_layout(layout.stride, &layout.attributes)?;
        let expected_bytes = (layout.stride as usize)
            .checked_mul(vertex_count)
            .ok_or_else(|| anyhow!("vertex buffer size overflow"))?;
        if vertex_count == 0 || bytes.len() != expected_bytes {
            bail!("vertex bytes must exactly match the declared layout and vertex count");
        }
        Ok(Self {
            buffer: Buffer::new(context, bytes, vk::BufferUsageFlags::VERTEX_BUFFER)?,
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
        assert!(context.device().handle() != vk::Device::null());
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
        let vertex_buffer = VertexBuffer::new(context, vertices)?;
        let index_buffer = IndexBuffer::new(context, indices)?;
        Self::from_buffers(
            vertex_buffer,
            index_buffer,
            materials,
            submeshes,
            <Mat4 as InstanceData>::layout(),
        )
    }

    pub fn from_buffers(
        vertices: VertexBuffer,
        indices: IndexBuffer,
        materials: Vec<Arc<Material>>,
        submeshes: Vec<Submesh>,
        instance_layout: VertexLayout,
    ) -> Result<Self> {
        if vertices.vertex_count() == 0
            || indices.len() == 0
            || materials.is_empty()
            || submeshes.is_empty()
        {
            bail!("meshes require vertices, indices, materials, and submeshes");
        }
        for material in &materials {
            Self::validate_material_layouts(material, &vertices.layout, &instance_layout)?;
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
            vertices,
            indices,
            materials,
            submeshes,
        })
    }

    fn validate_material_layouts(
        material: &Material,
        vertex_layout: &VertexLayout,
        instance_layout: &VertexLayout,
    ) -> Result<()> {
        let expected_vertex_stride = material
            .passes
            .iter()
            .map(|pass| pass.vertex_inputs.stride)
            .max()
            .unwrap_or(0);
        let expected_instance_stride = material
            .passes
            .iter()
            .map(|pass| pass.instance_inputs.stride)
            .max()
            .unwrap_or(0);
        if expected_vertex_stride != vertex_layout.stride {
            bail!(
                "mesh vertex stride {} does not match material '{}' stride {}",
                vertex_layout.stride,
                material.id(),
                expected_vertex_stride
            );
        }
        if expected_instance_stride != instance_layout.stride {
            bail!(
                "mesh instance stride {} does not match material '{}' stride {}",
                instance_layout.stride,
                material.id(),
                expected_instance_stride
            );
        }
        for pass in &material.passes {
            vertex_layout.validate_shader_inputs(&pass.vertex_inputs)?;
            instance_layout.validate_shader_inputs(&pass.instance_inputs)?;
        }
        Ok(())
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

fn validate_material_layouts_for_draw(mesh: &Mesh, instance_layout: &VertexLayout) -> Result<()> {
    for material in &mesh.materials {
        Mesh::validate_material_layouts(material, &mesh.vertices.layout, instance_layout)?;
    }
    Ok(())
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
    pub fn draw(&mut self, mesh: &Mesh, transform: Transform) -> Result<()> {
        assert!(
            transform.is_finite() && transform.determinant().abs() > f32::EPSILON,
            "draw transforms must be finite and invertible"
        );
        assert!(
            !mesh.submeshes.is_empty(),
            "draw mesh must contain submeshes"
        );
        validate_material_layouts_for_draw(mesh, &<Mat4 as InstanceData>::layout())?;
        let mesh = Arc::new(mesh.clone());
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Mesh {
                mesh: Arc::clone(&mesh),
                submesh_index,
                transforms: vec![transform],
            });
        }
        Ok(())
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
    pub fn draw_instanced<T: InstanceData>(
        &mut self,
        mesh: &Mesh,
        instances: &InstanceBuffer<T>,
    ) -> Result<()> {
        self.draw_instanced_buffer(mesh, instances.erase())
    }

    pub(crate) fn draw_instanced_buffer(
        &mut self,
        mesh: &Mesh,
        instances: InstanceBufferHandle,
    ) -> Result<()> {
        assert!(
            !mesh.submeshes.is_empty(),
            "instanced draw mesh must contain submeshes"
        );
        assert!(instances.len() > 0, "instanced draw must contain instances");
        if instances.len() == 0 {
            bail!("instanced draw must contain at least one instance");
        }
        validate_material_layouts_for_draw(mesh, instances.layout())?;
        for material in &mesh.materials {
            Mesh::validate_material_layouts(material, &mesh.vertices.layout, instances.layout())?;
        }
        let mesh = Arc::new(mesh.clone());
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Instanced {
                mesh: Arc::clone(&mesh),
                submesh_index,
                instances: instances.clone(),
            });
        }
        Ok(())
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
                    assert!(
                        !batches.is_empty(),
                        "mergeable batch must have a preceding batch"
                    );
                    let batch = match batches.last_mut() {
                        Some(batch) => batch,
                        None => unreachable!("preceding mergeable batch was asserted"),
                    };
                    assert_eq!(
                        batch.commands.len(),
                        1,
                        "mergeable mesh batch must contain exactly one source command"
                    );
                    if let DrawCommand::Mesh {
                        transforms: previous,
                        ..
                    } = &mut batch.commands[0]
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
