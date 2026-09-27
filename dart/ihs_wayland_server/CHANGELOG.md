## 0.1.0

- Initial release: `WaylandServer` (start, launch, activation tokens, stop)
  and `WaylandToplevelView` (bind by activation token or app_id; pointer,
  touch, keyboard and cursor input).
- Build hook builds libihs_wl_server from the repository, or uses the
  installed library (`system_library`, or when not inside the repository).
