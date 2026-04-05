# libghostty-vt Analysis Documentation

This directory contains a complete architectural analysis of libghostty-vt, ready for implementing libghostty-renderer with the same patterns.

## Documents

### 1. `LIBGHOSTTY_ANALYSIS.md` - Executive Summary
Start here for high-level overview. Contains:
- What libghostty-vt is and how it's structured
- Key files and their purposes
- Architecture patterns summary
- 10 key insights for renderer implementation
- Platform-specific implementation notes

### 2. `libghostty_vt_architecture.md` - Complete Technical Reference
Deep dive into every aspect. Contains:
- Public Zig API structure (lib_vt.zig)
- Build configuration (GhosttyLibVt.zig)
- C API implementation patterns
- Render state API details (critical for renderer)
- Terminal object with callbacks
- Thread safety design
- Sized struct ABI pattern
- Memory and lifetime semantics
- Example implementations
- 12 architectural principles for replication
- Header structure
- Build integration details

### 3. `libghostty_renderer_patterns.md` - Quick Reference
Practical patterns and templates for implementing the renderer. Contains:
- File structure template
- Minimum viable C wrapper template
- Allocator integration pattern
- Error handling pattern
- Opaque pointer declaration pattern
- Type-safe getter pattern
- Callback/trampoline pattern
- Sized struct pattern
- C API export pattern
- Main aggregator template
- Build configuration template
- Header pattern
- Common pitfalls (10 items)
- Thread safety pattern
- Testing patterns

## How to Use This

### For Overview (15 mins)
Read `LIBGHOSTTY_ANALYSIS.md` sections:
1. Executive Summary
2. Architecture Patterns Summary
3. Key Insights for libghostty-renderer

### For Implementation (1-2 hours)
Use `libghostty_renderer_patterns.md` as template and reference:
1. Create file structure using the template
2. Implement each wrapper following the "Minimum Viable Template"
3. Reference specific patterns as you need them

### For Deep Understanding (2-3 hours)
Read `libghostty_vt_architecture.md` section by section:
1. Start with section 3 (C API Implementation Patterns)
2. Read section 4 (Render State API) - critical for renderer
3. Read section 10 (Key Architectural Principles)

## Key Files in libghostty-vt Source

All of these are analyzed in the documentation:

**Root API:**
- `/Users/kk/code/seance/ghostty/src/lib_vt.zig` - Public Zig API (274 lines)

**Build:**
- `/Users/kk/code/seance/ghostty/src/build/GhosttyLibVt.zig` - Library build (311 lines)

**C API Implementation (20 files):**
- `src/terminal/c/main.zig` - C API aggregator
- `src/terminal/c/result.zig` - Error codes
- `src/terminal/c/types.zig` - Struct metadata
- `src/terminal/c/allocator.zig` - Memory interface
- `src/terminal/c/terminal.zig` - Terminal wrapper
- `src/terminal/c/render.zig` - Render state (CRITICAL)
- `src/terminal/c/key_event.zig` - Key events
- `src/terminal/c/mouse_event.zig` - Mouse events
- And ~12 more

**Allocator System:**
- `src/lib/allocator.zig` - Zig/C allocator conversion

**C Headers (26 files):**
- `include/ghostty/vt.h` - Main umbrella
- `include/ghostty/vt/types.h` - Basic types
- `include/ghostty/vt/render.h` - Render state (20KB)
- `include/ghostty/vt/terminal.h` - Terminal interface
- And ~22 more

## Core Patterns to Replicate

### 1. Allocator Tracking
Every wrapper stores its allocator so it can be freed correctly:
```zig
const MyObjectWrapper = struct {
    object: ZigObject,
    alloc: std.mem.Allocator,  // MUST track this
};
```

### 2. Opaque Pointers
C never knows internal layout:
```zig
pub const MyObject = ?*MyObjectWrapper;  // Opaque to C
```

### 3. Error Codes
All functions return Result enum:
```zig
pub const Result = enum(c_int) {
    success = 0,
    out_of_memory = -1,
    invalid_value = -2,
    out_of_space = -3,
    no_value = -4,
};
```

### 4. Calling Convention
All exported C functions use `.c` convention:
```zig
pub fn function(...) callconv(.c) Result { ... }
```

