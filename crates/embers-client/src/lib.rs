pub mod client;
pub mod config;
pub mod configured_client;
pub mod controller;
pub mod grid;
pub mod input;
pub mod presentation;
pub mod renderer;
pub mod scripting;
pub mod socket_transport;
pub mod state;
pub mod testing;
pub mod transport;

pub use client::MuxClient;
pub use config::{
    BUILTIN_CONFIG_SOURCE, CONFIG_ENV_VAR, ConfigDiscoveryOptions, ConfigError,
    ConfigManager, ConfigManagerError, ConfigOrigin, DiscoveredConfig, LoadedConfigSource,
    config_file_in_dir, default_config_path, discover_config, load_config_source,
};
pub use configured_client::ConfiguredClient;
pub use controller::{Controller, KeyEvent};
pub use grid::{BorderStyle, RenderGrid};
pub use input::{
    BindingMatch, BindingSpec, COPY_MODE, FallbackPolicy, InputResolution, InputState,
    KeyParseError, KeySequence, KeyToken, ModeSpec, NORMAL_MODE, SELECT_MODE, expand_leader,
    parse_key_sequence, resolve_key,
};
pub use presentation::{
    DividerFrame, FloatingFrame, LeafFrame, NavigationDirection, PresentationModel, TabItem,
    TabsFrame,
};
pub use renderer::Renderer;
pub use scripting::{
    Action, BufferRef, BufferSpawnSpec, BufferTarget, Context, FloatingOptions, FloatingRef,
    LoadedConfig, NodeRef, NodeTarget, PaletteError, RgbColor, ScriptEngine, ScriptError,
    ScriptFunctionRef, ScriptHarness, SessionRef, TabBarContext, TabSpec, TabStateRef, ThemeSpec,
    TreeSpec, WeightedTreeSpec, BarSpec, SegmentSpec,
};
pub use socket_transport::SocketTransport;
pub use state::ClientState;
pub use testing::{FakeTransport, ScriptedTransport, TestGrid};
pub use transport::Transport;
