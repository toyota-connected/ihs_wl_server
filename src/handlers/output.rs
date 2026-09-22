use smithay::delegate_output;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Transform;
use smithay::wayland::output::OutputHandler;

use crate::state::State;

impl OutputHandler for State {}

delegate_output!(State);

/// The one virtual output clients see. Clients refuse to start without a
/// `wl_output` (foot: "no monitors available"). Its mode and scale follow the
/// bound view once views carry a scale; until then it is a nominal 1080p60.
pub fn virtual_output(dh: &DisplayHandle) -> Output {
    let output = Output::new(
        "ihs-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "ivi-homescreen".into(),
            model: "platform view".into(),
        },
    );
    let mode = Mode {
        size: (1920, 1080).into(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    output.create_global::<State>(dh);
    output
}
