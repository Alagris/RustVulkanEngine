use crate::app::App;
use crate::ecs::{Camera, ComponentRegistry, InstancedMesh, Mesh, ProceduralMesh, Scene};
use crate::graphics::{GpuContext, Graphics, InstanceBuffer, Mesh as GraphicsMesh};
use crate::material::{Material, ShaderPass};
use crate::proc_mesh;
use crate::render_pipeline::RenderPipeline;
use crate::settings::RuntimeSettings;
use anyhow::{Context, Result, anyhow};
use glam::{Mat4, Vec3A};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

#[derive(Clone, Debug, Default)]
pub struct RenderLoopOptions {
    pub frame_limit: Option<u32>,
    pub compile_only: bool,
    pub title: String,
}

pub struct RenderLoop;

impl RenderLoop {
    pub fn run(
        settings: RuntimeSettings,
        options: RenderLoopOptions,
        mut registry: ComponentRegistry,
    ) -> Result<()> {
        if options.frame_limit == Some(0) {
            return Err(anyhow!("frame limit must be greater than zero"));
        }
        let pipeline = RenderPipeline::load(&settings.pipeline)?;
        pipeline.compile_shaders()?;
        if options.compile_only {
            log::info!("Shader compilation completed.");
            return Ok(());
        }
        let first_pass = pipeline
            .passes
            .first()
            .ok_or_else(|| anyhow!("render pipeline has no passes"))?;
        if first_pass.color_target != "swapchain" {
            return Err(anyhow!(
                "the first render pass must target 'swapchain' in this renderer"
            ));
        }
        let materials = load_materials(&pipeline)?;
        let first_material = materials
            .get(&first_pass.material_name)
            .ok_or_else(|| anyhow!("first render pass material was not compiled"))?;
        let default_camera = Camera::default();

        let event_loop = EventLoop::new()?;
        let window = WindowBuilder::new()
            .with_title(if options.title.is_empty() {
                "Rust Vulkan Game Engine"
            } else {
                &options.title
            })
            .with_inner_size(LogicalSize::new(1024, 768))
            .build(&event_loop)?;
        let size = window.inner_size();
        let frame_uniform = default_camera.frame_uniform(
            size.width as f32 / size.height.max(1) as f32,
            pipeline.render_buffers.shadow_cascades,
        )?;
        let mut app = App::create(
            &window,
            first_material,
            first_pass.clear_color.unwrap_or([0.035, 0.045, 0.07, 1.0]),
            &pipeline,
            frame_uniform,
        )?;
        let context = app.gpu_context();
        register_mesh_components(&mut registry, context, materials)?;
        let mut scene = Scene::load(&settings.default_scene, &registry)?;
        scene.initialize()?;
        let size = window.inner_size();
        let initial_frame = frame_for_scene(
            &scene,
            &default_camera,
            size.width as f32 / size.height.max(1) as f32,
            pipeline.render_buffers.shadow_cascades,
        )
        .and_then(|frame| app.set_frame_uniform(frame));
        if let Err(error) = initial_frame {
            return match scene.destroy() {
                Ok(()) => Err(error),
                Err(destroy_error) => Err(anyhow!(
                    "runtime setup failed: {error:#}; scene destruction also failed: {destroy_error:#}"
                )),
            };
        }
        let scene = Rc::new(RefCell::new(scene));
        let callback_scene = Rc::clone(&scene);
        let mut graphics = Graphics::default();
        let render_error = Arc::new(Mutex::new(None));
        let callback_error = Arc::clone(&render_error);
        let mut rendered_frames = 0_u32;
        let mut previous_frame = Instant::now();

        let event_result = event_loop.run(move |event, elwt| match event {
            Event::AboutToWait => window.request_redraw(),
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::RedrawRequested if !elwt.exiting() => {
                    let size = window.inner_size();
                    if size.width == 0 || size.height == 0 {
                        return;
                    }
                    let now = Instant::now();
                    let delta = now.duration_since(previous_frame).as_secs_f32();
                    previous_frame = now;
                    let update_result = callback_scene.borrow_mut().update(delta, &mut graphics);
                    let result = update_result
                        .and_then(|()| {
                            let frame = frame_for_scene(
                                &callback_scene.borrow(),
                                &default_camera,
                                size.width as f32 / size.height as f32,
                                pipeline.render_buffers.shadow_cascades,
                            )?;
                            app.set_frame_uniform(frame)
                        })
                        .and_then(|()| app.render(&mut graphics));
                    match result {
                        Ok(()) => {
                            rendered_frames += 1;
                            if let Some(limit) = options.frame_limit {
                                log::info!("Rendered frame {rendered_frames}/{limit}.");
                                if rendered_frames >= limit {
                                    elwt.exit();
                                }
                            }
                        }
                        Err(error) => {
                            log::error!("Rendering failed: {error:#}");
                            match callback_error.lock() {
                                Ok(mut slot) => *slot = Some(format!("{error:#}")),
                                Err(poisoned) => {
                                    *poisoned.into_inner() = Some(format!("{error:#}"))
                                }
                            }
                            elwt.exit();
                        }
                    }
                }
                WindowEvent::CloseRequested => elwt.exit(),
                _ => {}
            },
            _ => {}
        });
        let destroy_result = scene.borrow().destroy();
        if let Err(event_error) = event_result {
            return match destroy_result {
                Ok(()) => Err(event_error.into()),
                Err(destroy_error) => Err(anyhow!(
                    "event loop failed: {event_error}; scene destruction also failed: {destroy_error:#}"
                )),
            };
        }
        destroy_result?;
        if let Some(error) = render_error
            .lock()
            .map_err(|_| anyhow!("render error state was poisoned"))?
            .take()
        {
            return Err(anyhow!("Rendering failed: {error}"));
        }
        Ok(())
    }
}

