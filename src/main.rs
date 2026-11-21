#![allow(unsafe_op_in_unsafe_fn, clippy::too_many_arguments)]

mod app;
mod graphics;
mod proc_mesh;
mod render_pipeline;

use anyhow::{Context, Result, anyhow};
use app::App;
use glam::{Mat4, Vec3};
use graphics::{Graphics, Material, ShaderPass};
use render_pipeline::RenderPipeline;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

fn read_spirv(path: PathBuf) -> Result<Arc<[u32]>> {
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
        .map(|word| u32::from_le_bytes(word.try_into().expect("chunk has four bytes")))
        .collect::<Vec<_>>();
    if words.first() != Some(&0x0723_0203) {
        return Err(anyhow!("{} is not a SPIR-V module", path.display()));
    }
    Ok(Arc::from(words))
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init();

    let mut pipeline_path = PathBuf::from("assets/pipeline.toml");
    let mut compile_only = false;
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
                println!("Usage: Game [--pipeline <path>] [--compile-shaders] [--frames <count>]");
                return Ok(());
            }
            _ => return Err(anyhow!("Unknown argument: {}", arg.to_string_lossy())),
        }
    }

    let pipeline = RenderPipeline::load(&pipeline_path)?;
    pipeline.compile_shaders()?;
    if compile_only {
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
    let compiled_dir = pipeline_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("compiled_shaders");
    let material = Arc::new(Material::new(vec![ShaderPass {
        name: first_pass.metadata.name.clone(),
        vertex_entry: "VSMain".to_owned(),
        fragment_entry: "PSMain".to_owned(),
        vertex_spirv: read_spirv(compiled_dir.join("pass_0.vert.spv"))?,
        fragment_spirv: read_spirv(compiled_dir.join("pass_0.frag.spv"))?,
        cull: first_pass.metadata.cull.clone(),
        queue: first_pass.metadata.queue.clone(),
    }])?);

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("Rust Vulkan Game Engine - Cube")
        .with_inner_size(LogicalSize::new(1024, 768))
        .build(&event_loop)?;
    let mut app = App::create(
        &window,
        &material,
        first_pass.clear_color.unwrap_or([0.035, 0.045, 0.07, 1.0]),
    )?;
    log::info!("Vulkan application initialized.");
    let context = app.gpu_context();
    let cube = proc_mesh::cube(&context, material)?;
    log::info!("Created GPU cube buffers.");
    let mut graphics = Graphics::default();
    let size = window.inner_size();
    let projection = Mat4::perspective_rh(
        60_f32.to_radians(),
        size.width as f32 / size.height.max(1) as f32,
        0.1,
        100.0,
    );
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
    let model = Mat4::from_rotation_y(0.65) * Mat4::from_rotation_x(-0.45);
    let transform = projection * view * model;

    let render_error = Arc::new(Mutex::new(None));
    let callback_error = Arc::clone(&render_error);
    let mut rendered_frames = 0;
    event_loop.run(move |event, elwt| match event {
        Event::AboutToWait => window.request_redraw(),
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::RedrawRequested if !elwt.exiting() => {
                graphics.draw(&cube, transform);
                match app.render(&mut graphics) {
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
                        *callback_error.lock().unwrap() = Some(format!("{error:#}"));
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
