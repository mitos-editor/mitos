//! Cancellable connection, initialization, and launch tasks for debugger sessions.

use super::DebuggerLaunch;
use crate::{callbacks::EditorCallbackSender, events::DocumentDidClose, DocumentId, Editor};
use anyhow::Context as _;
use dap::{ConnectionType, DebugAdapterId, DebuggerCapabilities};
use event::{register_hook, TaskController, TaskHandle};
use language_config::{DebugAdapterConfig, DebugAdapterTransport};
use serde_json::Value;
use std::{future::Future, net::SocketAddr};

pub(super) struct Startup {
    cancel: TaskController,
    origin: StartupOrigin,
    initialized: bool,
}

#[derive(Clone, Copy)]
pub(super) enum StartupOrigin {
    Connection,
    Document(DocumentId),
    Child {
        parent: DebugAdapterId,
        sequence: u64,
    },
}

impl Startup {
    pub(super) fn parent(&self) -> Option<DebugAdapterId> {
        match self.origin {
            StartupOrigin::Child { parent, .. } => Some(parent),
            _ => None,
        }
    }
}

impl super::DebuggerState {
    pub(super) fn cancel_startup_requests(&mut self, id: DebugAdapterId) {
        let Some(startup) = self
            .sessions
            .get_mut(id)
            .and_then(|session| session.startup.take())
        else {
            return;
        };
        let origin = startup.origin;
        drop(startup);
        if let StartupOrigin::Child { parent, sequence } = origin
            && let Some(client) = self.get_client(parent)
            && let Err(error) = client.reply(
                sequence,
                "startDebugging",
                Err(anyhow::anyhow!("Child debugging startup was canceled").into()),
            )
        {
            log::warn!("Could not cancel child debugging request: {error}");
        }
    }
}

struct StartupContext {
    id: DebugAdapterId,
    owner: EditorCallbackSender,
    cancel: TaskHandle,
    origin: StartupOrigin,
}

impl StartupContext {
    fn spawn<T: Send + 'static>(
        self,
        future: impl Future<Output = dap::Result<T>> + Send + 'static,
        apply: impl FnOnce(&mut Editor, Self, T) + Send + 'static,
    ) {
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = self.cancel.canceled() => return,
                result = future => result,
            };
            let callbacks = self.owner.clone();
            callbacks
                .send(move |editor| {
                    if self.cancel.is_canceled()
                        || !self.owner.same_destination(editor.callback_sender())
                        || !editor.debugger.sessions.contains_key(self.id)
                    {
                        return;
                    }
                    // Focus changes do not retarget startup. Closing its source or
                    // revoking workspace trust does prevent further startup requests.
                    if let StartupOrigin::Document(document) = self.origin
                        && !editor.document(document).is_some_and(|doc| {
                            editor
                                .workspace_trust
                                .query(doc.workspace_root(), workspace::trust::TrustQuery::Dap)
                                .is_trusted()
                        })
                    {
                        editor.debugger.remove_session(self.id);
                        return;
                    }
                    match result {
                        Ok(result) => apply(editor, self, result),
                        Err(error) => self.finish(editor, Err(error)),
                    }
                })
                .await;
        });
    }

    fn finish(self, editor: &mut Editor, result: dap::Result<Value>) {
        let active = editor
            .debugger
            .get_active_session()
            .is_some_and(|session| session.id() == self.id);
        editor.debugger.sessions[self.id].startup = None;
        if result.is_err() {
            editor.debugger.remove_session(self.id);
        }
        match self.origin {
            StartupOrigin::Child { parent, sequence } => {
                editor.complete_debugger_request(parent, sequence, "startDebugging", result);
            }
            _ => {
                if let Err(error) = result {
                    if active {
                        editor.set_error(|| format!("Failed to start debugging: {error}"));
                    } else {
                        log::warn!("Failed to start debugging session {}: {error}", self.id);
                    }
                }
            }
        }
    }
}

impl Editor {
    /// Connect, initialize, and optionally launch an already authorized configuration.
    /// Call `start_debugger` to validate a document's trust and launch template.
    /// The returned ID can be canceled before connection or initialization completes.
    pub fn connect_debugger(
        &mut self,
        socket: Option<SocketAddr>,
        config: DebugAdapterConfig,
        launch: Option<DebuggerLaunch>,
    ) -> DebugAdapterId {
        self.begin_debugger_startup(socket, config, launch, StartupOrigin::Connection)
    }

