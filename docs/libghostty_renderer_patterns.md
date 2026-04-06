# libghostty-renderer Implementation Patterns - Quick Reference

This document provides a concise reference for replicating libghostty-vt patterns in libghostty-renderer.

## Files to Create/Structure

```
src/
├── lib_renderer.zig               # Public Zig API root
├── build/
│   └── GhosttyLibRenderer.zig      # Build configuration (copy from GhosttyLibVt)
├── renderer/
│   ├── lib.zig                     # Defines calling_conv = .c
│   └── c/
│       ├── main.zig                # C API aggregator
│       ├── result.zig              # Result enum (copy from vt)
│       ├── types.zig               # Struct metadata (copy pattern)
│       ├── allocator.zig           # Allocator wrapper (copy from vt)
│       ├── [your-objects].zig      # Specific wrappers
│       └── [...]
├── lib/
│   └── allocator.zig               # (shared with vt)
└── include/
    └── ghostty/
        └── renderer/
            ├── renderer.h           # Main header
            ├── types.h              # Basic types
            └── [...]
```

## Minimum Viable C Wrapper Template

```zig
// src/renderer/c/my_object.zig

const std = @import("std");
const lib = @import("../lib.zig");
const CAllocator = lib.alloc.Allocator;
const Result = @import("result.zig").Result;
const ZigObject = @import("../my_object.zig");

const log = std.log.scoped(.my_object);

// Internal wrapper - stores allocator + Zig object + extra state
const MyObjectWrapper = struct {
    object: ZigObject,
    alloc: std.mem.Allocator,
    // Optional: effects/callbacks
    // Optional: scratch buffers
};

// C API - expose as opaque pointer
pub const MyObject = ?*MyObjectWrapper;

// Constructor
pub fn new(
    alloc_: ?*const CAllocator,
    result: *MyObject,
) callconv(lib.calling_conv) Result {
    const alloc = lib.alloc.default(alloc_);
    const ptr = alloc.create(MyObjectWrapper) catch
        return .out_of_memory;
    ptr.* = .{
        .object = ZigObject.init(...),
        .alloc = alloc,
    };
    result.* = ptr;
    return .success;
}

// Destructor
pub fn free(obj_: MyObject) callconv(lib.calling_conv) void {
    const obj = obj_ orelse return;
    obj.object.deinit(obj.alloc);
    obj.alloc.destroy(obj);
}

// Getter - validates input
pub fn get_value(obj_: MyObject) callconv(lib.calling_conv) u32 {
    const obj = obj_ orelse return 0;
    return obj.object.value;
}

// Setter - with validation
pub fn set_value(obj_: MyObject, value: u32) callconv(lib.calling_conv) Result {
    const obj = obj_ orelse return .invalid_value;
    if (value > MAX_VALUE) return .invalid_value;
    obj.object.value = value;
    return .success;
}
```

## Allocator Integration Pattern

Always follow this:

```zig
// In your _c module
pub fn new(
    alloc_: ?*const CAllocator,
    result: *YourHandle,
) callconv(lib.calling_conv) Result {
    const alloc = lib.alloc.default(alloc_);  // Convert NULL to default
    const ptr = alloc.create(YourWrapper) catch
        return .out_of_memory;                 // Handle OOM
    ptr.* = .{ .alloc = alloc };              // ALWAYS track allocator
    result.* = ptr;
    return .success;
}

pub fn free(handle_: YourHandle) callconv(lib.calling_conv) void {
    const handle = handle_ orelse return;     // NULL-safe
    const alloc = handle.alloc;               // Retrieve tracked allocator
    alloc.destroy(handle);
}
```

## Error Handling Pattern

```zig
// result.zig - Copy this exactly
pub const Result = enum(c_int) {
    success = 0,
    out_of_memory = -1,
    invalid_value = -2,
    out_of_space = -3,
    no_value = -4,
};

// Usage in functions:
pub fn operation(...) callconv(lib.calling_conv) Result {
    if (ptr == null) return .invalid_value;       // Invalid input
    if (alloc fails) return .out_of_memory;       // OOM
    if (buffer too small) return .out_of_space;   // Need bigger buffer
    if (optional missing) return .no_value;       // No value available
    return .success;                              // All good
}
```

## Opaque Pointer Declaration Pattern

In C module:
```zig
// Private wrapper struct in Zig module
const RendererWrapper = struct {
    // Zig objects
    renderer: ZigRenderer,
    // State
    alloc: std.mem.Allocator,
    // Callbacks (optional)
    effects: Effects = .{},
};

// Public C opaque pointer
pub const Renderer = ?*RendererWrapper;
```

