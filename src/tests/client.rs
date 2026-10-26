use std::cmp::min;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};
use smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1};
use smithay::reexports::wayland_protocols::wp::single_pixel_buffer;
use smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::{self, XdgPositioner};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    self, ZwlrLayerShellV1,
};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::reexports::rustix::fs::{memfd_create, MemfdFlags};
use wayland_backend::client::Backend;
use wayland_client::globals::Global;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};

use crate::utils::id::IdCounter;

pub struct Client {
    pub id: ClientId,
    pub event_loop: EventLoop<'static, State>,
    pub connection: Connection,
    pub qh: QueueHandle<State>,
    pub display: WlDisplay,
    pub state: State,
}

pub struct State {
    pub qh: QueueHandle<State>,

    pub globals: Vec<Global>,
    pub outputs: HashMap<WlOutput, String>,
    pub surfaces: HashMap<WlSurface, Surface>,
    pub seats: HashMap<WlSeat, Seat>,

    pub compositor: Option<WlCompositor>,
    pub subcompositor: Option<WlSubcompositor>,
    pub xdg_wm_base: Option<XdgWmBase>,
    pub layer_shell: Option<ZwlrLayerShellV1>,
    pub virtual_pointer_manager: Option<ZwlrVirtualPointerManagerV1>,
    pub spbm: Option<WpSinglePixelBufferManagerV1>,
    pub shm: Option<WlShm>,
    pub viewporter: Option<WpViewporter>,
    pub ksim: Option<ZwpKeyboardShortcutsInhibitManagerV1>,
    pub foreign_toplevel_list: Option<ExtForeignToplevelListV1>,

    pub windows: Vec<Window>,
    pub layers: Vec<LayerSurface>,
    pub foreign_toplevels: Vec<ForeignToplevel>,
}

/// A toplevel announced through ext-foreign-toplevel-list.
#[derive(Debug)]
pub struct ForeignToplevel {
    pub handle: ExtForeignToplevelHandleV1,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub closed: bool,
}

pub struct Window {
    pub qh: QueueHandle<State>,
    pub spbm: WpSinglePixelBufferManagerV1,

    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub xdg_toplevel: XdgToplevel,
    pub viewport: WpViewport,
    pub pending_configure: Configure,
    pub configures_received: Vec<(u32, Configure)>,
    pub close_requested: bool,

    pub configures_looked_at: usize,
}

pub struct LayerSurface {
    pub qh: QueueHandle<State>,
    pub spbm: WpSinglePixelBufferManagerV1,

    pub surface: WlSurface,
    pub layer_surface: ZwlrLayerSurfaceV1,
    pub viewport: WpViewport,
    pub configures_received: Vec<(u32, LayerConfigure)>,
    pub close_requested: bool,

    pub configures_looked_at: usize,
}

#[derive(Debug, Default)]
pub struct Seat {
    pub keyboard: Option<Keyboard>,
}

#[derive(Debug)]
pub struct Surface {
    pub keyboard_events_received: Vec<KeyboardEvent>,
    pub keyboard_events_looked_at: usize,
    /// Outputs from `wl_surface.enter` without a matching `wl_surface.leave`.
    pub entered_outputs: Vec<WlOutput>,
}

/// User data of a popup's `xdg_surface`, whose configures are acked right away.
pub struct PopupXdgSurface;

#[derive(Debug)]
pub struct Keyboard {
    pub proxy: WlKeyboard,
    pub surface: Option<WlSurface>,
}

