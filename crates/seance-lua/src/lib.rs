//! Embedded Lua runtime for seance.
//!
//! [`LuaRuntime`] wraps a sandboxed `mlua` VM and bootstraps user scripting
//! from `init.lua`. The host constructs one runtime, calls [`LuaRuntime::load_init`]
//! at startup (and again on config change after [`LuaRuntime::reset`]), and turns
//! a returned [`InitError`] into a notification without tearing anything down:
//! the VM stays usable after a failed load.

use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use mlua::{Lua, LuaOptions, StdLib, Value, Variadic};

const INIT_FILENAME: &str = "init.lua";
const LUA_SUBDIR: &str = "lua";

/// Where `print` output is routed. The default forwards to `tracing`; tests
/// (and, later, the notification surface) can inject their own sink. The VM is
/// single-threaded, so the sink is an `Rc` and need not be `Send`.
pub type PrintSink = Rc<dyn Fn(&str)>;

/// Inputs the runtime needs to bootstrap: the version string exposed as
/// `seance.version`, and the config directory that holds `init.lua` and the
/// `lua/` module search root.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub version: String,
    pub config_dir: PathBuf,
}

/// Outcome of a successful [`LuaRuntime::load_init`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitStatus {
    /// No `init.lua` was present. A fresh install with no config is normal.
    Missing,
    /// `init.lua` was read and executed without error.
    Loaded,
}

/// Why loading `init.lua` failed. Neither variant is fatal — the host logs or
/// renders the error and keeps the VM (and the terminal) alive.
#[derive(Debug)]
pub enum InitError {
    /// `init.lua` exists but could not be read from disk.
    Read { path: PathBuf, source: io::Error },
    /// `init.lua` failed to parse or raised at runtime.
    Script(mlua::Error),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Read { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            InitError::Script(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InitError::Read { source, .. } => Some(source),
            InitError::Script(err) => Some(err),
        }
    }
}

pub struct LuaRuntime {
    lua: Lua,
    config: RuntimeConfig,
    sink: PrintSink,
}

impl LuaRuntime {
    /// Build a runtime whose `print` forwards to `tracing`.
    pub fn new(config: RuntimeConfig) -> mlua::Result<Self> {
        Self::with_print_sink(config, default_print_sink())
    }

    /// Build a runtime routing `print` output through `sink`.
    pub fn with_print_sink(config: RuntimeConfig, sink: PrintSink) -> mlua::Result<Self> {
        let lua = build_vm(&config, &sink)?;
        Ok(Self { lua, config, sink })
    }