In C header:
```c
// Forward declare opaque type
typedef struct GhosttyRendererImpl* GhosttyRenderer;

// Never expose the implementation:
// NO: struct GhosttyRendererImpl { ... };
```

## Comptime Type-Safe Getter Pattern

For flexible getter/setter APIs:

```zig
// In your C module
pub const DataKind = enum(c_int) {
    invalid = 0,
    width = 1,
    height = 2,
    dirty = 3,
    
    pub fn OutType(comptime self: DataKind) type {
        return switch (self) {
            .invalid => void,
            .width, .height => u32,
            .dirty => bool,
        };
    }
};

pub fn get(
    obj_: MyObject,
    kind: DataKind,
    out: ?*anyopaque,
) callconv(lib.calling_conv) Result {
    return switch (kind) {
        .invalid => .invalid_value,
        inline else => |comptime_kind| getTyped(
            obj_,
            comptime_kind,
            @ptrCast(@alignCast(out)),
        ),
    };
}

fn getTyped(
    obj_: MyObject,
    comptime kind: DataKind,
    out: *kind.OutType(),
) Result {
    const obj = obj_ orelse return .invalid_value;
    switch (kind) {
        .invalid => return .invalid_value,
        .width => out.* = obj.object.width,
        .height => out.* = obj.object.height,
        .dirty => out.* = obj.object.dirty,
    }
    return .success;
}
```

## Callback/Trampoline Pattern

For Zig callbacks exposed to C:

```zig
// Type definitions in wrapper
const MyCallback = struct {
    userdata: ?*anyopaque = null,
    on_event: ?OnEventFn = null,
    
    pub const OnEventFn = *const fn (
        MyObject,
        ?*anyopaque,
        event_data: u32,
    ) callconv(.c) void;
};

// Trampoline in Zig code (called by Zig internals)
fn onEventTrampoline(handler: *ZigHandler, event_data: u32) void {
    const wrapper: *MyObjectWrapper = @fieldParentPtr("handler", handler);
    const func = wrapper.callbacks.on_event orelse return;
    func(@ptrCast(wrapper), wrapper.callbacks.userdata, event_data);
}

// Register trampoline with Zig object
wrapper.object.handler.on_event_callback = onEventTrampoline;
```

## Sized Struct Pattern (for ABI stability)

In Zig:
```zig
pub const Options = extern struct {
    size: usize = @sizeOf(Options),
    enabled: bool,
    quality: u32,
    // ... more fields
};
```

In C header:
```c
typedef struct {
    size_t size;
    bool enabled;
    uint32_t quality;
} GhosttyRendererOptions;

#define GHOSTTY_INIT_SIZED(type) \
  ((type){ .size = sizeof(type) })

// Usage:
GhosttyRendererOptions opts = GHOSTTY_INIT_SIZED(GhosttyRendererOptions);
opts.enabled = true;
```

## C API Export Pattern

In lib_renderer.zig:

```zig
comptime {
    if (@import("root") == lib) {
        const c = renderer.c_api;
        
        // Objects
        @export(&c.renderer_new, .{ .name = "ghostty_renderer_new" });
        @export(&c.renderer_free, .{ .name = "ghostty_renderer_free" });
        
        // Methods
        @export(&c.renderer_render, .{ .name = "ghostty_renderer_render" });
        @export(&c.renderer_get, .{ .name = "ghostty_renderer_get" });
        
        // ... more
    }
}
```

## Main Aggregator (src/renderer/c/main.zig)

```zig
const lib = @import("../lib.zig");
const CAllocator = lib.alloc.Allocator;

const allocator = @import("allocator.zig");
const renderer = @import("renderer.zig");
const types = @import("types.zig");
// ... more modules

pub const allocator_alloc = allocator.alloc;
pub const allocator_free = allocator.free;

pub const renderer_new = renderer.new;
pub const renderer_free = renderer.free;
// ... etc

test {
    _ = allocator;
    _ = renderer;
    _ = types;
    // ... reference all modules
}
```

## Build Configuration (src/build/GhosttyLibRenderer.zig)

Copy the structure from GhosttyLibVt but adjust:

