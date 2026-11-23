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

The workspace includes `engine` (graphics API and runtime), `component_macros`,
`editor`, `game`, the vendored PhysX bindings (`physx` and `physx-sys`), the
`physx-safe` engine-facing wrapper, and the upstream `pxbind` binding generator.
The game project loads `game/assets/pipeline.pipe`; the editor resolves
authoring names into rendering paths and reusable weak asset references before
runtime startup. HLSL is compiled into SPIR-V through the embedded `shaderc`
crate without launching an external compiler. The sample renders a parametric
cube and segmented plane, including cascaded cube shadows on the plane.

The PhysX source is vendored under `physics/physx-rs` rather than included as a
Git submodule. `physx-safe` provides validated collider/material descriptors
and a thread-affine physics-world wrapper that does not expose native pointers.
The current wrapper runs PhysX on the CPU; CUDA context creation and PhysX GPU
library linking remain future work. See [`physics/README.md`](physics/README.md)
for provenance, build requirements, and known safety boundaries.

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
editor; reusable prefab/UI/data assets are loaded by path without editor name
resolution):

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
input_asset: PlayerControls
active_input_map: Gameplay
```

In development, the editor resolves the pipeline and default scene and creates
a `RuntimeSettings` value. Release packaging writes a stripped
`runtime_settings.yaml` containing `pipeline`, `default_scene`, the packaged
data asset directory, and any configured input asset/map.

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
`DataAsset` to make a type usable as a modifier asset, and declare its
`input_type`, `output_type`, and runtime compiler in `#[modifier(...)]`.

```rust
#[derive(engine::data::DataAsset, serde::Deserialize, serde::Serialize)]
struct KeyMap {
    bindings: std::collections::BTreeMap<
        String,
        engine::data::DataRef<engine::input::KeyBinding>,
    >,
}
```

Input assets select an input map and name the active map in runtime settings.
For example:

```yaml
type: Input
name: PlayerControls
data:
  keymaps:
    Gameplay:
      type: KeyMap
      data:
        bindings:
          jump:
            type: KeyBinding
            data:
              key: 62
              input_type: Bool
              modifiers:
                - type: Tap
                  data: {}
```

`DataRef<T>` values with `ref` are weak links to named assets. Inline
`{ type, data }` values are owned by their containing data asset. Input
bindings are compiled into source-to-binding lookup tables when the runtime
creates its input system. `Scene::set_active_input_map` changes maps, and
components can subscribe with `Scene::subscribe_input(map, binding, component,
callback)`. The callback is a typed `InputCallback<T>` function pointer that
receives the component, map name, numeric key code, `InputState`, and value.
Non-capturing closures can be used as callbacks. Subscriptions hold weak
component references and expired components are removed automatically.
`Scene::unsubscribe_input(component)` removes all callbacks for that component;
`Scene::unsubscribe_input_callback(component, callback)` removes that callback.
Keyboard buttons emit `Started` on press, `Ongoing` each frame while held, and
`Ended` on release. Modifiers can emit `Triggered` or `Canceled`; cancellation
ends the active input without a later `Ended`. Mouse buttons, movement, and
scroll emit `Ongoing` only, including when a modifier attempts a lifecycle state.

The built-in modifiers are `Swizzle` (swap Vector2 axes), `SwizzleX` (convert a
float to `(x, 0)`), `Tap`, `DoubleTap` (`max_interval` in seconds), and `Hold`
(`duration` in seconds). Modifiers declare fixed input and output types in
their `#[modifier(...)]` metadata; invalid chains are rejected while assets
are deserialized. Keyboard sources are numeric Winit `KeyCode` discriminants
(for example, `Space` is `62` in the current Winit version). Synthetic inputs
use the constants in `engine::input::codes`: `MOUSE_LEFT`, `MOUSE_RIGHT`,
`MOUSE_MIDDLE`, `MOUSE_BACK`, `MOUSE_FORWARD`, `MOUSE_MOVE`, and `SCROLL`.
Wheel values are `-1` or `1`.

The game window confines and hides the cursor by default while focused, and
releases it and shows it when focus is lost. `Scene::capture_mouse`,
`confine_mouse`, `release_mouse`, and `set_mouse_visible` change this behavior;
`set_mouse_grab` accepts a `MouseGrabMode`. Capture attempts cursor locking and
falls back to confinement if locking is unavailable. Grabbed mouse movement uses
relative device motion; free movement uses cursor position deltas. Mouse control
requires an attached game window and reports failures.