#[derive(Debug)]
pub enum KeyboardEvent {
    Enter {
        keys: Vec<u8>,
    },
    Leave,
    Key {
        key: u32,
        state: wl_keyboard::KeyState,
    },
    Modifiers {
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Configure {
    pub size: (i32, i32),
    pub bounds: Option<(i32, i32)>,
    pub states: Vec<xdg_toplevel::State>,
}

#[derive(Debug, Clone, Copy)]
pub struct LayerConfigure {
    pub size: (u32, u32),
}

#[derive(Clone, Copy, Default)]
pub struct LayerMargin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Clone, Copy, Default)]
pub struct LayerConfigureProps {
    pub size: Option<(u32, u32)>,
    pub anchor: Option<zwlr_layer_surface_v1::Anchor>,
    pub exclusive_zone: Option<i32>,
    pub margin: Option<LayerMargin>,
    pub kb_interactivity: Option<zwlr_layer_surface_v1::KeyboardInteractivity>,
    pub layer: Option<zwlr_layer_shell_v1::Layer>,
    pub exclusive_edge: Option<zwlr_layer_surface_v1::Anchor>,
}

#[derive(Default)]
pub struct SyncData {
    pub done: AtomicBool,
}

static CLIENT_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(u64);

impl ClientId {
    fn next() -> ClientId {
        ClientId(CLIENT_ID_COUNTER.next())
    }
}

impl fmt::Display for KeyboardEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyboardEvent::Enter { keys } => {
                write!(f, "enter: {keys:?}")?;
            }
            KeyboardEvent::Leave => {
                write!(f, "leave")?;
            }
            KeyboardEvent::Key { key, state } => {
                let action = match state {
                    wl_keyboard::KeyState::Released => "released",
                    wl_keyboard::KeyState::Pressed => "pressed",
                    wl_keyboard::KeyState::Repeated => "repeated",
                    _ => unreachable!(),
                };
                write!(f, "key {action}: {key}")?;
            }
            KeyboardEvent::Modifiers {
                depressed,
                latched,
                locked,
                group,
            } => {
                write!(
                    f,
                    "modifiers: depressed={depressed}, latched={latched}, \
                     locked={locked}, group={group}"
                )?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Configure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "size: {} × {}, ", self.size.0, self.size.1)?;
        if let Some(bounds) = self.bounds {
            write!(f, "bounds: {} × {}, ", bounds.0, bounds.1)?;
        } else {
            write!(f, "bounds: none, ")?;
        }
        write!(f, "states: {:?}", self.states)?;
        Ok(())
    }
}

impl fmt::Display for LayerConfigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "size: {} × {}", self.size.0, self.size.1)?;
        Ok(())
    }
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        let id = ClientId::next();

        let event_loop = EventLoop::try_new().unwrap();
        let backend = Backend::connect(stream).unwrap();
        let connection = Connection::from_backend(backend);
        let queue = connection.new_event_queue();
        let qh = queue.handle();
        WaylandSource::new(connection.clone(), queue)
            .insert(event_loop.handle())
            .unwrap();

        let display = connection.display();
        let _registry = display.get_registry(&qh, ());
        connection.flush().unwrap();

        let state = State {
            qh: qh.clone(),
            globals: Vec::new(),
            outputs: HashMap::new(),
            surfaces: HashMap::new(),
            seats: HashMap::new(),
            compositor: None,
            subcompositor: None,
            xdg_wm_base: None,
            layer_shell: None,
            virtual_pointer_manager: None,
            spbm: None,
            shm: None,
            viewporter: None,
            ksim: None,
            foreign_toplevel_list: None,
            windows: Vec::new(),
            layers: Vec::new(),
            foreign_toplevels: Vec::new(),
        };

        Self {
            id,
            event_loop,
            connection,
            qh,
            display,
            state,
        }
    }

    pub fn dispatch(&mut self) {
        self.event_loop
            .dispatch(Duration::ZERO, &mut self.state)
            .unwrap();

        if let Some(error) = self.connection.protocol_error() {
            panic!("{error}");
        }
    }

    pub fn send_sync(&self) -> Arc<SyncData> {
        let data = Arc::new(SyncData::default());
        self.display.sync(&self.qh, data.clone());
        self.connection.flush().unwrap();
        data
    }

    pub fn create_window(&mut self) -> &mut Window {
        self.state.create_window()
    }

    pub fn window(&mut self, surface: &WlSurface) -> &mut Window {
        self.state.window(surface)
    }

    pub fn create_layer(
        &mut self,
        output: Option<&WlOutput>,
        layer: zwlr_layer_shell_v1::Layer,
        namespace: &str,
    ) -> &mut LayerSurface {
        self.state.create_layer(output, layer, namespace.to_owned())
    }

    pub fn layer(&mut self, surface: &WlSurface) -> &mut LayerSurface {
        self.state.layer(surface)
    }

    pub fn output(&mut self, name: &str) -> WlOutput {
        self.state
            .outputs
            .iter()
            .find(|(_, v)| *v == name)
            .unwrap()
            .0
            .clone()
    }
}

