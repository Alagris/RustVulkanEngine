#![allow(
    unsafe_op_in_unsafe_fn,
    clippy::too_many_arguments,
    clippy::unnecessary_wraps
)]

mod app;

use app::App;
use anyhow::{anyhow, Result};
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::{Window, WindowBuilder};
use vulkanalia::Version;

use std::collections::HashSet;
use std::ffi::CStr;
use std::os::raw::c_void;

// Note: This trait was called `ExtDebugUtilsExtension` in versions of `vulkanalia` prior to `v0.31.0`.
use vulkanalia::vk::ExtDebugUtilsExtensionInstanceCommands;

use log::*;
use vulkanalia::loader::{LibloadingLoader, LIBRARY};
use vulkanalia::window as vk_window;
use vulkanalia::prelude::v1_0::*;



fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        unsafe { std::env::set_var("RUST_LOG", "debug"); }
    }
    pretty_env_logger::init();

    // Window

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("Vulkan Tutorial (Rust)")
        .with_inner_size(LogicalSize::new(1024, 768))
        .build(&event_loop)?;

    // App

    let mut app = App::create(&window)? ;
    event_loop.run(move |event, elwt| {
        match event {
            // Request a redraw when all events were processed.
            Event::AboutToWait => window.request_redraw(),
            Event::WindowEvent { event, .. } => match event {
                // Render a frame if our Vulkan app is not being destroyed.
                WindowEvent::RedrawRequested if !elwt.exiting() =>app.render(&window).unwrap(),
                // Destroy our Vulkan app.
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }
                _ => {}
            }
            _ => {}
        }
    })?;

    Ok(())
}

