# Rust + Vulkan Game Engine

This starter has a Vulkan swapchain renderer, GPU-backed typed buffers, a
queued draw API with basic opaque sorting/batching, and a generated-grammar
shader format that supports multiple HLSL passes per file. HLSL is compiled
into SPIR-V through the embedded `shaderc` crate, with no external compiler
process. The sample draws a procedurally generated cube and plane and renders
cascaded cube shadows onto the plane.

## Workspace and run

From the repository root, with Vulkan and a Vulkan-capable GPU installed:

```powershell
cargo run -p game
```

For a bounded smoke run that exits after ten successfully presented frames:

```powershell
cargo run -p game -- --frames 10
```

The workspace has three crates: `engine` contains the graphics API and runtime,
`editor` indexes and resolves game assets, and `game` is the sample project.
The game project loads `game/assets/pipeline.pipe`; the editor resolves
`(asset name, asset type)` references to absolute development paths before the
engine sees them. HLSL is compiled into SPIR-V through the embedded `shaderc`
crate without launching an external compiler. The sample renders a parametric
cube and segmented plane, including cascaded cube shadows on the plane.

Compile shaders without opening a window:

```powershell
cargo run -p game -- --compile-shaders
```

Select another pipeline with
`cargo run -p game -- --pipeline path\to\pipeline.pipe`. Compiled SPIR-V is
written alongside the resolved development pipeline under the OS temporary
directory. The editor CLI supports asset indexing and release packaging:

```powershell
cargo run -p editor -- index game\assets
cargo run -p editor -- package game\assets pipeline build\release
```

Run a packaged pipeline directly (the engine reads the paths emitted by the
editor and does not rescan or resolve assets):

```powershell
cargo run -p game -- --release --pipeline build\release\pipeline.pipe
```

## Render pipeline and shader format

Render passes are configured in TOML and ordered as listed:

```toml
[pipeline]
name = "Forward"

[swapchain]
image_count = 3
present_mode = "fifo"
color_format = "bgra8_srgb"

[render_buffers.depth]
width = 0
height = 0
format = "d32_float"

[render_buffers.shadow]
width = 2048
height = 2048
format = "d32_float"
shadow_cascades = 4

[[passes]]
name = "ForwardOpaque"
material = "cube"
shader_pass = "Forward"
color_target = "swapchain"
clear_color = [0.08, 0.1, 0.14, 1.0]

[[passes]]
name = "ShadowCascades"
material = "cube"
shader_pass = "ShadowCascade"
color_target = "shadow_cascades"
```

Each material references its shader by plain name; the editor resolves it by
asset type:

```toml
[material]
name = "CubeLit"
shader = "unlit"
```

Shader files use a LALRPOP-generated LR(1) context-free grammar. Each pass
contains face-culling and queue metadata followed by an HLSL block; HLSL braces,
comments, and quoted strings are parsed as block content:

```text
shader "Unlit" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            // HLSL vertex and pixel entry points
        }
    }

    pass "ShadowCaster" {
        face "front"
        queue "opaque"
        hlsl {
            // Another pair of entry points in the same file
        }
    }
}
```

Shader metadata and the complete shader file are parsed with the LALRPOP
grammar in `engine/src/shader_grammar.lalrpop`. HLSL input structures are then
parsed in a second stage with a separate generated grammar. The current renderer
uses the first configured material pass as a graphics pipeline; all configured
passes are parsed and compiled.

## GPU buffer and draw API

`VertexBuffer`, `IndexBuffer`, and `InstanceBuffer<T>` allocate Vulkan GPU
buffers and implement `ComputeBuffer`. They expose checked element updates but
do not expose Vulkan handles through the public buffer trait. Buffers currently
use host-visible/coherent GPU memory for straightforward uploads.

`Mesh::new` takes vertex/index slices, one or more `Arc<Material>` values, and
submesh index ranges. `Graphics` queues draws until `App::render` sorts opaque
draws by material/mesh and combines compatible transform draws:

```rust
graphics.draw(&mesh, transform)?;
graphics.draw_instanced(&mesh, &instance_buffer)?;
graphics.draw_procedural(material, vertex_count, instance_count)?;
app.render(&mut graphics)?;
```

`Transform` is `glam::Mat4`; vertex positions/colors and uniform vector/matrix
values also use glam types. Material uniforms can be set and read by name via
`set_uniform` and `uniform`. `InstanceBuffer<T>` currently expects the
layout implemented by `InstanceData` for `T`; implement that trait to describe
your own GPU instance layout. Vertex buffers can likewise be created from raw
GPU bytes with a checked `VertexLayout`. Mesh construction and draw submission
validate those layouts against the HLSL inputs. `proc_mesh::cube_with_size`
accepts three dimensions, while `proc_mesh::plane_with_segments` accepts width,
depth, and independent X/Z segment counts.

The current sample provides a basic single-frame-in-flight renderer, a
configurable depth attachment and swapchain, GPU frame-descriptor uniforms,
and a four-cascade shadow depth-array pass. The cube and plane demonstrate
cube casting onto plane geometry. Swapchain recreation on window resize and
additional render-target pass scheduling remain follow-up work.