impl State {
    pub fn create_surface(&mut self) -> WlSurface {
        let compositor = self.compositor.as_ref().unwrap();
        let surface = compositor.create_surface(&self.qh, ());
        self.surfaces.insert(
            surface.clone(),
            Surface {
                keyboard_events_received: Vec::new(),
                keyboard_events_looked_at: 0,
                entered_outputs: Vec::new(),
            },
        );
        surface
    }

    /// Attaches a 1×1 buffer to the surface, without committing.
    pub fn attach_pixel_buffer(&self, surface: &WlSurface) {
        let spbm = self.spbm.as_ref().unwrap();
        let buffer = spbm.create_u32_rgba_buffer(0, 0, 0, u32::MAX, &self.qh, ());
        surface.attach(Some(&buffer), 0, 0);
    }

    /// Attaches an opaque `width` × `height` buffer of one RGB color, without committing.
    ///
    /// Unlike single-pixel buffers, these are uploaded to a texture, so they show up in snapshots.
    pub fn attach_shm_buffer(&self, surface: &WlSurface, width: i32, height: i32, rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        let stride = width * 4;
        let fd = memfd_create("nori-test-buffer", MemfdFlags::CLOEXEC).unwrap();
        let mut file = File::from(fd);
        // Xrgb8888 is stored as B, G, R, X.
        let pixels = [b, g, r, 255].repeat((width * height) as usize);
        file.write_all(&pixels).unwrap();

        let shm = self.shm.as_ref().unwrap();
        let pool = shm.create_pool(file.as_fd(), stride * height, &self.qh, ());
        let format = wl_shm::Format::Xrgb8888;
        let buffer = pool.create_buffer(0, width, height, stride, format, &self.qh, ());
        pool.destroy();

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
    }

    /// Creates a synchronized subsurface with a buffer committed; it appears on the parent's next
    /// commit.
    pub fn create_subsurface(&mut self, parent: &WlSurface) -> (WlSurface, WlSubsurface) {
        let surface = self.create_surface();
        let subcompositor = self.subcompositor.as_ref().unwrap();
        let subsurface = subcompositor.get_subsurface(&surface, parent, &self.qh, ());
        self.attach_pixel_buffer(&surface);
        surface.commit();
        (surface, subsurface)
    }

    /// Creates a 10×10 popup at the parent's top-left corner, placed towards `gravity`, and sends
    /// its initial commit; it maps when committed with a buffer after the configure.
    pub fn create_popup(
        &mut self,
        parent: &XdgSurface,
        gravity: xdg_positioner::Gravity,
    ) -> (WlSurface, XdgSurface, XdgPopup) {
        let surface = self.create_surface();
        let xdg_wm_base = self.xdg_wm_base.as_ref().unwrap();
        let positioner = xdg_wm_base.create_positioner(&self.qh, ());
        positioner.set_size(10, 10);
        positioner.set_anchor_rect(0, 0, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(gravity);
        let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &self.qh, PopupXdgSurface);
        let popup = xdg_surface.get_popup(Some(parent), &positioner, &self.qh, ());
        positioner.destroy();
        surface.commit();
        (surface, xdg_surface, popup)
    }

