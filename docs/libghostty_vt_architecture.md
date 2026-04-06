# libghostty-vt Library Architecture & Patterns

## Overview
libghostty-vt is a C-compatible library extracted from Ghostty that implements a modern terminal emulator. It's built in Zig with a dual-module approach: a pure Zig module (`vt`) and a C ABI-compatible module (`vt_c`).

---

## 1. PUBLIC ZIG API (src/lib_vt.zig)

### Structure
- Root file that re-exports terminal modules as public API
- Imports from `terminal/main.zig` (extracted from main Ghostty)
- Separates public API from internal-only types
- Includes both Zig and C API surfaces

### Key API Categories Exported:
- **Terminal Parsing**: Parser, StreamAction
- **Grid/Display**: Page, Cell, Screen, ScreenSet, Cursor, Selection
- **Styling**: Style, Color, SGR attributes
- **Input Encoding**: Key events, Mouse events, Focus events
- **Utilities**: Formatter, Search, Size reports

### C API Export Pattern:
```zig
comptime {
    if (@import("root") == lib) {  // Only export if this is root module
        const c = terminal.c_api;
        @export(&c.function_name, .{ .name = "ghostty_function_name" });
        // ... many more exports
    }
}
```

---

## 2. BUILD CONFIGURATION (src/build/GhosttyLibVt.zig)

### Two Build Modes:

#### a) Pure Zig Module (`vt`)
- Root: `src/lib_vt.zig`
- No C ABI requirement
- For Zig consumers

#### b) C ABI Module (`vt_c`)
- Root: `src/lib_vt.zig` (same)
- Built with `c_abi = true` flag
- Enables C calling conventions
- For C/C++/FFI consumers

### Library Types:
1. **WASM Executable** (initWasm)
   - Entry disabled
   - Dynamic function table exported
   - Growable table patching applied
   
2. **Static Library** (initStatic)
   - Bundles compiler-rt, ubsan-rt
   - PIC enabled
   - Can combine with SIMD archives into fat archive
   
3. **Shared Library** (initShared)
   - Dynamic linking
   - dSYM debug symbols (Darwin)
   - pkg-config generation

### Header Installation:
```zig
lib.installHeadersDirectory(
    b.path("include/ghostty"),
    "ghostty",
    .{ .include_extensions = &.{".h"} },
);
```

### Platform-Specific Handling:
- **Darwin**: LLVM required, headerpad_max_install_names, Apple SDK
- **Android**: 16kb page size support (Android 15+), NDK paths
- **Windows**: ubsan-rt disabled (incompatible with MSVC linker)

---

## 3. C API IMPLEMENTATION PATTERNS (src/terminal/c/)

### File Organization:
```
src/terminal/c/
├── main.zig          # C API aggregator (exports all functions)
├── result.zig        # Error codes enum
├── types.zig         # Struct metadata/JSON generation
├── allocator.zig     # Memory management interface
├── terminal.zig      # Terminal object/callbacks
├── render.zig        # Render state for incremental updates
├── key_event.zig     # Key input encoding
├── mouse_event.zig   # Mouse input encoding
├── formatter.zig     # Content formatting (plain/VT/HTML)
├── grid_ref.zig      # Grid traversal references
├── [...]
```

### Calling Convention:
```zig
const lib = @import("../lib.zig");
// lib.calling_conv = .c  (defined in src/terminal/lib.zig)

pub fn function_name(...) callconv(lib.calling_conv) Result {
    // Implementation
}
```

### Wrapper Pattern (Opaque Handles)

All C objects use **opaque pointers** with internal wrapper structs:

#### Example: Terminal
```zig
// Internal wrapper (private to C module)
const TerminalWrapper = struct {
    terminal: *ZigTerminal,        // Real Zig object
    stream: Stream,                // VT stream state
    effects: Effects = .{},        // Callback state
};

// C API expose as opaque pointer
pub const Terminal = ?*TerminalWrapper;

// C header declares:
// typedef struct GhosttyTerminalImpl* GhosttyTerminal;
```

#### Key Advantages:
1. **ABI Stability**: C code never knows internal layout
2. **Memory Management**: Allocator tracked in wrapper
3. **Callback Integration**: Can store effect callbacks safely
4. **Type Safety**: Zig still has full type information

### Allocator Management Pattern:

