use crate::{Client, Payload, Result, StackFrame};
use dap_types::DebugAdapterConfig;
use futures_util::stream::SelectAll;
use slotmap::SlotMap;
use std::{fmt, future::Future};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Owns debug-adapter clients and merges their incoming message streams.
///
/// A registry may contain several sessions, but editor commands currently act
/// on at most one active client. Removing a client drops its process handle and
/// invalidates its generational [`DebugAdapterId`].
pub struct Registry {
    inner: SlotMap<DebugAdapterId, Option<Client>>,
    /// The active debugger client
    ///
    /// TODO: You can have multiple active debuggers, so the concept of a single active debugger
    /// may need to be changed
    current_client_id: Option<DebugAdapterId>,
    /// A stream of incoming messages from all debuggers
    pub incoming: SelectAll<UnboundedReceiverStream<(DebugAdapterId, Payload)>>,
}

impl Registry {
    /// Creates an empty registry with no active client.
    pub fn new() -> Self {
        Self {
            inner: SlotMap::with_key(),
            current_client_id: None,
            incoming: SelectAll::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Reserve a generational ID and return an owned connection future.
    /// Removing the ID before registration prevents a late connection from being installed.
    pub fn start_client(
        &mut self,
        socket: Option<std::net::SocketAddr>,
        config: DebugAdapterConfig,
    ) -> (
        DebugAdapterId,
        impl Future<Output = Result<Connection>> + Send + use<>,
    ) {
        let id = self.inner.insert(None);
        let connect = async move {
            let (mut client, incoming) = match socket {
                Some(socket) => Client::tcp(socket, id).await?,
                None => {
                    Client::process(
                        &config.transport,
                        &config.command,
                        config.args.iter().map(String::as_str).collect(),
                        config.port_arg.as_deref(),
                        id,
                    )
                    .await?
                }
            };
            client.quirks = config.quirks.clone();
            client.config = Some(config);
            Ok(Connection { client, incoming })
        };
        (id, connect)
    }

    /// Register incoming messages before initialization so reverse requests can be served.
    /// A removed or already registered connection cannot replace a live client.
    pub fn register_client(&mut self, connection: Connection) -> Option<&mut Client> {
        let slot = self.inner.get_mut(connection.client.id())?;
        if slot.is_some() {
            return None;
        }
        self.incoming
            .push(UnboundedReceiverStream::new(connection.incoming));
        *slot = Some(connection.client);
        slot.as_mut()
    }

    /// Whether an ID still belongs to a pending or connected adapter.
    pub fn contains(&self, id: DebugAdapterId) -> bool {
        self.inner.contains_key(id)
    }

    /// Removes a client if its identifier is still valid.
    pub fn remove_client(&mut self, id: DebugAdapterId) {
        self.inner.remove(id);
        if self.current_client_id == Some(id) {
            self.current_client_id = None;
        }
    }

    /// Returns a client by its generational identifier.
    pub fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.inner.get(id)?.as_ref()
    }

    /// Returns a mutable client by its generational identifier.
    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.inner.get_mut(id)?.as_mut()
    }

    /// Returns the client selected for editor commands.
    pub fn get_active_client(&self) -> Option<&Client> {
        self.current_client_id.and_then(|id| self.get_client(id))
    }

    /// Returns the active client mutably.
    pub fn get_active_client_mut(&mut self) -> Option<&mut Client> {
        self.current_client_id
            .and_then(|id| self.get_client_mut(id))
    }

    /// Selects `id`, or clears the active client when it is stale or unknown.
    pub fn set_active_client(&mut self, id: DebugAdapterId) {
        if self.get_client(id).is_some() {
            self.current_client_id = Some(id);
        } else {
            self.current_client_id = None;
        }
    }

    /// Clears the active-client selection without removing the client.
    pub fn unset_active_client(&mut self) {
        self.current_client_id = None;
    }

    /// Returns the selected stack frame of the active client, if any.
    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_client()
            .and_then(|debugger| debugger.current_stack_frame())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// An established transport awaiting registration and DAP initialization.
/// Dropping it also drops the owned adapter process.
pub struct Connection {
    client: Client,
    incoming: UnboundedReceiver<(DebugAdapterId, Payload)>,
}

slotmap::new_key_type! {
    /// Generational identifier for a debug adapter owned by [`Registry`].
    pub struct DebugAdapterId;
}

impl fmt::Display for DebugAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
