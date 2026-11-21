use anyhow::{Context, Result, anyhow};
use editor::Project;
use engine::app::App;
use engine::graphics::{FrameUniform, Graphics};
use engine::material::{Material, ShaderPass};
use engine::proc_mesh;
use engine::render_pipeline::RenderPipeline;
use glam::{Mat4, Vec3, Vec3A, Vec4};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

fn read_spirv(path: PathBuf) -> Result<Arc<[u32]>> {
    assert!(path.is_absolute() || path.components().count() > 0);
    let bytes =
        std::fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    if bytes.len() % 4 != 0 {
        return Err(anyhow!(
            "SPIR-V file {} has a truncated word",
            path.display()
        ));
    }
    let words = bytes
        .chunks_exact(4)
        .map(|word| {
            assert_eq!(word.len(), 4, "SPIR-V chunks are exactly four bytes");
            let array: [u8; 4] = match word.try_into() {
                Ok(array) => array,
                Err(_) => [0, 0, 0, 0],
            };
            u32::from_le_bytes(array)
        })
        .collect::<Vec<_>>();
    assert!(
        !words.is_empty(),
        "SPIR-V module must contain at least one word"
    );
    if words.first() != Some(&0x0723_0203) {
        return Err(anyhow!("{} is not a SPIR-V module", path.display()));
    }
    Ok(Arc::from(words))
}

