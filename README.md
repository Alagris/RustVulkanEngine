# Rust Vulkan Game Engine

This starter has a Vulkan swapchain renderer, GPU-backed typed buffers, a
queued draw API with basic opaque sorting/batching, and a generated-grammar
shader format that supports multiple HLSL passes per file. The sample starts
the renderer and draws a procedurally generated, depth-tested cube.

## Run

From the repository root, with Vulkan and a Vulkan-capable GPU installed:

```powershell
cargo run
```

For a bounded smoke run that exits after ten successfully presented frames:

```powershell
cargo run -- --frames 10
```

By default the engine loads `assets/pipeline.toml`, resolves shader paths
relative to that file, compiles all configured passes with `dxc`, creates a
Vulkan window/swapchain, and draws the cube. `dxc` must be on `PATH`; set `DXC`
to its executable path to use a non-default location. You can compile shaders
without opening a window with:

```powershell
cargo run -- --compile-shaders
```

Select another pipeline configuration with `cargo run -- --pipeline path\to\pipeline.toml`.
Compiled HLSL and SPIR-V files are written to `assets/compiled_shaders/`.
DXC targets Vulkan 1.0 SPIR-V to match the instance API version used by the
renderer.

## Render pipeline and shader format

Render passes are configured in TOML and ordered as listed:

```toml
[pipeline]
name = "Forward"

[[passes]]
name = "ForwardOpaque"
shader = "shaders/unlit.shader"
shader_pass = "Forward"
color_target = "swapchain"
clear_color = [0.08, 0.1, 0.14, 1.0]

[[passes]]
name = "ShadowMap"
shader = "shaders/unlit.shader"
shader_pass = "ShadowCaster"
color_target = "shadow_map"
```

Shader files use a LALRPOP-generated LR(1) context-free grammar. Each pass
contains face-culling and queue metadata followed by an HLSL block; HLSL braces,
comments, and quoted strings are parsed as block content:

```text
shader "Unlit" {
    pass "Forward" {
        face = back;
        queue = opaque;
        hlsl {
            // HLSL vertex and pixel entry points
        }
    }

    pass "ShadowCaster" {
        face = front;
        queue = opaque;
        hlsl {
            // Another pair of entry points in the same file
        }
    }
}
```

The grammar is in `src/shader_grammar.lalrpop`. The current renderer uses the
first configured material pass as a graphics pipeline; all configured passes
are still parsed and compiled.

## GPU buffer and draw API

`VertexBuffer`, `IndexBuffer`, and `InstanceBuffer<T>` allocate Vulkan GPU
buffers and implement `ComputeBuffer`. They expose checked element updates but
do not expose Vulkan handles through the public buffer trait. Buffers currently
use host-visible/coherent GPU memory for straightforward uploads.

`Mesh::new` takes vertex/index slices, one or more `Arc<Material>` values, and
submesh index ranges. `Graphics` queues draws until `App::render` sorts opaque
draws by material/mesh and combines compatible transform draws:

```rust
graphics.draw(&mesh, transform);
graphics.draw_instanced(&mesh, &instance_buffer);
graphics.draw_procedural(material, vertex_count, instance_count)?;
app.render(&mut graphics)?;
```

`Transform` is `glam::Mat4`; vertex positions/colors and uniform vector/matrix
values also use glam types. Material uniforms can be set and read by name via
`set_uniform` and `uniform`. `InstanceBuffer<T>` currently expects the
matrix-shaped 64-byte instance layout consumed by the sample vertex shader.

The current sample provides a basic single-frame-in-flight renderer and a depth
attachment. Swapchain recreation on window resize, descriptor-backed GPU
uniform uploads, and execution of secondary configured render targets such as
shadow maps remain follow-up work.