```zig
pub fn new(
    alloc_: ?*const CAllocator,
    result: *Event,
) callconv(lib.calling_conv) Result {
    const alloc = lib.alloc.default(alloc_);  // NULL -> default
    const ptr = alloc.create(KeyEventWrapper) catch
        return .out_of_memory;
    ptr.* = .{ .alloc = alloc };  // Track allocator in wrapper
    result.* = ptr;
    return .success;
}

pub fn free(event_: Event) callconv(lib.calling_conv) void {
    const wrapper = event_ orelse return;  // NULL-safe
    const alloc = wrapper.alloc;           // Retrieve allocator
    alloc.destroy(wrapper);
}
```

### Allocator Interface (src/lib/allocator.zig):

Dual allocator system:
```zig
pub const Allocator = extern struct {
    ctx: *anyopaque,
    vtable: *const VTable,
    
    pub fn fromZig(zig_alloc: *const std.mem.Allocator) Allocator { ... }
    pub fn zig(self: *const Allocator) std.mem.Allocator { ... }
};

pub fn default(c_alloc_: ?*const Allocator) std.mem.Allocator {
    if (c_alloc_) |c_alloc| return c_alloc.zig();
    if (comptime builtin.is_test) return testing.allocator;
    if (comptime builtin.link_libc) return std.heap.c_allocator;
    if (comptime builtin.target.cpu.arch.isWasm()) return std.heap.wasm_allocator;
    return std.heap.smp_allocator;  // Default: Zig SMP
}
```

Default Fallback Chain:
1. Custom allocator if provided
2. libc malloc/free if linked
3. Wasm allocator
4. Zig SMP allocator

### Error Handling Pattern:

```zig
pub const Result = enum(c_int) {
    success = 0,
    out_of_memory = -1,
    invalid_value = -2,
    out_of_space = -3,
    no_value = -4,
};

// Usage:
pub fn operation(...) callconv(lib.calling_conv) Result {
    if (check_fails) return .invalid_value;
    if (allocation_fails) return .out_of_memory;
    if (bounds_check_fails) return .out_of_space;
    // ...
    return .success;
}
```

### Runtime Safety Validation:

```zig
pub fn set_action(event_: Event, action: key.Action) callconv(lib.calling_conv) void {
    if (comptime std.debug.runtime_safety) {
        _ = std.meta.intToEnum(key.Action, @intFromEnum(action)) catch {
            log.warn("set_action invalid action value={d}", .{@intFromEnum(action)});
            return;
        };
    }
    const event: *key.KeyEvent = &event_.?.event;
    event.action = action;
}
```

---

## 4. RENDER STATE API DETAILS (src/terminal/c/render.zig)

Critical for understanding renderer integration:

### Three Opaque Types:
```zig
const RenderStateWrapper = struct {
    alloc: std.mem.Allocator,
    state: renderpkg.RenderState = .empty,
};

const RowIteratorWrapper = struct {
    alloc: std.mem.Allocator,
    y: ?size.CellCountInt,
    raws: []const page.Row,
    cells: []const std.MultiArrayList(renderpkg.RenderState.Cell),
    dirty: []bool,
    palette: *const colorpkg.Palette,
};

const RowCellsWrapper = struct {
    alloc: std.mem.Allocator,
    x: ?size.CellCountInt,
    raws: []const page.Cell,
    graphemes: []const []const u21,
    styles: []const Style,
    palette: *const colorpkg.Palette,
};

pub const RenderState = ?*RenderStateWrapper;
pub const RowIterator = ?*RowIteratorWrapper;
pub const RowCells = ?*RowCellsWrapper;
```

### Dirty Tracking Levels:
```zig
pub const Dirty = enum(c_int) {
    false = 0,      // No changes
    partial = 1,    // Some rows changed
    full = 2,       // Global state changed
};
```

### Generic Getter Pattern (Type-Safe with comptime):

```zig
pub const Data = enum(c_int) {
    cols = 1,
    rows = 2,
    dirty = 3,
    row_iterator = 4,
    // ... more kinds
    
    pub fn OutType(comptime self: Data) type {
        return switch (self) {
            .cols, .rows => size.CellCountInt,
            .dirty => Dirty,
            .row_iterator => RowIterator,
            // ... match each kind to its type
        };
    }
};

pub fn get(
    state_: RenderState,
    data: Data,
    out: ?*anyopaque,
) callconv(lib.calling_conv) Result {
    return switch (data) {
        .invalid => .invalid_value,
        inline else => |comptime_data| getTyped(
            state_,
            comptime_data,
            @ptrCast(@alignCast(out)),
        ),
    };
}

fn getTyped(
    state_: RenderState,
    comptime data: Data,
    out: *data.OutType(),
) Result {
    const state = state_ orelse return .invalid_value;
    switch (data) {
        .cols => out.* = state.state.cols,
        .rows => out.* = state.state.rows,
        // ... type-safe access
    };
}
```