Scene and prefab YAML share the same root object format. A scene can provide
`deltaTime`, `timeSinceStart`, a root object, children, transforms, and typed
component data. The engine initializes components in two phases (`awake`,
then `try_start`); its `Features` result selects the `update`, `sync_physics`
and `late_update` callbacks the component receives each
frame. Import `engine::ecs::{UPDATE, PHYSICS, LATE}` and return combinations
such as `Ok(UPDATE | PHYSICS)`; `Features::empty()` requests none.
`update` now also receives the owning `GameObject` and `Graphics`, so
rendering and component updates happen in one callback. `awake`, `try_start`, and
`destroy` also receive their owning object.

Components can change an object's transform through `GameObject::set_transform`.
The local `TransformDefinition` exposes translation, quaternion rotation and
scale; `world_transform()` is the composed matrix. `set_world_translation`
converts a world-space position into the object's parent space.
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

### Runtime asset references

`engine::assets::Prefab` is available without the editor. `AssetStore::load`
loads reusable `.prefab`, `.ui`/`.xml`, and `.data` files from an asset directory
using **asset-root-relative paths only**, even in debug builds.
`AssetStore::load_development` additionally accepts an authoring-time
`AssetNameResolver`; the editor supplies its `AssetDatabase`. Ambiguous names
are errors, not an arbitrary first match. Explicit paths disambiguate duplicate
prefab, UI, and data asset names.

Keep the owning `AssetStore` (or editor `AssetDatabase`) alive while using its
assets. `PrefabRef`, `UiDocumentRef`, and dynamically typed `AssetReference`
are weak handles; `with` reports expired or incorrectly typed targets.
`AssetReference::typed` checks the requested asset kind. Typed `DataRef<T>`
continues to distinguish weak external links from owned inline data assets.
Weak cycles are valid; prefab inheritance/instancing cycles and UI Include
cycles are not.

The data database uniquely owns boxed `DataAssetCell`s; an inline `DataRef<T>`
uniquely owns its own boxed cell. Linked `DataRef<T>`s contain only a checked
weak handle, with no duplicated name/type strings. Cells retain their identity
and value, not factory registrations. The private handle control block permits
temporary read leases so dropping an owner inside or during a callback cannot
invalidate that callback's borrow. It does not expose shared ownership, and
new accesses expire when the unique owner is dropped. Serializing an expired
link is an error because its identity is no longer available.

Game objects accept named injected references without eagerly instantiating
the targets. Component fields can use typed handles directly:

```yaml
root:
  name: Landscape
  references:
    panel: {asset: {type: ui, path: ui/palette.ui}}
    layer: {asset: {type: data, name: TerrainLayer/Ground}}
  components:
    - type: Foliage
      data:
        prefabs:
          - {asset: {type: prefab, name: oak}}
          - {asset: {type: prefab, path: plants/pine.prefab}}
    - type: Terrain
      data:
        layer: {ref: Ground, type: TerrainLayer}
```

An `asset` wrapper accepts exactly one `path` or development-only `name`, and
one `type` (`prefab`, `ui`, or `data`). Data names use `ConcreteType/Name` in
these wrappers; typed `DataRef<T>` fields use `{ref: Name}` or
`{ref: layers/ground.data}`, with an optional concrete `type` discriminator.
When a bare data name is ambiguous across types, include that discriminator
or use a path. Prefab `parent` and eager `prefab` instances also accept names
in development or paths in both environments. Prefab/UI references can be
instantiated repeatedly without consulting the editor or their source files.

UI metadata uses the same weak references. List and Table have a typed,
optional `UiDocumentRef` template, separate from ordinary child elements:

```xml
<Canvas>
  <Reference key="tree" type="prefab" path="plants/oak.prefab"/>
  <List id="foliage" template="row"/>
  <Table id="layers" template="ui/templates/cell.ui"/>
</Canvas>
```

`Document::references(id)` exposes metadata for an element; `Reference` is not
a rendered child. Template names are development-only, while template paths
are relative to the asset root. Unlike weak references, Include paths are
relative to the containing document and eagerly copy its tree. Includes loaded
through an asset store must stay within its asset directory.

