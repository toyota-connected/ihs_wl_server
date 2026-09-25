// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Input from the Dart platform-view controller, onto the seat.
//!
//! Every event is for one view, in view-local logical pixels. The view's
//! content starts at its toplevel's window geometry origin, so client-side
//! shadows fall outside it, the same as tree.rs lays the layers out. What is
//! under a point is hit-tested against the tree the view shows, input
//! regions included. Keys go to the view Dart last gave focus to, if it
//! shows a toplevel and is in the scene.

use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::WindowSurfaceType;
use smithay::input::keyboard::{FilterResult, KeyboardHandle, Keycode, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle};
use smithay::input::touch::{self, DownEvent, TouchHandle, UpEvent};
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::state::State;
use crate::{IhsWlPointerEvent, IhsWlPointerKind as P, IhsWlTouchEvent, IhsWlTouchKind as T};

/// The shell's repeat timing (key_repeater.h): clients repeat keys
/// themselves, from wl_keyboard.repeat_info.
const REPEAT_DELAY_MS: i32 = 600;
const REPEAT_RATE_HZ: i32 = 25;

/// Highest evdev key code (KEY_MAX).
pub(crate) const KEY_MAX: u32 = 0x2ff;

/// xkb keycodes are evdev codes offset by 8.
const XKB_OFFSET: u32 = 8;

/// The seat's devices.
pub struct Devices {
    pub pointer: PointerHandle<State>,
    pub touch: TouchHandle<State>,
    /// None when no keymap compiles (no xkeyboard-config on the target).
    pub keyboard: Option<KeyboardHandle<State>>,
    /// The pointer left its view mid-click: nothing is under it once the
    /// buttons are up.
    left: bool,
}

impl Devices {
    /// Give @p seat a pointer, touch and keyboard. The keymap is xkbcommon's
    /// default, which the XKB_DEFAULT_* variables override -- as the shell's
    /// own XkbKeyboard builds it, so both read keycodes the same way.
    pub fn add(seat: &mut Seat<State>) -> Self {
        let keyboard = seat
            .add_keyboard(XkbConfig::default(), REPEAT_DELAY_MS, REPEAT_RATE_HZ)
            .inspect_err(|e| tracing::warn!("no keyboard: {e}"))
            .ok();
        Devices {
            pointer: seat.add_pointer(),
            touch: seat.add_touch(),
            keyboard,
            left: false,
        }
    }
}

fn time_ms(time_us: u64) -> u32 {
    // Protocol times are milliseconds that wrap.
    (time_us / 1000) as u32
}

impl State {
    /// The surface under @p at in view @p view_id, and its origin in the
    /// same view-local space.
    fn surface_under(
        &self,
        view_id: i32,
        at: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        // Topmost first.
        self.view_trees(view_id)
            .iter()
            .rev()
            .find_map(|(root, start)| {
                under_from_surface_tree(root, at, *start, WindowSurfaceType::ALL)
            })
            .map(|(surface, loc)| (surface, loc.to_f64()))
    }

    fn pointer_in_view(&self, view_id: i32) -> bool {
        self.devices
            .pointer
            .current_focus()
            .is_some_and(|s| self.bound_view_of(&s) == Some(view_id))
    }

