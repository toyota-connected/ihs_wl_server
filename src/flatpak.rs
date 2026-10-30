// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A client's flatpak app, from `/.flatpak-info` in its process's root.
//! Flatpak mounts it read-only in every sandbox, so a sandboxed client
//! cannot change it. A process outside any sandbox can stage a root of its
//! own (an unprivileged user namespace) and name any app, but such a
//! process runs as the shell's user and could do as much to the shell.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

use crate::handlers::security_context::IDENTITY_MAX;
use crate::FlatpakApp;

/// Far larger than any `.flatpak-info` flatpak writes.
const INFO_MAX: u64 = 64 * 1024;

/// The flatpak app of the process at the other end of @p stream.
pub fn of_stream(stream: &UnixStream) -> Option<FlatpakApp> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for the call, len its size.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    of_pid(cred.pid)
}

/// The flatpak app the process @p pid runs in, if it is in one.
fn of_pid(pid: i32) -> Option<FlatpakApp> {
    if pid <= 0 {
        return None;
    }
    // The root is the client's: a link would resolve against ours, and a
    // FIFO or device would block or never end.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(format!("/proc/{pid}/root/.flatpak-info"))
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut info = String::new();
    file.take(INFO_MAX).read_to_string(&mut info).ok()?;
    parse(&info)
}

/// The app and instance a `.flatpak-info` keyfile names. Values longer than
/// `IDENTITY_MAX` are dropped.
fn parse(info: &str) -> Option<FlatpakApp> {
    let mut section = "";
    let (mut app_id, mut instance_id) = (None, None);
    for line in info.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value.len() > IDENTITY_MAX {
            continue;
        }
        match (section, key.trim()) {
            ("Application", "name") => app_id = Some(value.to_owned()),
            ("Instance", "instance-id") => instance_id = Some(value.to_owned()),
            _ => {}
        }
    }
    Some(FlatpakApp {
        app_id: app_id?,
        instance_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_app_and_instance() {
        let info = "[Application]\nname=org.libreoffice.LibreOffice\nruntime=runtime/x\n\n\
                    [Instance]\ninstance-id=2029383367\napp-path=/app\n\n[Context]\nshared=ipc;\n";
        assert_eq!(
            parse(info),
            Some(FlatpakApp {
                app_id: "org.libreoffice.LibreOffice".into(),
                instance_id: Some("2029383367".into()),
            })
        );
    }

    #[test]
    fn takes_name_only_from_its_section() {
        // A runtime has [Runtime] name=, not an app.
        assert_eq!(parse("[Runtime]\nname=org.example.Platform\n"), None);
        assert_eq!(parse(""), None);
        let long = format!("[Application]\nname={}\n", "a".repeat(IDENTITY_MAX + 1));
        assert_eq!(parse(&long), None);
        assert_eq!(
            parse("[Application]\nname=org.example.App\n"),
            Some(FlatpakApp {
                app_id: "org.example.App".into(),
                instance_id: None,
            })
        );
    }

    #[test]
    fn no_app_outside_a_sandbox() {
        // This test process runs on the host.
        assert_eq!(of_pid(std::process::id() as i32), None);
        assert_eq!(of_pid(0), None);
    }
}