Use `AssetStore::get::<Prefab>(path)` followed by
`reference.with(|prefab| prefab.instantiate(scene, registry))`, or
`UiDocument::instantiate` for an independent UI tree. `AssetStore::load_scene`
loads prepared scenes with both asset-reference and data-reference contexts;
relative scene paths are rooted at the asset directory.
`AssetStore::with` supplies the same scoped context for custom deserialization
or dynamic template attribute changes. The pure-code API can assign an
already-resolved handle directly to `List::template` or `Table::template`.

The editor resolves authoring names before returning prefab templates or
preparing scenes. Development preparation and release packaging export
flattened, path-only prefab/UI/data files under the runtime `data_assets`
directory; `input_asset` is an asset-relative `.data` path in generated runtime
settings. Release loading never performs name lookup. Clearing the editor
prefab cache requires mutable access and rebuilds the matching data/UI store;
old weak handles expire.

`Foliage`, `Terrain`, and `TerrainLayer` are deliberately dummy asset-wiring
types. Scattering, terrain generation/rendering, automatic hot reload, and
List/Table virtualization remain unimplemented. UI layout work is deferred
until after this reference mechanism.

### UI documents and styles

`engine::ui` provides a renderer-independent UI document tree for both games
and the editor. `Label`, `Button` and `Image` are elements; `Canvas`,
`ScrollPane`, `Grid`, `List`, `Table`, `Stack` and `Overlay` are layouts.
`Stack` accepts an `orientation` attribute of `horizontal` or `vertical`
(the default). Geometry, scrolling, virtualization and rendering are not yet
implemented.

Create a tree in code with `Document::new`, `append`, `move_to`, `remove`,
typed `edit`, `set_attribute` and `set_text`, or parse XML using `Document::parse` or
`Document::load`. IDs, names and classes are indexed and can be found with
`find_id`, `find_name` and `find_class`. IDs need not be unique. Element types
are in `engine::ui::elem`; their common `ElementData<S>` has typed `id`, `name`,
`class`, `lang`, `dir` and `style: S` fields. XML `xml:lang` is an alias for
`lang`. `Label` and `Button` hold text,
`Image` has an optional `src`, and `Stack` has a typed orientation. Unknown
attributes and non-whitespace text on elements without text are rejected;
whitespace-only XML text in those elements is ignored.

`#[element(style = LabelStyle)]` injects the public, Serde-flattened `data`
field before the derives run. `#[derive(Element)]` generates the tag,
typed attribute adapters, text accessors (for `#[ui(text)]` fields), and
the `Element` implementation; add `layout` to the attribute for containers.
`#[derive(Attributes)]` generates the same adapters for common attribute
groups, respecting Serde `rename` and `flatten`. Custom attribute value types
implement `AttributeValue`. Put `#[element(...)]` **above** `#[derive(...)]`.

Every built-in element has its own style type, such as `LabelStyle` or
`ImageStyle`, deriving `UiStyle`. Fields are typed `Option` values; images
support colors, background images and offsets, but not text properties.
Custom style types can derive `UiStyle` using any subset of the supported
style fields. Edit inline styles directly, for example
`document.edit::<Label>(id, |label| label.data.style.font_size = Some(Length::Pixels(18.0)))`.
XML and `set_attribute` also accept `style="foreground: red; font-size: 18px;"`.
These declarations are deserialized into the element's style type, not stored
as strings; invalid or inapplicable properties produce errors without
replacing the previous value. Inline styles override all stylesheet rules,
including interactive-state rules. `style(id)` returns the computed typed
`Style` and reports properties unsupported by that element's style type.

```xml
<Canvas id="screen">
  <Include src="toolbar.xml" id="toolbar" class="main-toolbar"/>
  <Label id="title">Hello</Label>
</Canvas>
```

`Include` substitutes an independent copy of the referenced document's root.
Paths are relative to the containing file; attributes other than `src` override
the included root's attributes. Includes cannot contain children and cycles
are errors. Includes require `Document::load`, not `Document::parse`.

Parse one or more `Stylesheet`s and pass them, in order, to
`StyledDocument::new`. Rules and declarations are applied in source order:
later declarations win, without CSS specificity or inheritance. Styles and
declarations use typed values: `foreground` accepts named or hexadecimal
colors (including `transparent`); `background` also accepts `url(...)`;
`text-align` accepts `left`, `center` or `right`; `font-size`, `left`, `right`,
`top` and `bottom` accept finite `px` or `%` values. Font sizes cannot be
negative. Unknown properties and invalid values are rejected. Element type
selectors are case-sensitive. Structural selectors, supported attributes,
combinators, `:is`, `:where`, `:not`, `:has`, `:lang` and `:dir` are supported.
`xml:lang` and `dir="ltr|rtl"` inherit through the tree for matching. XML
namespaces other than `xml:lang`, CSS pseudo-elements,
at-rules, browser link history, shadow-tree selectors, table-column selectors,
and other selector features without a UI model are rejected.

