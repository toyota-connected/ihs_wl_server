// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::{delegate_commit_timing, delegate_fifo, delegate_presentation};

use crate::state::State;

// Feedback, fifo barriers and commit timers are all driven by the shell's
// reports of shown frames; see timing.rs.
delegate_presentation!(State);
delegate_fifo!(State);
delegate_commit_timing!(State);
