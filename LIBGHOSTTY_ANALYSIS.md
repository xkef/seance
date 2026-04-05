# libghostty-vt Thorough Analysis - Complete Report

## Executive Summary

libghostty-vt is a production-ready C library built in Zig that implements a complete terminal emulator. It uses a sophisticated dual-API pattern: pure Zig API for Zig consumers, and a C ABI-compatible API for C/C++/FFI consumers. The C API uses opaque pointers, wrapper structs, and careful memory management to maintain ABI stability while allowing flexible allocator strategies.

## Key Files Analyzed

### 1. Public API Root
- **`src/lib_vt.zig`** (274 lines)
  - Re-exports 60+ types from terminal module
  - Single `comptime` block exports C API (uses `@export` for each function)
  - Conditional export: only when this is root module
  - Includes input encoding APIs (key, mouse, focus)

### 2. Build System
- **`src/build/GhosttyLibVt.zig`** (311 lines)
  - Three library targets: WASM executable, Static library, Shared library
  - Platform-specific handling (Darwin LLVM, Android NDK, Windows MSVC)
  - pkg-config generation
  - dSYM debug symbol extraction (Darwin)
  - Static archive combining for vendored SIMD
  - Header installation pattern: `installHeadersDirectory`

### 3. C API Implementation Structure (src/terminal/c/)
Complete C wrapper layer with ~20 modules:

#### Core Infrastructure:
- **`result.zig`** - Error enum (success=0, out_of_memory=-1, invalid_value=-2, etc.)
- **`types.zig`** - Comptime struct metadata generation for WASM/FFI
- **`allocator.zig`** - Memory allocation interface (copy, wrap, convert between Zig/C)
- **`main.zig`** - C API aggregator (imports all c modules, re-exports)

#### Object Wrappers:
- **`terminal.zig`** - Terminal state + VT stream + callback effects
- **`render.zig`** - Render state snapshots + row/cell iterators
- **`key_event.zig`** - Key event wrapper (allocator tracking + event data)
- **`mouse_event.zig`** - Mouse event wrapper
- **`formatter.zig`** - Text/HTML/VT formatters

#### Utilities:
- **`grid_ref.zig`** - Grid references for cell traversal
- **`selection.zig`** - Text selection ranges
- **`sgr.zig`** - Select Graphic Rendition (text styling) parser
- **`osc.zig`** - Operating System Command parser
- **Plus**: focus, paste, key_encode, mouse_encode, style, cell, row, color, modes, build_info

### 4. C Headers (include/ghostty/vt/)
All headers use Doxygen `@defgroup` organization:
- **`vt.h`** - Main umbrella header
- **`types.h`** - GHOSTTY_API macro, Result enum, GHOSTTY_INIT_SIZED macro
- **`allocator.h`** - GhosttyAllocator interface
- **`terminal.h`** - Terminal object
- **`render.h`** (20KB) - Render state, iterators, dirty tracking
- And 20+ more specialized headers

### 5. Allocator System (src/lib/allocator.zig)
- Dual interface: C allocator ↔ Zig allocator
- Default fallback chain: custom > libc > wasm_allocator > smp_allocator
- Every wrapper tracks its allocator for cleanup

## Architecture Patterns Summary

### 1. Dual Module Build
```
Root (lib_vt.zig)
├── vt module (pure Zig)
└── vt_c module (C ABI enabled via build option)
```
Both use same root but with `c_abi = true` flag for C module.

### 2. Opaque Pointer Pattern
```zig
const TerminalWrapper = struct {
    terminal: *ZigTerminal,
    stream: Stream,
    effects: Effects,
};
pub const Terminal = ?*TerminalWrapper;  // Opaque to C
```
C sees: `typedef struct GhosttyTerminalImpl* GhosttyTerminal;`

### 3. Wrapper Lifecycle Pattern
```zig
pub fn new(alloc_: ?*const CAllocator, result: *Terminal) callconv(.c) Result {
    const alloc = lib.alloc.default(alloc_);      // NULL -> default
    const ptr = alloc.create(TerminalWrapper)...  // Allocate
    ptr.* = .{ .terminal = ..., .stream = ..., .alloc = alloc };  // Initialize
    result.* = ptr;                                // Return via out param
    return .success;
}

pub fn free(term_: Terminal) callconv(.c) void {
    const term = term_ orelse return;              // NULL-safe
    const alloc = term.alloc;                      // Get tracked allocator
    term.terminal.deinit(alloc);                   // Deinit Zig object
    alloc.destroy(term);                           // Free wrapper
}
```

### 4. Error Handling
All functions return `Result` enum with codes:
- `success (0)` - Normal completion
- `out_of_memory (-1)` - Allocation failed
- `invalid_value (-2)` - Bad input
- `out_of_space (-3)` - Buffer too small
- `no_value (-4)` - Optional missing

### 5. Calling Convention
```zig
const calling_conv: std.builtin.CallingConvention = .c;
pub fn function(...) callconv(lib.calling_conv) Result { ... }
```
All exported functions use C calling convention for FFI compatibility.

### 6. Render State Pattern (Critical for Renderer)
Three iterator types for efficient data access:
1. **RenderState** - Top-level state, dirty tracking
2. **RowIterator** - Iterates visible rows
3. **RowCells** - Iterates cells in a row

Data querying uses **comptime dispatch**:
```zig
pub const Data = enum(c_int) { ... pub fn OutType(comptime self: Data) type { ... } }
pub fn get(...) Result {
    return switch (data) {
        inline else => |comptime_data| getTyped(..., comptime_data, ...)
    };
}
```