    pub fn create_window(&mut self) -> &mut Window {
        let surface = self.create_surface();

        let xdg_wm_base = self.xdg_wm_base.as_ref().unwrap();
        let viewporter = self.viewporter.as_ref().unwrap();

        let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &self.qh, ());
        let xdg_toplevel = xdg_surface.get_toplevel(&self.qh, ());
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());

        let window = Window {
            qh: self.qh.clone(),
            spbm: self.spbm.clone().unwrap(),

            surface,
            xdg_surface,
            xdg_toplevel,
            viewport,
            pending_configure: Configure::default(),
            configures_received: Vec::new(),
            close_requested: false,

            configures_looked_at: 0,
        };

        self.windows.push(window);
        self.windows.last_mut().unwrap()
    }

    pub fn window(&mut self, surface: &WlSurface) -> &mut Window {
        self.windows
            .iter_mut()
            .find(|w| w.surface == *surface)
            .unwrap()
    }

    pub fn create_layer(
        &mut self,
        output: Option<&WlOutput>,
        layer: zwlr_layer_shell_v1::Layer,
        namespace: String,
    ) -> &mut LayerSurface {
        let surface = self.create_surface();

        let layer_shell = self.layer_shell.as_ref().unwrap();
        let viewporter = self.viewporter.as_ref().unwrap();

        let layer_surface =
            layer_shell.get_layer_surface(&surface, output, layer, namespace, &self.qh, ());
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());

        let layer_surface = LayerSurface {
            qh: self.qh.clone(),
            spbm: self.spbm.clone().unwrap(),

            surface,
            layer_surface,
            viewport,
            configures_received: Vec::new(),
            close_requested: false,

            configures_looked_at: 0,
        };

        self.layers.push(layer_surface);
        self.layers.last_mut().unwrap()
    }

    pub fn layer(&mut self, surface: &WlSurface) -> &mut LayerSurface {
        self.layers
            .iter_mut()
            .find(|w| w.surface == *surface)
            .unwrap()
    }

    pub fn recent_keyboard_events(
        &mut self,
        surface: &WlSurface,
    ) -> impl Iterator<Item = &KeyboardEvent> + '_ {
        let surface = self.surfaces.get_mut(surface).unwrap();
        surface.recent_keyboard_events()
    }

    pub fn inhibit_shortcuts(&self, surface: &WlSurface) -> ZwpKeyboardShortcutsInhibitorV1 {
        let ksim = self.ksim.as_ref().unwrap();
        let seat = self.seats.keys().next().unwrap();
        ksim.inhibit_shortcuts(surface, seat, &self.qh, ())
    }
}

impl Window {
    pub fn commit(&self) {
        self.surface.commit();
    }

    pub fn ack_last(&self) {
        let serial = self.configures_received.last().unwrap().0;
        self.xdg_surface.ack_configure(serial);
    }

    pub fn ack_last_and_commit(&self) {
        self.ack_last();
        self.commit();
    }

    pub fn attach_new_buffer(&self) {
        let buffer = self.spbm.create_u32_rgba_buffer(0, 0, 0, 0, &self.qh, ());
        self.surface.attach(Some(&buffer), 0, 0);
    }

    pub fn attach_null(&self) {
        self.surface.attach(None, 0, 0);
    }

    pub fn set_size(&self, w: u16, h: u16) {
        self.viewport.set_destination(i32::from(w), i32::from(h));
    }

    pub fn set_fullscreen(&self, output: Option<&WlOutput>) {
        self.xdg_toplevel.set_fullscreen(output);
    }

    pub fn unset_fullscreen(&self) {
        self.xdg_toplevel.unset_fullscreen();
    }

    pub fn set_maximized(&self) {
        self.xdg_toplevel.set_maximized();
    }

    pub fn unset_maximized(&self) {
        self.xdg_toplevel.unset_maximized();
    }

    pub fn set_parent(&self, parent: Option<&XdgToplevel>) {
        self.xdg_toplevel.set_parent(parent);
    }

    pub fn set_title(&self, title: &str) {
        self.xdg_toplevel.set_title(title.to_owned());
    }

