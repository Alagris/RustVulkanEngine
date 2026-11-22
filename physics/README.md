# PhysX integration

The workspace vendors the source from
[EmbarkStudios/physx-rs](https://github.com/EmbarkStudios/physx-rs), upstream
commit `d0d571e` (2024-02-12). Its PhysX SDK source is also vendored at the
upstream-pinned commit `94ca32881287dc2a2063551e8ed3c5af5d6a1742`, which
reports PhysX version 5.1.3. Neither repository is a Git submodule.

The upstream Rust workspace has been folded into the root Cargo workspace:

- `physx-sys` contains the low-level generated bindings and native C++ build.
- `physx` contains Embark's incomplete high-level wrapper.
- `pxbind` is the upstream binding-generation tool.
- `physx-safe` is the engine-facing API being developed here.

Keep the upstream `LICENSE-APACHE`, `LICENSE-MIT`, and `physx-sys/LICENSE-BSD`
files with their corresponding source. Building `physx-sys` requires a working
C++ toolchain and compiles the vendored PhysX source.

## Current safe wrapper

`physx-safe` validates primitive collider dimensions, materials, transforms,
dynamic masses, controller movement, and time steps. `PhysicsWorld` owns its
scene, controllers, bodies and materials, exposes stable handles and copied
collider descriptors instead of native pointers, and is neither `Send` nor
`Sync`. Multiple worlds on the same thread share one PhysX foundation; creating
a world on another thread while that foundation is live returns an error.

Static and dynamic bodies support sphere, box and capsule shapes; planes are
static only. A body can be removed with `remove_body`, and `collider` returns a
copyable reference suitable for storing in a component. Handles are invalid
after removal or world destruction and cannot be used with a different world.
Capsule controllers have separate handles, position, collide-and-slide movement,
contact flags and removal APIs. Callers must supply gravity in the requested
displacement; controller movement does not apply gravity automatically.

`Pose` contains translation and rotation only. `ColliderGeometry::scaled`
converts local scale to primitive dimensions: box axes may scale independently,
spheres require uniform scale, and capsules require equal radial scales.
Zero, negative, nonfinite and unrepresentable scales are rejected. Scale
cannot currently be changed on an existing collider; callers must recreate it.
Mesh colliders, general scene queries and collision filtering are not yet exposed.

The wrapper currently uses the CPU simulation path. Although the vendored SDK
contains PhysX 5 GPU-related headers and data types, this `physx-rs` revision
does not bind CUDA context creation or link the PhysX GPU library. The high-level
`SceneDescriptor` also does not expose a CUDA context manager. GPU simulation
must not be treated as available until those pieces and their CUDA/driver
requirements are deliberately integrated.

## Safety work and remaining boundaries

The vendored wrapper now frees boxed user data when native objects are dropped,
avoids manufacturing mutable references to possibly aliased material slots,
handles null pointers for zero-length callback arrays, and owns scratch-buffer
allocations with RAII. Asynchronous `simulate`/`step` calls are unsafe because
PhysX may continue using caller-provided task or scratch-buffer pointers after
the call returns.

The upstream `physx` API is still a broad work-in-progress wrapper, not a fully
audited safe abstraction. In particular, its standalone owner types do not
encode the lifetime dependency on `PhysicsFoundation`, and callback
implementations must not unwind through the C ABI. Engine code should use
`physx-safe` rather than storing native owners or raw pointers directly.

Run `cargo test -p physx-safe` to exercise allocator tracking by default in the
safe-wrapper tests. The `memory-tracking` feature keeps a set of live pointers
for allocations routed through the Rust PhysX allocator callbacks, checks for
unknown/double frees, and exposes `assert_live` for explicit pointer checks.
Test builds also register `Owner` pointers and assert on duplicate ownership or
access after drop. Invariant assertions cover user-data initialization,
callback arrays, pointer alignment, and safe-wrapper postconditions. The tests
verify that a create/step/drop lifecycle balances. Ordinary production builds
use PhysX's default allocator and compile out this tracking. It cannot
intercept arbitrary native pointer dereferences, Rust allocations outside those
callbacks, or GPU/driver memory; use sanitizers to detect memory corruption
outside the instrumented paths.
