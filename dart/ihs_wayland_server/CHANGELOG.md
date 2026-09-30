## 0.1.0

- Initial release: `WaylandServer` (start, launch, activation tokens, stop)
  and `WaylandToplevelView` (bind by activation token or app_id; pointer,
  touch, keyboard and cursor input; `requestedSize` to configure the client
  to a size of its own, scaled to fit the view), and
  `WaylandToplevelController` (bound or closed, the client's window-state
  requests and the buttons it shows for them, the window states the app
  grants, `close()`). A view follows its layout size, and a window larger
  than asked is scaled down to fit.
- Build hook builds libihs_wl_server from the repository into the app's
  bundle, or uses the installed library (`system_library`, or when not inside
  the repository). The bundled copy is loaded first.