fn build_frame_uniform(
    view: Mat4,
    aspect: f32,
    fov_y: f32,
    near: f32,
    far: f32,
    cascade_count: u32,
) -> FrameUniform {
    assert!(aspect.is_finite() && aspect > 0.0);
    assert!(near.is_finite() && near > 0.0);
    assert!(far.is_finite() && far > near);
    assert!((1..=4).contains(&cascade_count));
    assert!(view.is_finite() && view.determinant().abs() > f32::EPSILON);

    let inverse_view = view.inverse();
    let camera_position = inverse_view.transform_point3(Vec3::ZERO);
    let camera_right = inverse_view.transform_vector3(Vec3::X).normalize();
    let camera_up = inverse_view.transform_vector3(Vec3::Y).normalize();
    let camera_forward = inverse_view.transform_vector3(-Vec3::Z).normalize();
    let tangent = (fov_y * 0.5).tan();
    let light_direction = Vec3::new(-2.0, -4.0, -1.0).normalize();
    let mut shadow_view_projection = [Mat4::IDENTITY.to_cols_array_2d(); 4];
    let mut cascade_splits = [far; 4];
    let mut previous_split = near;

    for cascade in 0..cascade_count as usize {
        let fraction = (cascade + 1) as f32 / cascade_count as f32;
        let logarithmic = near * (far / near).powf(fraction);
        let linear = near + (far - near) * fraction;
        let split = 0.5 * logarithmic + 0.5 * linear;
        cascade_splits[cascade] = split;

        let mut center = Vec3::ZERO;
        let mut corners = [Vec3::ZERO; 8];
        let mut corner_index = 0;
        for distance in [previous_split, split] {
            let half_height = tangent * distance;
            let half_width = half_height * aspect;
            for x_sign in [-1.0, 1.0] {
                for y_sign in [-1.0, 1.0] {
                    let corner = camera_position
                        + camera_forward * distance
                        + camera_right * (half_width * x_sign)
                        + camera_up * (half_height * y_sign);
                    corners[corner_index] = corner;
                    center += corner;
                    corner_index += 1;
                }
            }
        }
        assert_eq!(corner_index, corners.len());
        center /= corners.len() as f32;
        let radius = corners
            .iter()
            .map(|corner| corner.distance(center))
            .fold(0.0_f32, f32::max)
            .max(0.1);
        let light_up = if light_direction.dot(Vec3::Y).abs() > 0.98 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let light_view =
            Mat4::look_at_rh(center - light_direction * radius * 2.0, center, light_up);
        let mut light_projection =
            Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, radius * 4.0);
        light_projection.y_axis.y *= -1.0;
        let matrix = light_projection * light_view;
        assert!(matrix.is_finite());
        shadow_view_projection[cascade] = matrix.to_cols_array_2d();
        previous_split = split;
    }

    if cascade_count < 4 {
        for index in cascade_count as usize..4 {
            shadow_view_projection[index] = shadow_view_projection[cascade_count as usize - 1];
        }
    }
    cascade_splits[cascade_count as usize - 1] = far;
    let projection = Mat4::perspective_rh(fov_y, aspect, near, far);
    let mut vulkan_projection = projection;
    vulkan_projection.y_axis.y *= -1.0;
    let view_projection = vulkan_projection * view;
    assert!(view_projection.is_finite());

    FrameUniform {
        view_projection: view_projection.to_cols_array_2d(),
        view: view.to_cols_array_2d(),
        shadow_view_projection,
        cascade_splits: Vec4::from_array(cascade_splits).to_array(),
        cascade_index: 0,
        padding: [0; 3],
    }
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init();

    let mut pipeline_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("pipeline.pipe");
    let mut compile_only = false;
    let mut release_mode = false;
    let mut frame_limit = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--pipeline") => {
                pipeline_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--pipeline requires a TOML file path"))?;
            }
            Some("--compile-shaders") => compile_only = true,
            Some("--release") => release_mode = true,
            Some("--frames") => {
                let frames = args
                    .next()
                    .ok_or_else(|| anyhow!("--frames requires a positive frame count"))?
                    .to_string_lossy()
                    .parse::<u32>()
                    .context("--frames requires a positive integer")?;
                if frames == 0 {
                    return Err(anyhow!("--frames requires a positive frame count"));
                }
                frame_limit = Some(frames);
            }
            Some("--help") | Some("-h") => {
                println!(
                    "Usage: Game [--pipeline <path>] [--release] [--compile-shaders] [--frames <count>]"
                );
                return Ok(());
            }
            _ => return Err(anyhow!("Unknown argument: {}", arg.to_string_lossy())),
        }
    }

    let resolved_pipeline_path = if release_mode {
        pipeline_path.clone()
    } else {
        let asset_root = pipeline_path
            .parent()
            .ok_or_else(|| anyhow!("pipeline path has no parent directory"))?;
        let pipeline_name = pipeline_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| anyhow!("pipeline filename must be valid UTF-8"))?;
        Project::open(asset_root)?.prepare_development_pipeline(pipeline_name)?
    };
    let pipeline = RenderPipeline::load(&resolved_pipeline_path)?;
    pipeline.compile_shaders()?;
    if compile_only {
        log::info!("Shader compilation completed.");
        return Ok(());
    }
    let first_pass = pipeline
        .passes
        .first()
        .ok_or_else(|| anyhow!("render pipeline has no passes"))?;
    assert!(
        !first_pass.name.trim().is_empty(),
        "configured render pass names must not be empty"
    );
    if first_pass.color_target != "swapchain" {
        return Err(anyhow!(
            "the first render pass must target 'swapchain' in this renderer"
        ));
    }
    let compiled_dir = resolved_pipeline_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("compiled_shaders");
    let material_name = first_pass.material_name.clone();
    let shader_passes = pipeline
        .passes
        .iter()
        .enumerate()
        .filter(|(_, pass)| pass.material_name == material_name)
        .map(|(index, pass)| {
            Ok(ShaderPass {
                name: pass.metadata.name.clone(),
                vertex_entry: "VSMain".to_owned(),
                fragment_entry: "PSMain".to_owned(),
                vertex_spirv: read_spirv(compiled_dir.join(format!("pass_{index}.vert.spv")))?,
                fragment_spirv: read_spirv(compiled_dir.join(format!("pass_{index}.frag.spv")))?,
                cull: pass.metadata.cull.clone(),
                queue: pass.metadata.queue.clone(),
                vertex_inputs: pass.metadata.vertex_inputs.clone(),
                instance_inputs: pass.metadata.instance_inputs.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let material = Arc::new(Material::new(shader_passes)?);

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("Rust Vulkan Game Engine - Cube")
        .with_inner_size(LogicalSize::new(1024, 768))
        .build(&event_loop)?;
    let size = window.inner_size();
    let fov_y = 60_f32.to_radians();
    let near = 0.1;
    let far = 50.0;
    let view = Mat4::look_at_rh(Vec3::new(0.0, 3.0, 5.0), Vec3::ZERO, Vec3::Y);
    let frame_uniform = build_frame_uniform(
        view,
        size.width as f32 / size.height.max(1) as f32,
        fov_y,
        near,
        far,
        pipeline.render_buffers.shadow_cascades,
    );
    let mut app = App::create(
        &window,
        &material,
        first_pass.clear_color.unwrap_or([0.035, 0.045, 0.07, 1.0]),
        &pipeline,
        frame_uniform,
    )?;
    log::info!("Vulkan application initialized.");
    let context = app.gpu_context();
    let cube =
        proc_mesh::cube_with_size(&context, Arc::clone(&material), Vec3A::new(1.0, 1.5, 1.0))?;
    let plane = proc_mesh::plane_with_segments(&context, material, 12.0, 12.0, 8, 8)?;
    log::info!("Created GPU cube and plane buffers.");
    let mut graphics = Graphics::default();
    let cube_transform = Mat4::from_translation(Vec3::new(0.0, -0.15, 0.0))
        * Mat4::from_rotation_y(0.65)
        * Mat4::from_rotation_x(-0.45);

    let render_error = Arc::new(Mutex::new(None));
    let callback_error = Arc::clone(&render_error);
    let mut rendered_frames = 0;
    event_loop.run(move |event, elwt| match event {
        Event::AboutToWait => window.request_redraw(),
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::RedrawRequested if !elwt.exiting() => {
                let draw_result = graphics.draw(&cube, cube_transform).and_then(|()| {
                    graphics.draw(&plane, Mat4::from_translation(Vec3::new(0.0, -0.8, 0.0)))
                });
                let render_result = draw_result.and_then(|()| app.render(&mut graphics));
                match render_result {
                    Ok(()) => {
                        rendered_frames += 1;
                        if let Some(limit) = frame_limit {
                            log::info!("Rendered frame {rendered_frames}/{limit}.");
                        }
                        if frame_limit.is_some_and(|limit| rendered_frames >= limit) {
                            elwt.exit();
                        }
                    }
                    Err(error) => {
                        log::error!("Rendering failed: {error:#}");
                        match callback_error.lock() {
                            Ok(mut slot) => *slot = Some(format!("{error:#}")),
                            Err(poisoned) => *poisoned.into_inner() = Some(format!("{error:#}")),
                        }
                        elwt.exit();
                    }
                }
            }
            WindowEvent::CloseRequested => elwt.exit(),
            _ => {}
        },
        _ => {}
    })?;
    if let Some(error) = render_error
        .lock()
        .map_err(|_| anyhow!("render error state was poisoned"))?
        .take()
    {
        return Err(anyhow!("Rendering failed: {error}"));
    }
    Ok(())
}
