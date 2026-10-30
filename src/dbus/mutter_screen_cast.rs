use std::collections::HashMap;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::zvariant::{DeserializeDict, OwnedObjectPath, SerializeDict, Type, Value};
use zbus::{fdo, interface, ObjectServer};

use super::{request_name, Start};
use crate::backend::IpcOutputMap;
use crate::utils::{CastSessionId, CastStreamId};

#[derive(Clone)]
pub struct ScreenCast {
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
}

#[derive(Clone)]
pub struct Session {
    id: CastSessionId,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    #[allow(clippy::type_complexity)]
    streams: Arc<Mutex<Vec<(Stream, InterfaceRef<Stream>)>>>,
    stopped: Arc<AtomicBool>,
}

#[derive(Debug, Default, Deserialize, Type, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    #[default]
    Hidden = 0,
    Embedded = 1,
    Metadata = 2,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordMonitorProperties {
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordWindowProperties {
    #[zvariant(rename = "window-id")]
    window_id: u64,
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Clone)]
pub struct Stream {
    id: CastStreamId,
    session_id: CastSessionId,
    target: StreamTarget,
    cursor_mode: CursorMode,
    was_started: Arc<AtomicBool>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
}

#[derive(Clone)]
enum StreamTarget {
    // FIXME: update on scale changes and whatnot.
    Output(niri_ipc::Output),
    Window { id: u64 },
}

#[derive(Debug, Clone)]
pub enum StreamTargetId {
    Output { name: String },
    Window { id: u64 },
}

#[derive(Debug, SerializeDict, Type, Value)]
#[zvariant(signature = "dict")]
struct StreamParameters {
    /// Position of the stream in logical coordinates.
    position: (i32, i32),
    /// Size of the stream in logical coordinates.
    size: (i32, i32),
}

pub enum ScreenCastToNiri {
    StartCast {
        session_id: CastSessionId,
        stream_id: CastStreamId,
        target: StreamTargetId,
        cursor_mode: CursorMode,
        signal_ctx: SignalEmitter<'static>,
    },
    StopCast {
        session_id: CastSessionId,
    },
}