fn frame_for_scene(
    scene: &Scene,
    default_camera: &Camera,
    aspect: f32,
    cascade_count: u32,
) -> Result<crate::graphics::FrameUniform> {
    match scene.find_component::<Camera>() {
        Some(camera) => camera.frame_uniform(aspect, cascade_count),
        None => default_camera.frame_uniform(aspect, cascade_count),
    }
}

fn load_materials(pipeline: &RenderPipeline) -> Result<HashMap<String, Arc<Material>>> {
    let compiled_dir = pipeline.output_dir.clone();
    let mut passes_by_material: HashMap<String, Vec<ShaderPass>> = HashMap::new();
    for (index, pass) in pipeline.passes.iter().enumerate() {
        passes_by_material
            .entry(pass.material_name.clone())
            .or_default()
            .push(ShaderPass {
                name: pass.metadata.name.clone(),
                vertex_entry: "VSMain".to_owned(),
                fragment_entry: "PSMain".to_owned(),
                vertex_spirv: read_spirv(&compiled_dir.join(format!("pass_{index}.vert.spv")))?,
                fragment_spirv: read_spirv(&compiled_dir.join(format!("pass_{index}.frag.spv")))?,
                cull: pass.metadata.cull.clone(),
                queue: pass.metadata.queue.clone(),
                vertex_inputs: pass.metadata.vertex_inputs.clone(),
                instance_inputs: pass.metadata.instance_inputs.clone(),
            });
    }
    passes_by_material
        .into_iter()
        .map(|(name, passes)| Ok((name, Arc::new(Material::new(passes)?))))
        .collect()
}

#[derive(Deserialize)]
#[serde(default)]
struct PrimitiveMeshSettings {
    primitive: String,
    material: String,
    size: [f32; 3],
    width: f32,
    depth: f32,
    segments_x: u32,
    segments_z: u32,
}

impl Default for PrimitiveMeshSettings {
    fn default() -> Self {
        Self {
            primitive: "cube".to_owned(),
            material: String::new(),
            size: [1.0; 3],
            width: 10.0,
            depth: 10.0,
            segments_x: 1,
            segments_z: 1,
        }
    }
}

