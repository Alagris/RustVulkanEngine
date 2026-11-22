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

The workspace has four crates: `engine` contains the graphics API and runtime,
`component_macros` provides component derive/attribute macros, `editor` indexes
and resolves game assets, and `game` is the sample project.
The game project loads `game/assets/pipeline.pipe`; the editor resolves
`(asset name, asset type)` references to absolute development paths before the
engine sees them. HLSL is compiled into SPIR-V through the embedded `shaderc`
crate without launching an external compiler. The sample renders a parametric
cube and segmented plane, including cascaded cube shadows on the plane.

Compile shaders without opening a window:

```powershell
cargo run -p game -- --compile-shaders
```

Choose another project configuration with
`cargo run -p game -- --settings path\to\settings.yaml`. Compiled SPIR-V is
written alongside the resolved development pipeline under the OS temporary
directory. The editor CLI supports asset indexing and release packaging:

```powershell
cargo run -p editor -- index game\assets
cargo run -p editor -- package game\settings.yaml build\release
```

Run a packaged pipeline directly (the engine reads the paths emitted by the
editor and does not rescan or resolve assets):

```powershell
cargo run -p game -- --release --settings build\release\runtime_settings.yaml
```

## Render pipeline and shader format

Render passes and render targets are configured in YAML and ordered as listed:

```yaml
pipeline:
  name: Forward
swapchain:
  image_count: 3
  present_mode: fifo
  color_format: bgra8_srgb
render_buffers:
  depth:
    width: 0
    height: 0
    format: d32_float
  shadow:
    width: 2048
    height: 2048
    format: d32_float
    shadow_cascades: 4
passes:
  - name: ForwardOpaque
    material: cube
    shader_pass: Forward
    color_target: swapchain
    clear_color: [0.08, 0.1, 0.14, 1.0]
  - name: ShadowCascades
    material: cube
    shader_pass: ShadowCascade
    color_target: shadow_cascades
```

Each material references its shader by plain name; the editor resolves it by
asset type:

```yaml
material:
  name: CubeLit
  shader: CubeLit
```

Shader files use a LALRPOP-generated LR(1) context-free grammar. Each pass
contains face-culling and queue metadata followed by an HLSL block; HLSL braces,
comments, and quoted strings are parsed as block content:

```text
shader "CubeLit" {
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

GameObject transforms and `TransformDefinition` use `cgmath::Matrix4`,
`Vector3`, and quaternion rotations; matrices are converted to glam at the
renderer boundary. Vertex positions/colors and renderer uniform values use
glam types. Material uniforms can be set and read by name via
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

## Settings, scenes, and ECS

The editor reads a project `settings.yaml`. Runtime fields are flattened at the
top level, while `editor_scene` is development-only:

```yaml
pipeline: pipeline.pipe
default_scene: game.scene
editor_scene: editor.scene
```

In development, the editor resolves the pipeline and default scene and creates
a `RuntimeSettings` value. Release packaging writes a stripped
`runtime_settings.yaml` containing `pipeline`, `default_scene`, and the packaged
data asset directory.

Data assets are YAML `.data` files under the project's `assets` directory. They
are instantiated and awakened during editor scanning and runtime startup. A
data type can use `#[derive(DataAsset)]`; registrations are collected
automatically. The default loader and serializer expect the type to implement
`Deserialize` and `Serialize`. For custom serde
behavior, `#[data_asset(deserialize_with = path, serialize_with = path)]`
provides a deserializer with signature
`fn(&serde_yaml::Value, &DataAssetDeserializeContext) -> anyhow::Result<T>` and
a serializer with signature
`fn(&T) -> anyhow::Result<serde_yaml::Value>`. Derive `Modifier` alongside
`DataAsset` to make a type usable as a modifier asset.

```rust
#[derive(engine::data::DataAsset, serde::Deserialize, serde::Serialize)]
struct KeyMap {
    bindings: std::collections::BTreeMap<
        String,
        engine::data::DataRef<engine::data::key_binding::KeyBinding>,
    >,
}
```

Each file declares a registered type and a name, with optional same-type parent
inheritance:

