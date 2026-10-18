use crate::{Client, Payload, Result, StackFrame};
use dap_types::DebugAdapterConfig;
use futures_executor::block_on;
use futures_util::stream::SelectAll;
use slotmap::SlotMap;
use std::fmt;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Owns debug-adapter clients and merges their incoming message streams.
///
/// A registry may contain several sessions, but editor commands currently act
/// on at most one active client. Removing a client drops its process handle and
/// invalidates its generational [`DebugAdapterId`].
pub struct Registry {
    inner: SlotMap<DebugAdapterId, Client>,
    supports_run_in_terminal: bool,
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
            supports_run_in_terminal: false,
            current_client_id: None,
            incoming: SelectAll::new(),
        }
    }

    /// Configure frontend terminal support before creating adapter connections.
    pub fn set_terminal_support(&mut self, supported: bool) {
        self.supports_run_in_terminal = supported;
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Starts and initializes a client from configuration.
    ///
    /// This method blocks the current thread while async startup and DAP
    /// initialization complete. It is intended for the editor's synchronous
    /// command boundary, not an async task.
    pub fn start_client(
        &mut self,
        socket: Option<std::net::SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> Result<DebugAdapterId> {
        self.inner.try_insert_with_key(|id| {
            let result = match socket {
                Some(socket) => block_on(Client::tcp(socket, id)),
                None => block_on(Client::process(
                    &config.transport,
                    &config.command,
                    config.args.iter().map(|arg| arg.as_str()).collect(),
                    config.port_arg.as_deref(),
                    id,
                )),
            };

            let (mut client, receiver) = result?;
            self.incoming.push(UnboundedReceiverStream::new(receiver));

            client.config = Some(config.clone());
            block_on(client.initialize(config.name.clone(), self.supports_run_in_terminal))?;
            client.quirks = config.quirks.clone();

            Ok(client)
        })
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
        self.inner.get(id)
    }

    /// Returns a mutable client by its generational identifier.
    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.inner.get_mut(id)
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

slotmap::new_key_type! {
    /// Generational identifier for a debug adapter owned by [`Registry`].
    pub struct DebugAdapterId;
}

impl fmt::Display for DebugAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