    pub fn recent_configures(&mut self) -> impl Iterator<Item = &Configure> {
        let start = self.configures_looked_at;
        self.configures_looked_at = self.configures_received.len();
        self.configures_received[start..].iter().map(|(_, c)| c)
    }

    pub fn format_recent_configures(&mut self) -> String {
        let mut buf = String::new();
        for configure in self.recent_configures() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            write!(buf, "{configure}").unwrap();
        }
        buf
    }
}

impl LayerSurface {
    pub fn commit(&self) {
        self.surface.commit();
    }

    pub fn ack_last(&self) {
        let serial = self.configures_received.last().unwrap().0;
        self.layer_surface.ack_configure(serial);
    }

    pub fn ack_last_and_commit(&self) {
        self.ack_last();
        self.commit();
    }

    pub fn set_configure_props(&self, props: LayerConfigureProps) {
        let LayerConfigureProps {
            size,
            anchor,
            exclusive_zone,
            margin,
            kb_interactivity,
            layer,
            exclusive_edge,
        } = props;

        if let Some(x) = size {
            self.layer_surface.set_size(x.0, x.1);
        }
        if let Some(x) = anchor {
            self.layer_surface.set_anchor(x);
        }
        if let Some(x) = exclusive_zone {
            self.layer_surface.set_exclusive_zone(x);
        }
        if let Some(x) = margin {
            self.layer_surface
                .set_margin(x.top, x.right, x.bottom, x.left);
        }
        if let Some(x) = kb_interactivity {
            self.layer_surface.set_keyboard_interactivity(x);
        }
        if let Some(x) = layer {
            self.layer_surface.set_layer(x);
        }
        if let Some(x) = exclusive_edge {
            self.layer_surface.set_exclusive_edge(x);
        }
    }

    pub fn attach_new_buffer(&self) {
        let buffer = self.spbm.create_u32_rgba_buffer(0, 0, 0, 0, &self.qh, ());
        self.surface.attach(Some(&buffer), 0, 0);
    }

    pub fn attach_null(&self) {
        self.surface.attach(None, 0, 0);
    }

    pub fn set_size(&self, w: u16, h: u16) {
        self.viewport.set_destination(i32::from(w), i32::from(h));
    }

    pub fn recent_configures(&mut self) -> impl Iterator<Item = &LayerConfigure> {
        let start = self.configures_looked_at;
        self.configures_looked_at = self.configures_received.len();
        self.configures_received[start..].iter().map(|(_, c)| c)
    }

    pub fn format_recent_configures(&mut self) -> String {
        let mut buf = String::new();
        for configure in self.recent_configures() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            write!(buf, "{configure}").unwrap();
        }
        buf
    }
}

impl Surface {
    pub fn recent_keyboard_events(&mut self) -> impl Iterator<Item = &KeyboardEvent> {
        let start = self.keyboard_events_looked_at;
        self.keyboard_events_looked_at = self.keyboard_events_received.len();
        self.keyboard_events_received[start..].iter()
    }
}

