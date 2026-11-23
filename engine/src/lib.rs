#![allow(unsafe_op_in_unsafe_fn, clippy::too_many_arguments)]

extern crate self as engine;

pub mod anim;
pub mod app;
pub mod assets;
pub mod data;
pub mod ecs;
pub mod graphics;
pub mod input;
pub mod material;
mod parsers;
pub mod postprocess;
pub mod proc_mesh;
pub mod render_loop;
pub mod render_pipeline;
pub mod settings;
pub mod ui;

pub use app::App;
pub use graphics::{FrameUniform, Graphics};
pub use material::{Material, ShaderPass};
pub use physx_safe as physics;
pub use render_pipeline::RenderPipeline;