#[interface(name = "org.gnome.Mutter.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        if properties.contains_key("remote-desktop-session-id") {
            return Err(fdo::Error::Failed(
                "there are no remote desktop sessions".to_owned(),
            ));
        }

        let session_id = CastSessionId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Session/u{}", session_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let session = Session::new(session_id, self.ipc_outputs.clone(), self.to_niri.clone());
        match server.at(&path, session).await {
            Ok(true) => (),
            Ok(false) => return Err(fdo::Error::Failed("session path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating session object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(property)]
    async fn version(&self) -> i32 {
        4
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Session")]
impl Session {
    async fn start(&self) {
        debug!("start");

        for (stream, iface) in &*self.streams.lock().unwrap() {
            stream.start(iface.signal_emitter().clone());
        }
    }

    pub async fn stop(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
    ) {
        debug!("stop");

        if self.stopped.swap(true, Ordering::SeqCst) {
            // Already stopped.
            return;
        }

        if let Err(err) = Session::closed(&ctxt).await {
            warn!("error sending Closed: {err:?}");
        }

        if let Err(err) = self.to_niri.send(ScreenCastToNiri::StopCast {
            session_id: self.id,
        }) {
            warn!("error sending StopCast to niri: {err:?}");
        }

        let streams = mem::take(&mut *self.streams.lock().unwrap());
        for (_, iface) in streams.iter() {
            if let Err(err) = server
                .remove::<Stream, _>(iface.signal_emitter().path())
                .await
            {
                warn!("error removing Stream: {err:?}");
            }
        }

        if let Err(err) = server.remove::<Session, _>(ctxt.path()).await {
            warn!("error removing Session: {err:?}");
        }
    }

    // Takes &self, so that zbus calls it with the interface lock held only for reading. With
    // &mut self, the write lock held while server.at() below waits for the object server's lock
    // deadlocks with anything that holds the object server's lock while it waits for this
    // interface's lock, like Niri::stop_cast() or a D-Bus Introspect call.
    async fn record_monitor(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        connector: &str,
        properties: RecordMonitorProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(connector, ?properties, "record_monitor");

        let output = {
            let ipc_outputs = self.ipc_outputs.lock().unwrap();
            ipc_outputs.values().find(|o| o.name == connector).cloned()
        };
        let Some(output) = output else {
            return Err(fdo::Error::Failed("no such monitor".to_owned()));
        };

        if output.logical.is_none() {
            return Err(fdo::Error::Failed("monitor is disabled".to_owned()));
        }

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Output(output);
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_niri.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.add_stream(server, stream, iface).await?;
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    // Takes &self for the same reason as record_monitor().
    async fn record_window(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        properties: RecordWindowProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(?properties, "record_window");

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Window {
            id: properties.window_id,
        };
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_niri.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.add_stream(server, stream, iface).await?;
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(signal)]
    async fn closed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Stream")]
impl Stream {
    #[zbus(signal)]
    pub async fn pipe_wire_stream_added(ctxt: &SignalEmitter<'_>, node_id: u32)
        -> zbus::Result<()>;

    #[zbus(property)]
    async fn parameters(&self) -> StreamParameters {
        match &self.target {
            StreamTarget::Output(output) => {
                let logical = output.logical.as_ref().unwrap();
                StreamParameters {
                    position: (logical.x, logical.y),
                    size: (logical.width as i32, logical.height as i32),
                }
            }
            StreamTarget::Window { .. } => {
                // Does any consumer need this?
                StreamParameters {
                    position: (0, 0),
                    size: (1, 1),
                }
            }
        }
    }
}

impl ScreenCast {
    pub fn new(
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    ) -> Self {
        Self {
            ipc_outputs,
            to_niri,
        }
    }
}

impl Start for ScreenCast {
    fn start(self, monitor: bool) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        conn.object_server()
            .at("/org/gnome/Mutter/ScreenCast", self)?;
        request_name(&conn, "org.gnome.Mutter.ScreenCast", monitor)?;

        Ok(conn)
    }
}

impl Session {
    pub fn new(
        id: CastSessionId,
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    ) -> Self {
        Self {
            id,
            ipc_outputs,
            streams: Arc::new(Mutex::new(vec![])),
            to_niri,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Adds a stream that was just put on the object server, unless the session has stopped.
    async fn add_stream(
        &self,
        server: &ObjectServer,
        stream: Stream,
        iface: InterfaceRef<Stream>,
    ) -> fdo::Result<()> {
        {
            let mut streams = self.streams.lock().unwrap();
            // stop() can run at the same time. It sets `stopped` before it takes the streams, so
            // checking `stopped` under the lock means that either stop() removes this stream or
            // this method does.
            if !self.stopped.load(Ordering::SeqCst) {
                streams.push((stream, iface));
                return Ok(());
            }
        }

        if let Err(err) = server
            .remove::<Stream, _>(iface.signal_emitter().path())
            .await
        {
            warn!("error removing Stream: {err:?}");
        }
        Err(fdo::Error::Failed("session is stopped".to_owned()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.to_niri.send(ScreenCastToNiri::StopCast {
            session_id: self.id,
        });
    }
}

impl Stream {
    fn new(
        id: CastStreamId,
        session_id: CastSessionId,
        target: StreamTarget,
        cursor_mode: CursorMode,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    ) -> Self {
        Self {
            id,
            session_id,
            target,
            cursor_mode,
            was_started: Arc::new(AtomicBool::new(false)),
            to_niri,
        }
    }

    fn start(&self, ctxt: SignalEmitter<'static>) {
        if self.was_started.load(Ordering::SeqCst) {
            return;
        }

        let msg = ScreenCastToNiri::StartCast {
            session_id: self.session_id,
            stream_id: self.id,
            target: self.target.make_id(),
            cursor_mode: self.cursor_mode,
            signal_ctx: ctxt,
        };

        if let Err(err) = self.to_niri.send(msg) {
            warn!("error sending StartCast to niri: {err:?}");
        }
    }
}

impl StreamTarget {
    fn make_id(&self) -> StreamTargetId {
        match self {
            StreamTarget::Output(output) => StreamTargetId::Output {
                name: output.name.clone(),
            },
            StreamTarget::Window { id } => StreamTargetId::Window { id: *id },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use zbus::blocking::connection::Builder;
    use zbus::Guid;

    use super::*;
    use crate::backend::OutputId;

    const SESSION_PATH: &str = "/org/gnome/Mutter/ScreenCast/Session/u1";

    #[test]
    fn session_stops_while_record_monitor_runs() {
        let output = niri_ipc::Output {
            name: String::from("DP-1"),
            make: String::from("Make"),
            model: String::from("Model"),
            serial: None,
            physical_size: None,
            modes: vec![],
            current_mode: None,
            is_custom_mode: false,
            vrr_supported: false,
            vrr_enabled: false,
            logical: Some(niri_ipc::LogicalOutput {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.,
                transform: niri_ipc::Transform::Normal,
            }),
            max_bpc: None,
        };
        let ipc_outputs = Arc::new(Mutex::new(HashMap::from([(OutputId::next(), output)])));
        let (to_niri, _from_session) = calloop::channel::channel();
        let session = Session::new(CastSessionId::next(), ipc_outputs.clone(), to_niri);

        // A peer-to-peer D-Bus connection with the session on the server side.
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            Builder::async_io_unix_stream(server_socket)
                .server(Guid::generate())
                .unwrap()
                .p2p()
                .build()
                .unwrap()
        });
        let client = Builder::async_io_unix_stream(client_socket)
            .p2p()
            .build()
            .unwrap();
        let server = server.join().unwrap();
        server.object_server().at(SESSION_PATH, session).unwrap();

        // Keep RecordMonitor waiting inside the method, where zbus holds the session's interface
        // lock.
        let outputs_guard = ipc_outputs.lock().unwrap();
        let record = {
            let client = client.clone();
            thread::spawn(move || {
                let properties = HashMap::<&str, Value>::new();
                client.call_method(
                    None::<&str>,
                    SESSION_PATH,
                    Some("org.gnome.Mutter.ScreenCast.Session"),
                    "RecordMonitor",
                    &("DP-1", properties),
                )
            })
        };
        thread::sleep(Duration::from_millis(100));

        // Stop the session like Niri::stop_cast() does. It used to wait for the session's
        // interface lock while holding the object server's lock, which RecordMonitor then waited
        // for.
        let (stopped_tx, stopped_rx) = mpsc::channel();
        {
            let server = server.clone();
            thread::spawn(move || {
                let server = server.object_server();
                let iface = server.interface::<_, Session>(SESSION_PATH).unwrap();
                async_io::block_on(async move {
                    iface
                        .get()
                        .stop(server.inner(), iface.signal_emitter().clone())
                        .await
                });
                stopped_tx.send(()).unwrap();
            });
        }
        stopped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("stopping the session should finish while RecordMonitor runs");
        drop(outputs_guard);

        // The session stopped before RecordMonitor added its stream, so RecordMonitor fails and
        // removes the stream.
        let reply = record.join().unwrap();
        assert!(reply.is_err(), "{reply:?}");

        let reply = client
            .call_method(
                None::<&str>,
                "/",
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            )
            .unwrap();
        let xml: String = reply.body().deserialize().unwrap();
        assert!(!xml.contains("<node name=\"u"), "{xml}");
    }
}