### 7. Callback Integration (Trampolines)
Terminal supports C callbacks via effects struct:
```zig
const Effects = struct {
    write_pty: ?WritePtyFn = null,
    bell: ?BellFn = null,
    // ... more callbacks
};

fn writePtyTrampoline(handler: *Handler, data: [:0]const u8) void {
    const wrapper: *TerminalWrapper = @fieldParentPtr("stream", ...);
    const func = wrapper.effects.write_pty orelse return;
    func(@ptrCast(wrapper), wrapper.effects.userdata, data.ptr, data.len);
}
```

### 8. Sized Struct Pattern (ABI Forward Compatibility)
```zig
pub const Options = extern struct {
    size: usize = @sizeOf(Options),
    field1: bool,
    field2: u32,
    // Can add new fields at end without breaking ABI
};
```

C usage:
```c
GhosttyFormatterOptions opts = GHOSTTY_INIT_SIZED(GhosttyFormatterOptions);
opts.field1 = true;
```

### 9. Thread Safety Model
- **No internal locks**: Library is not synchronized
- **Caller provides locks**: Must serialize terminal modifications
- **Render state snapshots**: Can iterate after update without lock
- **Pattern**: Lock during update(), then iterate/read freely

### 10. Memory Management Flexibility
- All functions accept `?*const GhosttyAllocator`
- NULL means use default allocator
- Works with: libc malloc/free, custom allocators, WASM, embedded systems
- Each wrapper stores its allocator for deallocation matching

## Platform-Specific Implementation

### Darwin (macOS)
- LLVM required (`lib.use_llvm = true`)
- Apple SDK auto-detection
- dSYM symbol extraction
- Headerpad for dynamic linking

### Android
- 16KB page size support (Android 15+)
- NDK path integration
- Link constraints for system libraries

### Windows
- MSVC compatibility (ubsan-rt disabled)
- MSVC linker compatibility adjustments
- Symbol visibility via `__declspec(dllexport/import)`

### WebAssembly
- Entry disabled (no main function)
- Export table enabled (for callbacks)
- Custom table patching for growable tables
- Wasm-specific allocator fallback

## Build Integration

### Module Configuration
Both `vt` and `vt_c` modules:
- Root: `src/lib_vt.zig`
- Target: user-specified platform
- Optimize: user-specified level
- Conditional linking: libc, libcpp (SIMD only)

### Library Types
1. **Static** (`initStatic`)
   - Bundles compiler-rt, ubsan-rt
   - PIC enabled (for PIE executables)
   - Can combine with SIMD archives

2. **Shared** (`initShared`)
   - Dynamic linking
   - dSYM symbols (Darwin)
   - pkg-config file generation

3. **WASM** (`initWasm`)
   - Executable (not library)
   - Table patching post-processing
   - Indirect function table exported

### Header Installation
```zig
lib.installHeadersDirectory(
    b.path("include/ghostty"),
    "ghostty",
    .{ .include_extensions = &.{".h"} },
);
```
Installs all .h files from `include/ghostty/` to `<prefix>/include/ghostty/`

### pkg-config Generation
Automatically generates `libghostty-vt.pc` with:
- Cflags, Libs, Libs.private
- Version metadata
- Dependency information

## Export Mechanism

The C API export happens via comptime conditional:

```zig
comptime {
    if (@import("root") == lib) {  // Only when this is the root module
        const c = terminal.c_api;   // Import all C API functions
        @export(&c.key_event_new, .{ .name = "ghostty_key_event_new" });
        @export(&c.key_event_free, .{ .name = "ghostty_key_event_free" });
        // ... 200+ more @export calls
    }
}
```

This achieves:
1. **Conditional export**: Only exports when building the C library
2. **Explicit naming**: Each function explicitly bound to C symbol name
3. **Type safety**: Zig still has full type info despite C ABI

## Key Insights for libghostty-renderer

1. **Follow exact patterns**: Allocator tracking, opaque pointers, error codes
2. **Allocator is critical**: Every wrapper must track its allocator
3. **NULL safety everywhere**: All pointers can be NULL from C
4. **Sized structs for stability**: Allows adding fields without ABI break
5. **Render state model**: Iterator pattern is proven for efficient rendering
6. **No locks in library**: Thread safety is caller's responsibility
7. **Calling convention**: Use `callconv(.c)` consistently
8. **Runtime validation**: Validate enums in debug builds
9. **comptime dispatch**: Type-safe getter/setter patterns
10. **Comptime exports**: Explicit @export() for each function

## Testing Approach

libghostty-vt tests:
- NULL pointer handling
- Allocator tracking and cleanup
- Result code correctness
- Enum validation
- Memory leak detection (tests use testing.allocator)

## Documentation

Each C header uses Doxygen:
- `@file` and `@mainpage` for overview
- `@defgroup` for API groups
- `@ingroup` for function/type membership
- `@code` blocks with examples
- Parameter documentation with `@param`
- Return value documentation with `@return`

---

## Conclusion

libghostty-vt demonstrates production-quality Zig-to-C FFI design. The patterns are:
- **Mature**: Used in real production (Ghostty terminal)
- **Robust**: Handles NULL, allocation failures, platform differences
- **Flexible**: Custom allocators, WASM, embedded systems
- **Stable**: ABI-compatible via opaque pointers and sized structs
- **Safe**: Thread-safe when used correctly, no surprises

Replicating these patterns in libghostty-renderer will ensure quality, consistency, and maintainability.