Applications set the interactive states `hover`, `active`, `focus`, `enabled`,
`disabled` and `checked` explicitly with `set_state`. `style` returns the
current typed style.
Only properties differing from the base style are stored for active states.
`set_stylesheets` replaces the ordered sheets and recomputes only elements
matched by changed rules in the current or base state; document edits and state
changes invalidate structural matches.

### Physics and camera follow

Scenes create a thread-affine physics world on demand. `SphereCollider`,
`BoxCollider`, `CapsuleCollider` (Y-axis), and `PlaneCollider` are static
colliders; their geometry, material and local `center` are synchronized with
the owner's world transform. A plane's local normal points along +X. The
`CharacterController` owns an upright capsule and `move_by` performs
collide-and-slide movement, updates the game object's position, and returns
contact flags. Callers must supply gravity themselves.

All collider components implement `Collider`, whose primitive associated type
identifies the shape. Custom components can store `PhysicsCollider<Primitive>`,
return `PHYSICS` from `try_start` (or combine flags such as
`UPDATE | PHYSICS | LATE`),
call `sync_static` from `sync_physics`, and call `remove` from `destroy`.
`Scene::with_physics` provides access to the underlying world. Invalid or
sheared physics transforms are reported as errors. Spheres require uniform
scale, and capsules require equal X/Z scale. Changed dimensions and scale
recreate the native collider; mesh colliders, in-place resizing, scene queries,
and collision filtering remain unavailable.

Add `Camera` and `CameraFollow` to the followed object. First-person follow
snaps to the object's orientation; orbital follow can be configured with
mode-specific distance and damping:

```yaml
- type: CameraFollow
  data:
    offset: [0.0, 0.6, 0.0]
    mode:
      type: orbit
      distance: 4.0
      damping: 5.0
```

The sample game's `PlayerController` requires `CharacterController`,
`CameraFollow`, and `Camera` on the same object. Its `Gameplay` input map
provides WASD movement, mouse look, and Space to jump; `jump_force` configures
the initial upward velocity, and the controller supplies gravity.
Orbit cameras do not yet avoid obstructions.

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

## Testing

Run the engine and editor suites from the repository root:

```powershell
cargo test -p engine -p editor
```

The editor exposes a reusable test framework in `editor::testing` that points
the asset database and project loader at any folder on disk. `AssetDir::open`
uses an existing directory as the asset root, `AssetDir::fixture` loads a
checked-in folder from `editor/tests/resources`, `AssetDir::temp` creates a
self-cleaning scratch directory, and `AssetDir::temp_copy_of_fixture` clones a
fixture so tests can mutate it. `ProjectDir` provides the same entry points
for whole projects (a `settings.yaml` next to an `assets` directory), and
`scan_error_at`/`project_error_at` capture full error chains for negative
tests.

`editor/tests/resources` contains spoofed asset folders exercised by
`editor/tests/fixtures.rs`: a complete project with a pipeline, materials,
shaders, scenes, prefabs, and data assets (`basic_project`), prefab
inheritance with id-keyed child merging and removal (`prefab_inheritance`),
deeply nested asset roots (`deep_nesting`), data asset parent chains
(`data_inheritance`), and a set of intentionally broken configurations under
`invalid`, `pipelines`, and `invalid_projects` that assert the exact error
messages produced for cyclic prefabs, duplicate assets, malformed scenes,
unregistered data asset types, missing materials/shaders, and invalid project
settings.

`editor/tests/resources/references` adds component and UI reference fixtures:
Foliage prefab lists, linked and inline terrain layers, inheritance, weak
cycles, document Includes, List/Table row/cell templates, and duplicate names
resolved by path. `editor/tests/asset_references.rs` checks these, weak-handle
expiry, cache rebuilding, and engine-only loading after deleting the original
source directory. Its 23 negative fixture directories each carry an
`expected_error.txt`; the test verifies both the diagnostic and the total case
count so missing fixture directories cannot pass vacuously.
