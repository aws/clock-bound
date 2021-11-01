// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use log::{error, info};
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;

///
/// Create a local Unix Datagram Socket.
///
/// Remove any existing socket file at the exact same path, throwing an error if the removal fails.
/// Creates a socket and binds to it at the specified path.
///
/// # Arguments:
///
/// * `path`: The path and filename of the UNIX socket to be created. The directory up to
///          the filename must exist or it will fail.
pub fn create_unix_socket(path: &std::path::Path) -> UnixDatagram {
    // Remove existing socket file if found
    remove_socket_file(path);
    // Create socket and bind to path
    let sock = match UnixDatagram::bind(path) {
        Ok(s) => {
            info!("Created unix socket at path {}", path.display());
            info!("Connected to local unix socket at path {}", path.display());
            s
        }
        Err(e) => {
            panic!("Failed to bind to unix socket {:?}. Error: {:?}", path, e)
        }
    };

    let mode = 0o777;
    let permissions = fs::Permissions::from_mode(mode);

    // Set permissions to rwx rwx rwx so that an unprivileged process can
    // write requests to the socket.
    if let Err(err_permissions) = fs::set_permissions(path, permissions) {
        error!("Failed to set permissions: {}", err_permissions);
    };

    return sock;
}

/// Remove the socket file at the path if possible.
/// Checks if the path exists, then checks if the file is a socket, and removes it.
/// If the remove succeeds or the path doesn't exist at all, panic since we can not start the daemon
/// without a socket.
///
/// # Arguments:
/// * `path`: The path of the socket to be removed.
///
pub fn remove_socket_file(path: &std::path::Path) {
    if path.exists() {
        let metadata = fs::metadata(path);
        if let Err(e) = metadata {
            panic!(
                "Failed to check file metadata for removal of file: {:?}. Error: {:?}",
                path, e
            );
        }
        // Fail preemptively so that we don't remove non-socket files
        if !metadata.unwrap().file_type().is_socket() {
            panic!(
                "Failed to remove socket file. An existing file that is not a socket exists: {:?}",
                path
            );
        }

        if let Err(e) = fs::remove_file(&path) {
            panic!("Failed to remove socket file: {:?}. Error: {:?}", path, e);
        }
        info!(
            "Removed preexisting local unix socket at path {}",
            path.display()
        );
        return;
    }

    info!(
        "Local unix socket at path {} does not exist, skipping remove",
        path.display()
    );
}