### Iterator Pattern (Used by Renderers):

```c
// C usage pattern:
GhosttyRenderStateRowIterator iter;
ghostty_render_state_row_iterator_new(NULL, &iter);
while (ghostty_render_state_row_iterator_next(iter)) {
    bool dirty;
    ghostty_render_state_row_get(iter, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &dirty);
    
    GhosttyRenderStateRowCells cells;
    ghostty_render_state_row_cells_new(NULL, &cells);
    while (ghostty_render_state_row_cells_next(cells)) {
        // Process each cell
    }
    ghostty_render_state_row_cells_free(cells);
}
ghostty_render_state_row_iterator_free(iter);
```

---

## 5. TERMINAL OBJECT WITH CALLBACKS (src/terminal/c/terminal.zig)

### Effects/Callback Structure:

```zig
const Effects = struct {
    userdata: ?*anyopaque = null,
    write_pty: ?WritePtyFn = null,
    bell: ?BellFn = null,
    color_scheme: ?ColorSchemeFn = null,
    device_attributes_cb: ?DeviceAttributesFn = null,
    enquiry: ?EnquiryFn = null,
    xtversion: ?XtversionFn = null,
    title_changed: ?TitleChangedFn = null,
    size_cb: ?SizeFn = null,
    da_features_buf: [64]device_attributes.Primary.Feature = undefined,
    
    pub const WritePtyFn = *const fn (Terminal, ?*anyopaque, [*]const u8, usize) callconv(.c) void;
    pub const BellFn = *const fn (Terminal, ?*anyopaque) callconv(.c) void;
    pub const ColorSchemeFn = *const fn (Terminal, ?*anyopaque, *device_status.ColorScheme) callconv(.c) bool;
    // ... more function types
};
```

### Trampoline Pattern (Zig -> C Callbacks):

```zig
fn writePtyTrampoline(handler: *Handler, data: [:0]const u8) void {
    const stream_ptr: *Stream = @fieldParentPtr("handler", handler);
    const wrapper: *TerminalWrapper = @fieldParentPtr("stream", stream_ptr);
    const func = wrapper.effects.write_pty orelse return;
    func(@ptrCast(wrapper), wrapper.effects.userdata, data.ptr, data.len);
}
```

---

## 6. THREAD SAFETY

### Design Principle:
**The render state API is designed to be thread-safe when locks are used properly:**

From `include/ghostty/vt/render.h`:
> "The key design principle of this API is that it only needs read/write access to the terminal instance during the update call. This allows the render state to minimally impact terminal IO performance and also allows the renderer to be safely multi-threaded (as long as a lock is held during the update call to ensure exclusive access to the terminal instance)."

### Pattern:
```c
// Thread A (Input):
ghostty_terminal_vt_write(terminal, data, len);

// Thread B (Render) - requires lock:
pthread_mutex_lock(&lock);
ghostty_render_state_update(render_state, terminal);  // Reads terminal
pthread_mutex_unlock(&lock);

// Can iterate render state without lock
ghostty_render_state_row_iterator_next(iter);
```

### No Mutex in Library:
- Library provides no synchronization primitives
- Caller responsible for serializing terminal modifications
- Render state snapshots data, allowing lock-free reads after update

---

## 7. SIZED STRUCTS ABI PATTERN

For forward compatibility with adding new fields:

```zig
pub const Options = extern struct {
    size: usize = @sizeOf(Options),  // Caller sets to sizeof()
    emit: Format,
    unwrap: bool,
    trim: bool,
    // ... more fields
};

// C usage:
GhosttyFormatterTerminalOptions opts = GHOSTTY_INIT_SIZED(GhosttyFormatterTerminalOptions);
opts.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
opts.trim = true;
// Pass opts...
```

Macro:
```c
#define GHOSTTY_INIT_SIZED(type) \
  ((type){ .size = sizeof(type) })
```

---

## 8. MEMORY AND LIFETIME SEMANTICS

### Allocator Flexibility:
- All functions accept `?*const GhosttyAllocator` (or NULL)
- NULL defaults to system allocator
- Each wrapper stores its allocator for deallocation
- Client can use custom allocator (JNI, WASM, embedded systems)

### Buffer Management Examples:

**Formatter Output (Allocated):**
```zig
pub fn format_alloc(
    formatter_: Formatter,
    terminal: terminal_c.Terminal,
    alloc_: ?*const CAllocator,
    out: ?*lib.String,
) callconv(lib.calling_conv) Result {
    // Allocates via alloc_, returns via out
}
// Caller must free with ghostty_free(alloc_, ptr, len)
```