```zig
const GhosttyLibRenderer = @This();

const std = @import("std");
const GhosttyZig = @import("GhosttyZig.zig");

pub fn initStatic(b: *std.Build, zig: *const GhosttyZig) !GhosttyLibRenderer {
    return initLib(b, zig, .static);
}

pub fn initShared(b: *std.Build, zig: *const GhosttyZig) !GhosttyLibRenderer {
    return initLib(b, zig, .dynamic);
}

fn initLib(
    b: *std.Build,
    zig: *const GhosttyZig,
    linkage: std.builtin.LinkMode,
) !GhosttyLibRenderer {
    const kind: Kind = switch (linkage) {
        .static => .static,
        .dynamic => .shared,
    };
    
    const lib = b.addLibrary(.{
        .name = if (kind == .static) 
            "ghostty-renderer-static" 
        else 
            "ghostty-renderer",
        .linkage = linkage,
        .root_module = zig.renderer_c,
        .version = std.SemanticVersion{ .major = 0, .minor = 1, .patch = 0 },
    });
    
    lib.installHeadersDirectory(
        b.path("include/ghostty"),
        "ghostty",
        .{ .include_extensions = &.{".h"} },
    );
    
    // ... rest of setup
}
```

## GhosttyZig Module Integration

In GhosttyZig.zig, add:

```zig
pub const renderer: *std.Build.Module,
pub const renderer_c: *std.Build.Module,

pub fn init(...) !GhosttyZig {
    return .{
        // ... existing
        .renderer = try initRenderer("ghostty-renderer", b, cfg, deps, ...),
        .renderer_c = try initRenderer("ghostty-renderer-c", b, cfg, deps, options: {
            var dup = renderer_options;
            dup.c_abi = true;
            break :options dup;
        }, ...),
    };
}

fn initRenderer(
    name: []const u8,
    b: *std.Build,
    cfg: *const Config,
    deps: *const SharedDeps,
    renderer_options: ...,
) !*std.Build.Module {
    const renderer = b.addModule(name, .{
        .root_source_file = b.path("src/lib_renderer.zig"),
        .target = cfg.target,
        .optimize = cfg.optimize,
    });
    renderer.addOptions("build_options", general_options);
    // ... add dependencies
    return renderer;
}
```

## Header Pattern (include/ghostty/renderer.h)

```c
/**
 * @file renderer.h
 *
 * libghostty-renderer - Renderer library for Ghostty terminals
 *
 * WARNING: Unstable API
 */

#ifndef GHOSTTY_VT_RENDERER_H
#define GHOSTTY_VT_RENDERER_H

#ifdef __cplusplus
extern "C" {
#endif

#include <ghostty/vt/types.h>
#include <ghostty/vt/allocator.h>
#include <ghostty/vt/render.h>
#include <ghostty/renderer/types.h>
#include <ghostty/renderer/renderer.h>
#include <ghostty/renderer/[...].h>

#ifdef __cplusplus
}
#endif

#endif
```

## Common Pitfalls

1. **Forgot allocator tracking**: Wrapper won't know how to free itself
2. **NULL not handled**: All C API functions must handle NULL gracefully
3. **Wrong calling convention**: Always use `callconv(lib.calling_conv)` which is `.c`
4. **Missing Result codes**: Always return appropriate error code
5. **Exposing internal layout**: Never use concrete struct, always opaque pointer
6. **Not validating enums**: Use runtime validation in debug builds
7. **Mixing Zig and C calling conventions**: Trampolines must be `callconv(.c)`
8. **Forgetting @export**: Comptime block must explicitly export every function
9. **Not null-checking inputs**: All pointers can be NULL in C
10. **Wrong @ptrCast alignment**: Use `@alignCast` with `@ptrCast`

## Thread Safety

libghostty-renderer should follow the same pattern as libghostty-vt:

- **No internal locks**: Library doesn't provide synchronization
- **Caller responsible**: C code must use mutexes/locks
- **Snapshot approach**: Get mutable access, snapshot, release lock
- **Iterators are safe**: Can iterate render state without lock after snapshot

```c
// Thread-safe pattern:
pthread_mutex_lock(&mutex);
ghostty_render_state_update(state, terminal);  // Exclusive access
pthread_mutex_unlock(&mutex);

// Now iterate without lock - data is snapshotted
while (ghostty_render_state_row_iterator_next(iter)) {
    // Safe - reading snapshot
}
```

## Testing

Copy test patterns from libghostty-vt:

```zig
test "allocator tracking" {
    const alloc = std.testing.allocator;
    var obj: c.MyObject = null;
    try testing.expectEqual(c.new(&lib.alloc.test_allocator, &obj), .success);
    try testing.expect(obj != null);
    c.free(obj);
}

test "null safety" {
    var result: c.MyObject = null;
    try testing.expectEqual(c.new(null, &result), .success);
    // C allows passing NULL to getter
    _ = c.get(null);  // Should not crash
}
```

