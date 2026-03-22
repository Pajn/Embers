mod context;
mod documentation;
mod engine;
mod error;
mod harness;
mod model;
mod runtime;
mod types;

pub(crate) type ScriptResult<T> = Result<T, Box<rhai::EvalAltResult>>;
pub(crate) type RhaiResultOf<T> = ScriptResult<T>;

pub use context::{
    BufferRef, Context, EventInfo, FloatingRef, NodeRef, SessionRef, TabBarContext, TabInfo,
};
pub use documentation::{build_mdbook, generate_config_api_docs};
pub use engine::ScriptEngine;
pub use error::ScriptError;
pub use harness::ScriptHarness;
pub use model::{
    Action, BufferSpawnSpec, FloatingAnchor, FloatingGeometrySpec, FloatingSize, FloatingSpec,
    NotifyLevel, TabSpec, TabsSpec, TreeSpec,
};
pub use types::{
    BarSegment, BarSpec, BarTarget, LoadedConfig, ModeHooks, MouseSettings, PaletteError, RgbColor,
    ScriptFunctionRef, StyleSpec, ThemeSpec,
};