**Grid Graphemes (Caller-Provided Buffer):**
```zig
pub fn grid_ref_graphemes(
    ref: *const CGridRef,
    out_buf: ?[*]u32,        // Caller provides
    buf_len: usize,
    out_len: *usize,         // Output length
) callconv(lib.calling_conv) Result {
    if (out_buf == null or buf_len < total) {
        out_len.* = total;
        return .out_of_space;  // Caller knows needed size
    }
    // Fill out_buf
}
```

---

## 9. EXAMPLE C WRAPPER IMPLEMENTATION

```zig
const KeyEventWrapper = struct {
    event: key.KeyEvent = .{},
    alloc: Allocator,  // Track allocator
};

pub const Event = ?*KeyEventWrapper;

pub fn new(
    alloc_: ?*const CAllocator,
    result: *Event,
) callconv(lib.calling_conv) Result {
    const alloc = lib.alloc.default(alloc_);
    const ptr = alloc.create(KeyEventWrapper) catch
        return .out_of_memory;
    ptr.* = .{ .alloc = alloc };
    result.* = ptr;
    return .success;
}

pub fn free(event_: Event) callconv(lib.calling_conv) void {
    const wrapper = event_ orelse return;
    const alloc = wrapper.alloc;
    alloc.destroy(wrapper);
}

pub fn set_action(event_: Event, action: key.Action) callconv(lib.calling_conv) void {
    if (comptime std.debug.runtime_safety) {
        _ = std.meta.intToEnum(key.Action, @intFromEnum(action)) catch {
            log.warn("invalid action={d}", .{@intFromEnum(action)});
            return;
        };
    }
    const event: *key.KeyEvent = &event_.?.event;
    event.action = action;
}

pub fn get_action(event_: Event) callconv(lib.calling_conv) key.Action {
    return event_.?.event.action;
}
```

---

## 10. KEY ARCHITECTURAL PRINCIPLES FOR REPLICATION

When building libghostty-renderer, follow these patterns:

1. **Dual Modules**: Pure Zig + C ABI variant
2. **Opaque Handles**: All C objects are opaque pointers to internal wrappers
3. **Wrapper Pattern**: Store allocator + Zig object + extra state
4. **Calling Convention**: All exported C functions use `callconv(.c)`
5. **Allocator Tracking**: Each wrapper tracks its allocator for cleanup
6. **NULL Safety**: All API functions handle NULL gracefully
7. **Result Codes**: Use Result enum for error handling
8. **Runtime Validation**: Validate enum values in debug builds
9. **Sized Structs**: Use size field for ABI forward compatibility
10. **Comptime Dispatch**: Use comptime `inline else` for type-safe getter patterns
11. **Trampolines**: Convert Zig callbacks to C callback pointers
12. **Export Discipline**: Explicit @export() calls in comptime block

---

## 11. HEADER STRUCTURE (include/ghostty/vt/)

```
vt/
├── vt.h                    # Main include (includes all below)
├── types.h                 # Basic types, GHOSTTY_API macro
├── allocator.h             # GhosttyAllocator interface
├── terminal.h              # Terminal object
├── render.h                # Render state (CRITICAL FOR RENDERER)
├── grid_ref.h              # Grid traversal
├── formatter.h             # Content formatting
├── key.h / mouse.h         # Input encoding
├── style.h / color.h       # Styling types
├── sgr.h / osc.h          # Sequence parsing
└── [...]
```

Each header has:
- `@defgroup` Doxygen groups
- Opaque typedef declarations: `typedef struct GhosttyXyzImpl* GhosttyXyz;`
- Sized struct pattern with GHOSTTY_INIT_SIZED macro
- Result return codes
- Detailed documentation with examples

---

## 12. BUILD INTEGRATION

### pkg-config Generation:
```zig
const pc: std.Build.LazyPath = pc: {
    const wf = b.addWriteFiles();
    break :pc wf.add("libghostty-vt.pc", b.fmt(
        \\prefix={s}
        \\includedir=${{prefix}}/include
        \\libdir=${{prefix}}/lib
        \\Cflags: -I${{includedir}}
        \\Libs: -L${{libdir}} -lghostty-vt
        \\Libs.private: {s}
        // ... etc
    , .{ b.install_prefix, libsPrivate(zig), requiresPrivate(b) }));
};
```

### Header Installation:
```zig
lib.installHeadersDirectory(
    b.path("include/ghostty"),
    "ghostty",
    .{ .include_extensions = &.{".h"} },
);
```

### Static/Shared/Wasm Variants:
- All built from same root source
- WASM gets special treatment (executable, table patching)
- Static libs bundle compiler runtime
- Shared libs get dSYM symbols on Darwin

