#![allow(unsafe_op_in_unsafe_fn, clippy::too_many_arguments)]

pub mod app;
pub mod ecs;
pub mod graphics;
pub mod material;
mod parsers;
pub mod proc_mesh;
pub mod render_loop;
pub mod render_pipeline;
pub mod settings;

pub use app::App;
pub use graphics::{FrameUniform, Graphics};
pub use material::{Material, ShaderPass};
pub use render_pipeline::RenderPipeline;