```yaml
type: Input
name: PlayerControls
data:
  keymaps:
    Gameplay: { ref: Gameplay }
```

``DataRef<T>` values with `ref` are weak links to named assets. A directly
embedded value is owned by its containing data asset; nested modifiers use an
explicit `{ type, data }` mapping. Parent inheritance merges mappings
recursively, while cross-asset references may form cycles. Built-in data types
include `Input`, `KeyMap`, `KeyBinding`, and an animation clip configuration;
input processing and clip playback are not yet implemented.

Scene and prefab YAML share the same root object format. A scene can provide
`deltaTime`, `timeSinceStart`, a root object, children, transforms, and typed
component data. The engine initializes components in two phases (`awake`,
then `start`); components whose `start` returns `true` receive `update` each
frame. `update` now also receives the owning `GameObject` and `Graphics`, so
rendering and component updates happen in one callback. `awake`, `start`, and
`destroy` also receive their owning object.

Components can change an object's transform through `GameObject::set_transform`.
For changes to another object, use
`Scene::defer_game_object_mutation(object_id, |object| ...)`. Components are strongly owned by their `GameObject`; the scene keeps weak
references for component and object traversal, compacting expired references
during updates. Components choose which typed `ComponentWeak<T>` handles to
cache themselves, avoiding repeated type searches without a shared sibling
cache. Structural component
additions/removals use `GameObject::add_component(scene, ...)` and
`GameObject::remove_component::<T>(scene)`; they are applied after the current
component-update traversal.
Parents own their child objects directly. `GameObject::weak_reference` creates
a non-owning handle that can be resolved through `Scene::resolve_game_object`;
the scene prunes expired object handles during update.

Use the `#[component]` attribute with `#[derive(Component)]` to inject the
`ComponentInstance` field (marked `#[serde(skip)]` when the type derives serde)
and generate `Deref`/`DerefMut` plus component validation metadata. Component
constructors initialize the injected field with
`ComponentInstance::default()`. Add `#[require(TypeA, TypeB)]` to require sibling
components and `#[unique]` to restrict the component to one instance per
`GameObject`:

```rust
#[component]
#[derive(Component)]
#[require(SkinnedMesh)]
#[unique]
struct Animator {
    // Component data
}
```

Component requirements and uniqueness are emitted as compile-time metadata by
`#[derive(Component)]`. The editor checks those rules while scanning scene and
prefab assets. Scene and prefab loading do not perform these development-time
checks, and release builds compile out the validator. Runtime factories
remain registered with `ComponentRegistry`; components that need GPU or
material context are registered when the render loop is initialized.
`ecs::Mesh`, `ecs::ProceduralMesh`, and `ecs::InstancedMesh` queue graphics API
draw calls through their unified `update` callback. `Camera` is serializable
and supplies the frame and cascaded-shadow uniforms.

The engine exposes `graphics::fbx::load_meshes_from_fbx` and
`graphics::fbx::load_animations_from_fbx` using the pure-Rust `fbx-reader`
crate. Imported meshes use the supplied material and preserve FBX model
transforms and units. `SkinnedMesh` stores skeleton and inverse-bind data;
`Animator` selects a loaded clip, finds a `SkinnedMesh` on the same
`GameObject`, evaluates bone poses, prepares per-vertex blend matrices, and
dispatches vertex deformation through an embedded Vulkan compute shader. The
current FBX reader exposes per-control-point influences only through its CPU
API, so matrix blending is prepared on the CPU while vertex transformation is
performed on the GPU.

The sample scene uses the same component schema:

```yaml
root:
  name: Game
  components:
    - type: Camera
  children:
    - name: Cube
      transform:
        translation: [0.0, -0.15, 0.0]
        rotation: [-0.21140331, 0.31124407, 0.07122724, 0.9237762]
        scale: [1.0, 1.5, 1.0]
      components:
        - type: Mesh
          data:
            primitive: cube
            material: CubeLit
            size: [1.0, 1.0, 1.0]
```

The editor CLI owns runtime options; the game binary is only a small entry
point that supplies its project settings path:

```powershell
cargo run -p game -- --frames 10
cargo run -p editor -- run --settings game\settings.yaml --compile-shaders
```