#[derive(Deserialize)]
struct ProceduralMeshSettings {
    material: String,
    vertices: i32,
    #[serde(default = "one")]
    instances: i32,
}

#[derive(Deserialize)]
struct InstancedMeshSettings {
    #[serde(flatten)]
    primitive: PrimitiveMeshSettings,
    transforms: Vec<[f32; 16]>,
}

fn register_mesh_components(
    registry: &mut ComponentRegistry,
    context: GpuContext,
    materials: HashMap<String, Arc<Material>>,
) -> Result<()> {
    let materials = Arc::new(materials);
    {
        let context = context.clone();
        let materials = Arc::clone(&materials);
        registry.register_typed::<Mesh>("Mesh", move |data| {
            let settings: PrimitiveMeshSettings = serde_yaml::from_value(data.clone())?;
            let material = get_material(&materials, &settings.material)?;
            let mesh = create_primitive_mesh(&context, material, &settings)?;
            Ok(Mesh::new(mesh))
        })?;
    }
    {
        let materials = Arc::clone(&materials);
        registry.register_typed::<ProceduralMesh>("ProceduralMesh", move |data| {
            let settings: ProceduralMeshSettings = serde_yaml::from_value(data.clone())?;
            let material = get_material(&materials, &settings.material)?;
            ProceduralMesh::new(material, settings.vertices, settings.instances)
        })?;
    }
    {
        registry.register_typed::<InstancedMesh>("InstancedMesh", move |data| {
            let settings: InstancedMeshSettings = serde_yaml::from_value(data.clone())?;
            let material = get_material(&materials, &settings.primitive.material)?;
            let mesh = create_primitive_mesh(&context, material, &settings.primitive)?;
            let transforms = settings
                .transforms
                .into_iter()
                .map(|matrix| Mat4::from_cols_array(&matrix))
                .collect::<Vec<_>>();
            let instances = InstanceBuffer::new(&context, &transforms)?;
            Ok(InstancedMesh::new(mesh, &instances))
        })?;
    }
    Ok(())
}

fn create_primitive_mesh(
    context: &GpuContext,
    material: Arc<Material>,
    settings: &PrimitiveMeshSettings,
) -> Result<GraphicsMesh> {
    match settings.primitive.as_str() {
        "cube" => proc_mesh::cube_with_size(context, material, Vec3A::from_array(settings.size)),
        "plane" => proc_mesh::plane_with_segments(
            context,
            material,
            settings.width,
            settings.depth,
            settings.segments_x,
            settings.segments_z,
        ),
        other => Err(anyhow!("unsupported procedural primitive '{other}'")),
    }
}

fn get_material(materials: &HashMap<String, Arc<Material>>, name: &str) -> Result<Arc<Material>> {
    materials
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow!("scene refers to unknown material '{}'", name))
}

fn one() -> i32 {
    1
}

fn read_spirv(path: &Path) -> Result<Arc<[u32]>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err(anyhow!(
            "SPIR-V file {} must contain whole, non-empty words",
            path.display()
        ));
    }
    let words = bytes
        .chunks_exact(4)
        .map(|word| {
            let word: [u8; 4] = word
                .try_into()
                .expect("chunks_exact yields four-byte words");
            u32::from_le_bytes(word)
        })
        .collect::<Vec<_>>();
    if words.first() != Some(&0x0723_0203) {
        return Err(anyhow!("{} is not a SPIR-V module", path.display()));
    }
    Ok(Arc::from(words))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_zero_frame_limit() {
        let result = RenderLoop::run(
            RuntimeSettings {
                pipeline: PathBuf::from("missing.pipe"),
                default_scene: PathBuf::from("missing.scene"),
            },
            RenderLoopOptions {
                frame_limit: Some(0),
                ..RenderLoopOptions::default()
            },
            ComponentRegistry::new(),
        );
        assert!(result.unwrap_err().to_string().contains("frame limit"));
    }
}