    pub(super) fn begin_debugger_startup(
        &mut self,
        socket: Option<SocketAddr>,
        config: DebugAdapterConfig,
        launch: Option<DebuggerLaunch>,
        origin: StartupOrigin,
    ) -> DebugAdapterId {
        let name = config.name.clone();
        let terminal_support = self.debugger.terminal_launcher.is_some();
        let parent = match origin {
            StartupOrigin::Child { parent, .. } => Some(parent),
            _ => None,
        };
        let id = self.debugger.insert_session(name.clone(), parent);
        self.debugger.sessions[id].adapter = Some(config.clone());
        let connect = async move {
            if let Some(socket) = socket {
                return dap::Client::tcp(socket, id).await;
            }
            if config.command.is_empty() {
                return Err(anyhow::anyhow!("Debug adapter command is empty").into());
            }
            let args = config.args.iter().map(String::as_str).collect();
            match config.transport {
                DebugAdapterTransport::Stdio => dap::Client::stdio(&config.command, args, id),
                DebugAdapterTransport::Tcp => {
                    let port = config
                        .port_arg
                        .as_deref()
                        .context("TCP debug adapter requires port-arg")?;
                    dap::Client::tcp_process(&config.command, args, port, id).await
                }
            }
        };
        let mut startup = Startup {
            cancel: TaskController::new(),
            origin,
            initialized: false,
        };
        let context = StartupContext {
            id,
            owner: self.callback_sender().clone(),
            cancel: startup.cancel.restart(),
            origin,
        };
        self.debugger.sessions[id].startup = Some(startup);
        context.spawn(connect, move |editor, context, (client, incoming)| {
            let initialize = client.initialize(name, terminal_support, true);
            editor.debugger.sessions[id].client = Some(client);
            editor
                .debugger
                .incoming
                .push(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    incoming,
                ));
            context.spawn(
                initialize,
                move |editor, context, capabilities: DebuggerCapabilities| {
                    let initialized = editor.debugger.sessions[id]
                        .startup
                        .as_ref()
                        .is_some_and(|startup| startup.initialized);
                    let client = editor.debugger.get_client_mut(id).unwrap();
                    client.caps = Some(capabilities);
                    if let Some(launch) = launch {
                        let future: futures_util::future::BoxFuture<'static, dap::Result<Value>> =
                            match launch.request {
                                ConnectionType::Launch => Box::pin(client.launch(launch.arguments)),
                                ConnectionType::Attach => Box::pin(client.attach(launch.arguments)),
                            };
                        context.spawn(future, |editor, context, result| {
                            context.finish(editor, Ok(result))
                        });
                    } else {
                        context.finish(editor, Ok(serde_json::json!({ "success": true })));
                    }
                    if initialized {
                        editor.debugger_initialized(id);
                    }
                },
            );
        });
        id
    }

    pub(crate) fn debugger_initialized(&mut self, id: DebugAdapterId) {
        if self
            .debugger
            .get_client(id)
            .is_some_and(|client| client.caps.is_none())
        {
            if let Some(startup) = self
                .debugger
                .sessions
                .get_mut(id)
                .and_then(|session| session.startup.as_mut())
            {
                startup.initialized = true;
            }
            return;
        }
        self.reset_debugger_events(id);
        let active = self
            .debugger
            .get_active_session()
            .is_some_and(|session| session.id() == id);
        if active {
            self.set_status("Debugger initialized...");
        }
        if let Err(error) = self.configure_debugger_breakpoints(id) {
            if active {
                self.set_error(|| format!("Debugger configuration failed: {error}"));
            } else {
                log::warn!("Debugger configuration failed for {id}: {error}");
            }
        }
    }

    pub(crate) fn start_child_debugger(
        &mut self,
        parent: DebugAdapterId,
        sequence: u64,
        arguments: dap::requests::StartDebuggingArguments,
    ) -> dap::Result<()> {
        let client = self
            .debugger
            .get_client(parent)
            .context("No active debugger found")?;
        // Reconnect TCP adapters to their server; stdio children get their own process.
        let socket = client.socket;
        let config = self
            .debugger
            .get_session(parent)
            .unwrap()
            .adapter
            .clone()
            .context("No debugger configuration found")?;
        self.begin_debugger_startup(
            socket,
            config,
            Some(DebuggerLaunch {
                request: arguments.request,
                arguments: arguments.configuration,
            }),
            StartupOrigin::Child { parent, sequence },
        );
        Ok(())
    }
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let startups: Vec<_> = event.editor.debugger.sessions.iter().filter_map(|(id, session)| {
            let startup = session.startup.as_ref()?;
            matches!(startup.origin, StartupOrigin::Document(document) if document == event.doc.id()).then_some(id)
        }).collect();
        for id in startups {
            event.editor.debugger.remove_session(id);
        }
        Ok(())
    });
}
