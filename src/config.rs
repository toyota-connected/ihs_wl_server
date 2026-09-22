/// Owned, validated form of `IhsWlConfig`.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Socket to bind under `$XDG_RUNTIME_DIR`; `None` auto-picks
    /// (`wayland-ihs-N` when this process is itself a Wayland client, else
    /// `wayland-N`).
    pub socket_name: Option<String>,
}
