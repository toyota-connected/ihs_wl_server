use smithay::delegate_viewporter;

use crate::state::State;

// Crop and scale come through as each layer's source and destination; the
// shell applies them.
delegate_viewporter!(State);