impl Dispatch<WlCallback, Arc<SyncData>> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlCallback,
        event: <WlCallback as wayland_client::Proxy>::Event,
        data: &Arc<SyncData>,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_callback::Event::Done { .. } => data.done.store(true, Ordering::Relaxed),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == WlCompositor::interface().name {
                    let version = min(version, WlCompositor::interface().version);
                    state.compositor = Some(registry.bind(name, version, qh, ()));
                } else if interface == XdgWmBase::interface().name {
                    let version = min(version, XdgWmBase::interface().version);
                    state.xdg_wm_base = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwlrLayerShellV1::interface().name {
                    let version = min(version, ZwlrLayerShellV1::interface().version);
                    state.layer_shell = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwlrVirtualPointerManagerV1::interface().name {
                    let version = min(version, ZwlrVirtualPointerManagerV1::interface().version);
                    state.virtual_pointer_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == WpSinglePixelBufferManagerV1::interface().name {
                    let version = min(version, WpSinglePixelBufferManagerV1::interface().version);
                    state.spbm = Some(registry.bind(name, version, qh, ()));
                } else if interface == WpViewporter::interface().name {
                    let version = min(version, WpViewporter::interface().version);
                    state.viewporter = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlOutput::interface().name {
                    let version = min(version, WlOutput::interface().version);
                    let output = registry.bind(name, version, qh, ());
                    state.outputs.insert(output, String::new());
                } else if interface == WlShm::interface().name {
                    let version = min(version, WlShm::interface().version);
                    state.shm = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlSubcompositor::interface().name {
                    let version = min(version, WlSubcompositor::interface().version);
                    state.subcompositor = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlSeat::interface().name {
                    let version = min(version, WlSeat::interface().version);
                    state
                        .seats
                        .insert(registry.bind(name, version, qh, ()), Seat::default());
                } else if interface == ZwpKeyboardShortcutsInhibitManagerV1::interface().name {
                    let version = min(
                        version,
                        ZwpKeyboardShortcutsInhibitManagerV1::interface().version,
                    );
                    state.ksim = Some(registry.bind(name, version, qh, ()));
                } else if interface == ExtForeignToplevelListV1::interface().name {
                    let version = min(version, ExtForeignToplevelListV1::interface().version);
                    state.foreign_toplevel_list = Some(registry.bind(name, version, qh, ()));
                }

                let global = Global {
                    name,
                    interface,
                    version,
                };
                state.globals.push(global);
            }
            wl_registry::Event::GlobalRemove { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Geometry { .. } => (),
            wl_output::Event::Mode { .. } => (),
            wl_output::Event::Done => (),
            wl_output::Event::Scale { .. } => (),
            wl_output::Event::Name { name } => {
                *state.outputs.get_mut(output).unwrap() = name;
            }
            wl_output::Event::Description { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let capabilities = capabilities.into_result().unwrap();
                let data = state.seats.get_mut(seat).unwrap();

                if capabilities.contains(wl_seat::Capability::Keyboard) {
                    if data.keyboard.is_none() {
                        let keyboard = Keyboard {
                            proxy: seat.get_keyboard(qh, seat.clone()),
                            surface: None,
                        };
                        data.keyboard = Some(keyboard);
                    }
                } else {
                    if let Some(keyboard) = data.keyboard.take() {
                        keyboard.proxy.release();
                    }
                }
            }
            wl_seat::Event::Name { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlKeyboard, WlSeat> for State {
    fn event(
        state: &mut Self,
        _keyboard: &WlKeyboard,
        event: <WlKeyboard as wayland_client::Proxy>::Event,
        seat: &WlSeat,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let seat = state.seats.get_mut(seat).unwrap();
        let keyboard = seat.keyboard.as_mut().unwrap();

        match event {
            wl_keyboard::Event::Keymap { .. } => (),
            wl_keyboard::Event::Enter { surface, keys, .. } => {
                keyboard.surface = Some(surface.clone());

                let event = KeyboardEvent::Enter { keys };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Leave { .. } => {
                let surface = keyboard.surface.take().unwrap();

                let event = KeyboardEvent::Leave;
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let key_state = key_state.into_result().unwrap();
                let surface = keyboard.surface.clone().unwrap();

                let event = KeyboardEvent::Key {
                    key,
                    state: key_state,
                };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let surface = keyboard.surface.clone().unwrap();

                let event = KeyboardEvent::Modifiers {
                    depressed: mods_depressed,
                    latched: mods_latched,
                    locked: mods_locked,
                    group,
                };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::RepeatInfo { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlCompositor, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _state: &mut Self,
        xdg_wm_base: &XdgWmBase,
        event: <XdgWmBase as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_wm_base::Event::Ping { serial } => {
                xdg_wm_base.pong(serial);
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

wayland_client::delegate_noop!(State: ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore WlSubcompositor);
wayland_client::delegate_noop!(State: ignore WlSubsurface);
wayland_client::delegate_noop!(State: ignore XdgPositioner);
wayland_client::delegate_noop!(State: ZwlrVirtualPointerV1);

impl Dispatch<WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some(surface) = state.surfaces.get_mut(surface) {
                    surface.entered_outputs.push(output);
                }
            }
            wl_surface::Event::Leave { output } => {
                if let Some(surface) = state.surfaces.get_mut(surface) {
                    surface.entered_outputs.retain(|o| *o != output);
                }
            }
            wl_surface::Event::PreferredBufferScale { .. } => (),
            wl_surface::Event::PreferredBufferTransform { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: <XdgSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_surface::Event::Configure { serial } => {
                let window = state
                    .windows
                    .iter_mut()
                    .find(|w| w.xdg_surface == *xdg_surface)
                    .unwrap();
                let configure = window.pending_configure.clone();
                window.configures_received.push((serial, configure));
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        xdg_toplevel: &XdgToplevel,
        event: <XdgToplevel as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let window = state
            .windows
            .iter_mut()
            .find(|w| w.xdg_toplevel == *xdg_toplevel)
            .unwrap();

        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let configure = &mut window.pending_configure;
                configure.size = (width, height);
                configure.states = states
                    .chunks_exact(4)
                    .flat_map(TryInto::<[u8; 4]>::try_into)
                    .map(u32::from_ne_bytes)
                    .flat_map(xdg_toplevel::State::try_from)
                    .collect();
            }
            xdg_toplevel::Event::Close => {
                window.close_requested = true;
            }
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                window.pending_configure.bounds = Some((width, height));
            }
            xdg_toplevel::Event::WmCapabilities { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let layer_surface = state
            .layers
            .iter_mut()
            .find(|w| w.layer_surface == *layer_surface)
            .unwrap();

        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                let configure = LayerConfigure {
                    size: (width, height),
                };
                layer_surface.configures_received.push((serial, configure));
            }
            zwlr_layer_surface_v1::Event::Closed => layer_surface.close_requested = true,
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_buffer::Event::Release => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        event: <WlShm as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_shm::Event::Format { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<WpSinglePixelBufferManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpSinglePixelBufferManagerV1,
        _event: <WpSinglePixelBufferManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<WpViewporter, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<WpViewport, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpKeyboardShortcutsInhibitManagerV1,
        _event: <ZwpKeyboardShortcutsInhibitManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpKeyboardShortcutsInhibitorV1,
        event: <ZwpKeyboardShortcutsInhibitorV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => (),
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgSurface, PopupXdgSurface> for State {
    fn event(
        _state: &mut Self,
        xdg_surface: &XdgSurface,
        event: <XdgSurface as wayland_client::Proxy>::Event,
        _data: &PopupXdgSurface,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_surface::Event::Configure { serial } => xdg_surface.ack_configure(serial),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgPopup, ()> for State {
    fn event(
        _state: &mut Self,
        _popup: &XdgPopup,
        event: <XdgPopup as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure { .. } => (),
            xdg_popup::Event::PopupDone => (),
            xdg_popup::Event::Repositioned { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtForeignToplevelListV1,
        event: <ExtForeignToplevelListV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                state.foreign_toplevels.push(ForeignToplevel {
                    handle: toplevel,
                    title: None,
                    app_id: None,
                    closed: false,
                });
            }
            ext_foreign_toplevel_list_v1::Event::Finished => (),
            _ => unreachable!(),
        }
    }

    wayland_client::event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: <ExtForeignToplevelHandleV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let toplevel = state
            .foreign_toplevels
            .iter_mut()
            .find(|t| t.handle == *handle)
            .unwrap();

        match event {
            ext_foreign_toplevel_handle_v1::Event::Closed => toplevel.closed = true,
            ext_foreign_toplevel_handle_v1::Event::Done => (),
            ext_foreign_toplevel_handle_v1::Event::Title { title } => toplevel.title = Some(title),
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                toplevel.app_id = Some(app_id)
            }
            ext_foreign_toplevel_handle_v1::Event::Identifier { .. } => (),
            _ => unreachable!(),
        }
    }
}
