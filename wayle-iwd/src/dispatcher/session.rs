//! Subscribing to, and enumerating, a iwd instance.

use std::{future::Future, pin::Pin, time::Duration};

use futures::StreamExt;
use tracing::debug;
use zbus::{
    Connection, MatchRule, Message, MessageStream,
    fdo::{self, ManagedObjects},
    names::BusName,
    proxy::CacheProperties,
};

use crate::error::Error;

/// Capacity of the subscription's queue. The dispatcher is fast, so this only
/// has to absorb bursts (e.g. a scan finding many networks).
const QUEUE: usize = 1024;

/// Delay before retrying a failed enumeration, doubling up to [`MAX_RETRY`].
const INITIAL_RETRY: Duration = Duration::from_millis(500);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// Result of enumerating a iwd instance: its unique bus name, the live
/// subscription, the objects it exports, and the messages received while
/// enumerating.
pub(super) struct Session {
    pub owner: String,
    pub stream: MessageStream,
    pub objects: ManagedObjects,
    pub buffered: Vec<Message>,
}

impl Session {
    /// Subscribes to every message from `owner` (iwd's unique bus name),
    /// then enumerates its objects.
    ///
    /// The subscription is not limited to signals: it also carries IWD's
    /// replies to our requests, interleaved with its signals in the order
    /// IWD sent them. Subscribing first means nothing that changes during
    /// enumeration is missed. Messages received while the reply is pending are
    /// buffered rather than left queued: zbus applies backpressure to a full
    /// subscription by stalling the connection's reader, which would also
    /// stall the reply and deadlock enumeration.
    pub(super) async fn open(connection: &Connection, owner: &str) -> Result<Self, Error> {
        let rule = MatchRule::builder()
            .sender(BusName::try_from(owner.to_owned()).map_err(zbus::Error::from)?)?
            .build();
        let mut stream = MessageStream::for_match_rule(rule, connection, Some(QUEUE)).await?;

        let object_manager = fdo::ObjectManagerProxy::builder(connection)
            .destination(BusName::try_from(owner.to_owned()).map_err(zbus::Error::from)?)?
            .path("/")?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;

        let objects = object_manager.get_managed_objects();
        tokio::pin!(objects);

        let mut buffered = Vec::new();
        loop {
            tokio::select! {
                result = &mut objects => {
                    return Ok(Self {
                        owner: owner.to_owned(),
                        stream,
                        objects: result.map_err(Error::Enumeration)?,
                        buffered,
                    });
                }
                Some(Ok(message)) = stream.next() => buffered.push(message),
            }
        }
    }
}

type SessionFuture = Pin<Box<dyn Future<Output = Result<Session, Error>> + Send>>;

/// The dispatcher's connection to the current iwd instance.
pub(super) enum Link {
    /// iwd is not on the bus.
    Down,
    /// Enumerating a newly appeared iwd.
    Syncing {
        owner: String,
        /// How long this attempt waited before starting.
        delay: Duration,
        session: SessionFuture,
    },
    /// Consuming the current iwd's messages.
    Up {
        owner: String,
        stream: MessageStream,
    },
}

pub(super) enum LinkEvent {
    Synced(Result<Session, Error>),
    Message(Message),
}

impl Link {
    /// The current iwd's unique bus name, if it's on the bus.
    pub(super) fn owner(&self) -> Option<&str> {
        match self {
            Self::Down => None,
            Self::Syncing { owner, .. } | Self::Up { owner, .. } => Some(owner),
        }
    }

    /// Enumerates `owner` after `delay`.
    pub(super) fn syncing(connection: &Connection, owner: String, delay: Duration) -> Self {
        let connection = connection.clone();
        let session_owner = owner.clone();
        let session = Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Session::open(&connection, &session_owner).await
        });

        Self::Syncing {
            owner,
            delay,
            session,
        }
    }

    /// Retries a failed enumeration of the same owner, backing off.
    pub(super) fn retry(&self, connection: &Connection) -> Option<Self> {
        let Self::Syncing { owner, delay, .. } = self else {
            return None;
        };

        let delay = (*delay * 2).clamp(INITIAL_RETRY, MAX_RETRY);
        Some(Self::syncing(connection, owner.clone(), delay))
    }

    pub(super) async fn next(&mut self) -> LinkEvent {
        match self {
            Self::Down => std::future::pending().await,
            Self::Syncing { session, .. } => LinkEvent::Synced(session.await),
            Self::Up { stream, .. } => loop {
                match stream.next().await {
                    Some(Ok(message)) => return LinkEvent::Message(message),
                    Some(Err(err)) => debug!(error = %err, "cannot read iwd message"),
                    None => std::future::pending::<()>().await,
                }
            },
        }
    }
}