    pub fn pointer_input(&mut self, view_id: i32, ev: IhsWlPointerEvent) {
        let pointer = self.devices.pointer.clone();
        let time = time_ms(ev.time_us);
        let at: Point<f64, Logical> = (ev.x, ev.y).into();
        let motion = |state: &mut State, focus| {
            pointer.motion(
                state,
                focus,
                &MotionEvent {
                    location: at,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
        };
        match ev.kind {
            k if k == P::Enter as u32 || k == P::Motion as u32 => {
                self.devices.left = false;
                let focus = self.surface_under(view_id, at);
                motion(self, focus);
            }
            k if k == P::Leave as u32 => {
                // Leaving one view may come after entering the next.
                if !self.pointer_in_view(view_id) {
                    return;
                }
                // Mid-click, the implicit grab keeps the focus until the
                // release.
                self.devices.left = true;
                motion(self, None);
            }
            k if k == P::Button as u32 => {
                if pointer.current_location() != at || !self.pointer_in_view(view_id) {
                    let focus = self.surface_under(view_id, at);
                    motion(self, focus);
                }
                pointer.button(
                    self,
                    &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                        button: ev.button,
                        state: if ev.pressed != 0 {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                    },
                );
                // The implicit grab ends with the last button up, and leaves
                // the focus where the click started.
                if !pointer.is_grabbed() {
                    let focus = if self.devices.left {
                        None
                    } else {
                        self.surface_under(view_id, at)
                    };
                    if focus.as_ref().map(|f| &f.0) != pointer.current_focus().as_ref() {
                        motion(self, focus);
                    }
                }
            }
            k if k == P::Axis as u32 => {
                let source = match ev.axis_source {
                    1 => AxisSource::Finger,
                    2 => AxisSource::Continuous,
                    3 => AxisSource::WheelTilt,
                    _ => AxisSource::Wheel,
                };
                let mut frame = AxisFrame::new(time).source(source);
                for (axis, value, v120) in [
                    (Axis::Horizontal, ev.axis_x, ev.value120_x),
                    (Axis::Vertical, ev.axis_y, ev.value120_y),
                ] {
                    if value != 0.0 {
                        frame = frame.value(axis, value);
                    }
                    if v120 != 0 {
                        frame = frame.v120(axis, v120);
                    }
                }
                pointer.axis(self, frame);
            }
            _ => return,
        }
        pointer.frame(self);
    }

    pub fn touch_input(&mut self, view_id: i32, ev: IhsWlTouchEvent) {
        let touch = self.devices.touch.clone();
        let time = time_ms(ev.time_us);
        let slot = Some(ev.slot as u32).into();
        let at: Point<f64, Logical> = (ev.x, ev.y).into();
        match ev.kind {
            k if k == T::Down as u32 => {
                let focus = self.surface_under(view_id, at);
                touch.down(
                    self,
                    focus,
                    &DownEvent {
                        slot,
                        location: at,
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                    },
                );
            }
            k if k == T::Up as u32 => touch.up(
                self,
                &UpEvent {
                    slot,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            ),
            // The point stays with the surface it went down on.
            k if k == T::Motion as u32 => touch.motion(
                self,
                None,
                &touch::MotionEvent {
                    slot,
                    location: at,
                    time,
                },
            ),
            k if k == T::Cancel as u32 => touch.cancel(self),
            k if k == T::Frame as u32 => touch.frame(self),
            _ => {}
        }
    }

    pub fn key_input(&mut self, view_id: i32, evdev: u32, pressed: bool, time_us: u64) {
        if self.focused_view != Some(view_id) {
            return;
        }
        let Some(keyboard) = self.devices.keyboard.clone() else {
            return;
        };
        keyboard.input::<(), _>(
            self,
            Keycode::from(evdev + XKB_OFFSET),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            SERIAL_COUNTER.next_serial(),
            time_ms(time_us),
            |_, _, _| FilterResult::Forward,
        );
    }

    pub fn focus_input(&mut self, view_id: i32, focused: bool) {
        if focused {
            if self.focused_view == Some(view_id) {
                return;
            }
            self.focused_view = Some(view_id);
        } else if self.focused_view == Some(view_id) {
            self.focused_view = None;
            // As a click off a window would.
            self.dismiss_popups(view_id);
        } else {
            return;
        }
        self.release_keys();
        self.refresh_keyboard_focus();
    }

    /// Forget keys held across a focus change: their releases go wherever
    /// Flutter's focus went, never here, and xkb would keep a modifier down.
    /// The client is not told; losing focus drops its keys anyway.
    fn release_keys(&mut self) {
        let Some(keyboard) = self.devices.keyboard.clone() else {
            return;
        };
        for key in keyboard.pressed_keys() {
            keyboard.input::<(), _>(
                self,
                key,
                KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                0,
                |_, _, _| FilterResult::Intercept(()),
            );
        }
    }

    /// Point the keyboard at what the focused view shows now: after focus,
    /// binding, unbinding or suspension changed it.
    pub fn refresh_keyboard_focus(&mut self) {
        let Some(keyboard) = self.devices.keyboard.clone() else {
            return;
        };
        let target = self
            .focused_view
            .and_then(|id| self.views.get(&id))
            .filter(|v| !v.suspended)
            .and_then(|v| v.toplevel)
            .and_then(|t| self.toplevels.by_id.get(&t))
            .map(|t| t.surface.wl_surface().clone());
        if keyboard.current_focus() != target {
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
    }
}
