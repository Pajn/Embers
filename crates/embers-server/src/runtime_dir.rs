//! Location of the per-buffer runtime keeper sockets.
//!
//! Keeper sockets normally live next to the server socket, in
//! `<socket>.runtimes/`. Unix socket paths are limited to the size of
//! `sockaddr_un.sun_path`, and that directory is longer than the server socket
//! path itself, so a server socket close to the limit leaves no room for
//! keeper sockets. Those keeper sockets go in a short private directory under
//! `/tmp/embers-<uid>/` instead.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use embers_core::{BufferId, MuxError, Result};

/// Size of `sockaddr_un.sun_path` in bytes: macOS exposes 104 bytes per
/// `unix(4)`, while other Unix/Linux platforms expose 108 bytes per `unix(7)`.
/// A bindable path must be shorter than this to leave room for the trailing
/// NUL.
#[cfg(target_os = "macos")]
const UNIX_SOCKET_PATH_LIMIT: usize = 104;
/// Size of `sockaddr_un.sun_path` in bytes: macOS exposes 104 bytes per
/// `unix(4)`, while other Unix/Linux platforms expose 108 bytes per `unix(7)`.
/// A bindable path must be shorter than this to leave room for the trailing
/// NUL.
#[cfg(all(unix, not(target_os = "macos")))]
const UNIX_SOCKET_PATH_LIMIT: usize = 108;

/// File name of the keeper socket for `buffer_id` inside the runtime dir.
fn keeper_socket_name(buffer_id: BufferId) -> String {
    format!("buffer-{}.sock", buffer_id.0)
}

/// Whether `path` is short enough to bind as a Unix socket.
fn fits_unix_socket_path(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() < UNIX_SOCKET_PATH_LIMIT
}

/// Path of the keeper socket for `buffer_id` of the server on `socket_path`.
///
/// Prefers `runtime_dir`. When the socket would not fit there, it goes in a
/// short directory under the shared base instead, named after a hash of the
/// server socket path, so the location is deterministic per server and buffer
/// and distinct between servers. That path is at most 81 bytes, well within
/// the limit.
pub(crate) fn keeper_socket_path(
    socket_path: &Path,
    runtime_dir: &Path,
    buffer_id: BufferId,
) -> PathBuf {
    let name = keeper_socket_name(buffer_id);
    let preferred = runtime_dir.join(&name);
    if fits_unix_socket_path(&preferred) {
        return preferred;
    }
    let absolute = std::path::absolute(socket_path).unwrap_or_else(|_| socket_path.to_path_buf());
    shared_runtime_base()
        .join(format!(
            "runtimes-{:016x}",
            fnv1a_64(absolute.as_os_str().as_encoded_bytes())
        ))
        .join(name)
}

/// Create the directory holding `keeper_socket` if needed and make sure it is
/// private to the current user. When it is a hashed directory under the shared
/// base, the base is checked as well, since whoever controls it controls its
/// entries.
pub(crate) fn prepare_keeper_socket_dir(keeper_socket: &Path) -> Result<()> {
    let Some(runtime_dir) = keeper_socket.parent() else {
        return Ok(());
    };
    let base = shared_runtime_base();
    if runtime_dir.parent() == Some(base.as_path()) {
        ensure_private_dir(&base)?;
    }
    ensure_private_dir(runtime_dir)
}

/// Short per-user directory that holds the keeper sockets that do not fit in
/// the configured runtime dir. Mirrors the client's `/tmp/embers-<uid>` fallback.
fn shared_runtime_base() -> PathBuf {
    PathBuf::from("/tmp").join(format!("embers-{}", effective_uid()))
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    // `symlink_metadata` so a symlink planted in place of the directory is
    // rejected rather than followed.
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(MuxError::invalid_input(format!(
            "runtime directory {} is not a directory",
            path.display()
        )));
    }
    if metadata.uid() != effective_uid() {
        return Err(MuxError::invalid_input(format!(
            "runtime directory {} is not owned by uid {}",
            path.display(),
            effective_uid()
        )));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// 64-bit FNV-1a. Used instead of `DefaultHasher` because its output is
/// specified, so a given socket path maps to the same directory across builds.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded_socket_path(len: usize) -> PathBuf {
        let prefix = "/tmp/";
        let suffix = "/mux.sock";
        PathBuf::from(format!(
            "{prefix}{}{suffix}",
            "d".repeat(len - prefix.len() - suffix.len())
        ))
    }

    fn keeper_for(socket: &Path, buffer_id: BufferId) -> PathBuf {
        keeper_socket_path(socket, &socket.with_extension("runtimes"), buffer_id)
    }

    #[test]
    fn short_socket_keeps_keepers_in_derived_runtime_dir() {
        let socket = PathBuf::from("/tmp/embers-test/embers.sock");
        assert_eq!(
            keeper_for(&socket, BufferId(7)),
            PathBuf::from("/tmp/embers-test/embers.runtimes/buffer-7.sock")
        );
    }

    #[test]
    fn any_bindable_socket_yields_bindable_keeper_sockets() {
        for len in 20..UNIX_SOCKET_PATH_LIMIT {
            let socket = padded_socket_path(len);
            assert!(fits_unix_socket_path(&socket));
            for buffer_id in [BufferId(1), BufferId(u64::MAX)] {
                let keeper = keeper_for(&socket, buffer_id);
                assert!(
                    fits_unix_socket_path(&keeper),
                    "keeper socket {} for server socket of {len} bytes is too long",
                    keeper.display()
                );
            }
        }
    }

    #[test]
    fn long_sockets_get_distinct_deterministic_keeper_dirs() {
        let first = padded_socket_path(UNIX_SOCKET_PATH_LIMIT - 1);
        let second = first.with_file_name("other.sock");
        let first_keeper = keeper_for(&first, BufferId(1));
        assert_eq!(first_keeper, keeper_for(&first, BufferId(1)));
        assert_eq!(
            first_keeper.parent(),
            keeper_for(&first, BufferId(2)).parent()
        );
        assert_ne!(
            first_keeper.parent(),
            keeper_for(&second, BufferId(1)).parent()
        );
        assert_eq!(
            first_keeper.parent().and_then(Path::parent),
            Some(shared_runtime_base().as_path())
        );
    }

    #[test]
    fn fnv1a_64_matches_reference_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn prepare_keeper_socket_dir_creates_private_dir_and_tightens_existing_one() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let runtime_dir = tempdir.path().join("mux.runtimes");
        let keeper = runtime_dir.join("buffer-1.sock");

        prepare_keeper_socket_dir(&keeper).expect("create runtime dir");
        let mode = fs::metadata(&runtime_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_keeper_socket_dir(&keeper).expect("tighten runtime dir");
        let mode = fs::metadata(&runtime_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn prepare_keeper_socket_dir_rejects_symlink() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let target = tempdir.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = tempdir.path().join("mux.runtimes");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(prepare_keeper_socket_dir(&link.join("buffer-1.sock")).is_err());
    }
}