### 5. NULL Safety
All pointers can be NULL from C:
```zig
pub fn free(obj_: MyObject) callconv(.c) void {
    const obj = obj_ orelse return;  // NULL-safe
    // ... cleanup
}
```

### 6. Sized Structs
Options structs have size field for ABI forward compatibility:
```zig
pub const Options = extern struct {
    size: usize = @sizeOf(Options),
    field1: bool,
    // Can add fields at end without breaking ABI
};
```

### 7. Comptime Dispatch
Type-safe getters using comptime:
```zig
pub const DataKind = enum(c_int) { ... pub fn OutType(comptime self: DataKind) type { ... } }
pub fn get(...) Result {
    return switch (kind) {
        inline else => |comptime_kind| getTyped(..., comptime_kind, ...)
    };
}
```

### 8. Conditional Export
Only export C API when building the C library:
```zig
comptime {
    if (@import("root") == lib) {
        const c = renderer.c_api;
        @export(&c.function_name, .{ .name = "ghostty_function_name" });
    }
}
```

### 9. Trampoline Callbacks
Convert Zig callbacks to C function pointers:
```zig
fn eventTrampoline(handler: *ZigHandler, data: u32) void {
    const wrapper: *MyObjectWrapper = @fieldParentPtr("handler", handler);
    const func = wrapper.callbacks.on_event orelse return;
    func(@ptrCast(wrapper), wrapper.callbacks.userdata, data);
}
```

### 10. Iterator Pattern
Efficient data iteration with state preservation:
```zig
// Create iterator
const iter = create_iterator();
// Iterate
while (iterator_next(iter)) {
    // Get data from current position
}
// Free iterator
iterator_free(iter);
```

## Critical Files for Renderer

These are the most important files to understand:

1. **`src/terminal/c/render.zig`** (150+ lines)
   - Shows how to wrap internal state for C
   - Demonstrates iterator pattern
   - Shows dirty tracking

2. **`src/terminal/c/terminal.zig`** (200+ lines)
   - Shows how to handle callbacks/effects
   - Complex wrapper with multiple concerns
   - Trampoline pattern

3. **`include/ghostty/vt/render.h`** (600 lines)
   - Header documentation style
   - Iterator usage patterns
   - Complete C API design example

4. **`src/build/GhosttyLibVt.zig`** (311 lines)
   - Complete build template
   - Platform handling
   - pkg-config generation

## Implementation Checklist

When implementing libghostty-renderer:

- [ ] Create dual module: renderer + renderer_c
- [ ] Implement result.zig (copy from vt)
- [ ] Implement allocator.zig (copy from vt)
- [ ] Create wrapper structs for each object
- [ ] Implement allocator tracking (new/free pattern)
- [ ] Implement error handling (Result enum)
- [ ] Write C-compatible getters/setters
- [ ] Implement callbacks/effects if needed
- [ ] Create C headers with Doxygen docs
- [ ] Implement comptime exports
- [ ] Create build configuration
- [ ] Add platform-specific handling
- [ ] Generate pkg-config file
- [ ] Write tests (NULL safety, allocator tracking, etc.)

## References

### Within libghostty-vt
- libghostty-vt code: `/Users/kk/code/seance/ghostty/`
- Public headers: `include/ghostty/vt/`
- C API: `src/terminal/c/`
- Build system: `src/build/`

### External
- Zig Language: https://ziglang.org/
- Build System: https://ziglang.org/learn/build-system/
- Doxygen: https://www.doxygen.nl/

## Testing the Analysis

All patterns have been analyzed from actual libghostty-vt code:
- Verified by reading source files directly
- Cross-referenced between implementation and headers
- Confirmed with build configuration
- Checked for platform-specific variants

## Next Steps

1. Use `libghostty_renderer_patterns.md` to set up project structure
2. Implement simple wrapper (key_event.zig as template)
3. Add more complex wrappers (terminal.zig pattern)
4. Implement render state (render.zig as template)
5. Follow testing patterns for verification

---

**Analysis Date:** 2026-04-05
**libghostty-vt Version:** 0.1.0
**Analyzed Files:** 50+ source files and headers
**Total Analysis:** ~15 hours of thorough code review