    /// Read `<config_dir>/init.lua` and execute it. A missing file yields
    /// [`InitStatus::Missing`]; parse/runtime failures yield [`InitError`]
    /// without disturbing the VM.
    pub fn load_init(&self) -> Result<InitStatus, InitError> {
        let path = self.config.config_dir.join(INIT_FILENAME);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(InitStatus::Missing),
            Err(err) => return Err(InitError::Read { path, source: err }),
        };
        self.lua
            .load(source)
            .set_name(format!("@{}", path.display()))
            .exec()
            .map_err(InitError::Script)?;
        Ok(InitStatus::Loaded)
    }

    /// Discard all script state and rebuild a clean sandboxed VM. The host
    /// calls this before re-running [`LuaRuntime::load_init`] on config change
    /// so a reload starts from a known baseline rather than accreting state.
    pub fn reset(&mut self) -> mlua::Result<()> {
        self.lua = build_vm(&self.config, &self.sink)?;
        Ok(())
    }

    /// Execute an inline chunk. Primarily for host glue and tests.
    pub fn exec(&self, chunk: &str) -> mlua::Result<()> {
        self.lua.load(chunk).exec()
    }

    /// Evaluate an inline expression chunk and convert the result.
    pub fn eval<T: mlua::FromLuaMulti>(&self, chunk: &str) -> mlua::Result<T> {
        self.lua.load(chunk).eval()
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

fn build_vm(config: &RuntimeConfig, sink: &PrintSink) -> mlua::Result<Lua> {
    // Curated standard library: no `os`/`io`, so `os.execute` and `io.popen`
    // are simply absent. `package` is included so `require` resolves user
    // modules; its C-loader surface is stripped in `configure_package`.
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::PACKAGE;
    let lua = Lua::new_with(libs, LuaOptions::default())?;

    install_print(&lua, sink.clone())?;
    scrub_unsafe_base(&lua)?;
    configure_package(&lua, config)?;
    install_seance_global(&lua, &config.version)?;

    Ok(lua)
}

/// Replace `print` with one that renders each argument through Lua `tostring`,
/// joins with tabs (matching stock `print`), and hands the line to `sink`.
fn install_print(lua: &Lua, sink: PrintSink) -> mlua::Result<()> {
    let print = lua.create_function(move |lua, args: Variadic<Value>| {
        let tostring: mlua::Function = lua.globals().get("tostring")?;
        let mut line = String::new();
        for (i, value) in args.into_iter().enumerate() {
            if i > 0 {
                line.push('\t');
            }
            let text: mlua::LuaString = tostring.call(value)?;
            line.push_str(&text.to_string_lossy());
        }
        sink(&line);
        Ok(())
    })?;
    lua.globals().set("print", print)?;
    Ok(())
}

/// Remove the base-library file loaders that read arbitrary paths off disk.
/// `require` keeps working: its module search runs through `package`, not the
/// global `loadfile`.
fn scrub_unsafe_base(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.set("loadfile", Value::Nil)?;
    globals.set("dofile", Value::Nil)?;
    Ok(())
}

/// Point `require` at `<config_dir>/lua/` and disable native-module loading.
fn configure_package(lua: &Lua, config: &RuntimeConfig) -> mlua::Result<()> {
    let package: mlua::Table = lua.globals().get("package")?;
    let lua_dir = config.config_dir.join(LUA_SUBDIR);
    let base = lua_dir.display();
    package.set("path", format!("{base}/?.lua;{base}/?/init.lua"))?;
    // No C modules: empty cpath and a nil loader keep `require` to Lua sources.
    package.set("cpath", "")?;
    package.set("loadlib", Value::Nil)?;
    Ok(())
}

/// Install the `seance` root namespace. Sub-issues (opt, keymap, event, …)
/// hang their tables off this one; today it carries version identity.
fn install_seance_global(lua: &Lua, version: &str) -> mlua::Result<()> {
    let seance = lua.create_table()?;
    seance.set("version", version)?;
    lua.globals().set("seance", seance)?;
    Ok(())
}

fn default_print_sink() -> PrintSink {
    Rc::new(|line: &str| tracing::info!(target: "seance_lua::print", "{line}"))
}

/// Collect `print` output into a shared buffer. Handy for tests and for a host
/// that wants to mirror script output into a scrollback or overlay.
pub fn capturing_sink() -> (PrintSink, Rc<std::cell::RefCell<Vec<String>>>) {
    let buffer = Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink_buffer = Rc::clone(&buffer);
    let sink: PrintSink = Rc::new(move |line: &str| {
        sink_buffer.borrow_mut().push(line.to_owned());
    });
    (sink, buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: PathBuf) -> RuntimeConfig {
        RuntimeConfig {
            version: "9.9.9-test".to_string(),
            config_dir: dir,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "seance-lua-{tag}-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn base_and_seance_globals_are_present() {
        let rt = LuaRuntime::new(config(temp_dir("globals"))).unwrap();
        assert_eq!(rt.eval::<String>("return type(print)").unwrap(), "function");
        assert_eq!(
            rt.eval::<String>("return type(tostring)").unwrap(),
            "function"
        );
        assert_eq!(
            rt.eval::<String>("return seance.version").unwrap(),
            "9.9.9-test"
        );
    }

    #[test]
    fn missing_init_starts_normally() {
        let rt = LuaRuntime::new(config(temp_dir("missing"))).unwrap();
        assert_eq!(rt.load_init().unwrap(), InitStatus::Missing);
    }

    #[test]
    fn empty_init_loads() {
        let dir = temp_dir("empty");
        std::fs::write(dir.join(INIT_FILENAME), "").unwrap();
        let rt = LuaRuntime::new(config(dir)).unwrap();
        assert_eq!(rt.load_init().unwrap(), InitStatus::Loaded);
    }

    #[test]
    fn print_routes_to_sink() {
        let dir = temp_dir("print");
        std::fs::write(dir.join(INIT_FILENAME), r#"print("hello", 1, true)"#).unwrap();
        let (sink, buffer) = capturing_sink();
        let rt = LuaRuntime::with_print_sink(config(dir), sink).unwrap();
        assert_eq!(rt.load_init().unwrap(), InitStatus::Loaded);
        assert_eq!(buffer.borrow().as_slice(), &["hello\t1\ttrue".to_string()]);
    }

    #[test]
    fn syntax_error_is_reported_and_vm_survives() {
        let dir = temp_dir("syntax");
        std::fs::write(dir.join(INIT_FILENAME), "this is not lua (").unwrap();
        let rt = LuaRuntime::new(config(dir)).unwrap();
        assert!(matches!(rt.load_init(), Err(InitError::Script(_))));
        // The VM is still usable after a failed init.
        assert_eq!(rt.eval::<i64>("return 1 + 1").unwrap(), 2);
    }

    #[test]
    fn runtime_error_is_reported() {
        let dir = temp_dir("runtime");
        std::fs::write(dir.join(INIT_FILENAME), r#"error("boom")"#).unwrap();
        let rt = LuaRuntime::new(config(dir)).unwrap();
        assert!(matches!(rt.load_init(), Err(InitError::Script(_))));
    }

    #[test]
    fn dangerous_stdlib_is_absent() {
        let rt = LuaRuntime::new(config(temp_dir("sandbox"))).unwrap();
        assert_eq!(rt.eval::<String>("return type(os)").unwrap(), "nil");
        assert_eq!(rt.eval::<String>("return type(io)").unwrap(), "nil");
        assert_eq!(rt.eval::<String>("return type(loadfile)").unwrap(), "nil");
        assert_eq!(rt.eval::<String>("return type(dofile)").unwrap(), "nil");
    }

    #[test]
    fn require_resolves_user_modules() {
        let dir = temp_dir("require");
        std::fs::create_dir_all(dir.join(LUA_SUBDIR)).unwrap();
        std::fs::write(
            dir.join(LUA_SUBDIR).join("greet.lua"),
            "return { hi = function() return 'hi from module' end }",
        )
        .unwrap();
        std::fs::write(dir.join(INIT_FILENAME), r#"result = require("greet").hi()"#).unwrap();
        let rt = LuaRuntime::new(config(dir)).unwrap();
        assert_eq!(rt.load_init().unwrap(), InitStatus::Loaded);
        assert_eq!(
            rt.eval::<String>("return result").unwrap(),
            "hi from module"
        );
    }

    #[test]
    fn reset_clears_script_state() {
        let mut rt = LuaRuntime::new(config(temp_dir("reset"))).unwrap();
        rt.exec("marker = 42").unwrap();
        assert_eq!(rt.eval::<i64>("return marker").unwrap(), 42);
        rt.reset().unwrap();
        assert_eq!(rt.eval::<String>("return type(marker)").unwrap(), "nil");
        // Sandbox invariants hold after a reset.
        assert_eq!(
            rt.eval::<String>("return seance.version").unwrap(),
            "9.9.9-test"
        );
    }
}
