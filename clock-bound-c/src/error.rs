// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use thiserror::Error;

/// ClockBoundCError enumerates all possible errors.
#[derive(Error, Debug)]
pub enum ClockBoundCError {
    /// Represents an error when trying to connect to ClockBoundD's socket.
    #[error("Could not connect to ClockBoundD's socket. {0}")]
    ConnectError(#[source] std::io::Error),
    /// Represents an error when trying to bind to a socket.
    #[error("Could not bind to socket. {0}")]
    BindError(#[source] std::io::Error),
    /// Represents an error when trying to set permissions on a socket file.
    #[error("Could not set permissions on socket. {0}")]
    SetPermissionsError(#[source] std::io::Error),
    /// Represents an error when trying to send a message to ClockBoundD.
    #[error("Could not send message to ClockBoundD. {0}")]
    SendMessageError(#[source] std::io::Error),
    /// Represents an error when trying receive a message from ClockBoundD.
    #[error("Could not receive message from ClockBoundD. {0}")]
    ReceiveMessageError(#[source] std::io::Error),
    /// Represents an error when trying to write a request.
    #[error("Could not write a request. {0}")]
    WriteRequestError(#[source] std::io::Error),
}
