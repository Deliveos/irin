use std::collections::HashMap;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::thread;

use smithay::output::Output;
use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_protocols_wlr;
use smithay::reexports::wayland_server::backend::{ClientId, ObjectId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::{Dispatch2, GlobalDispatch2};
use wayland_protocols_wlr::gamma_control::v1::server::{
    zwlr_gamma_control_manager_v1, zwlr_gamma_control_v1,
};
use zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use zwlr_gamma_control_v1::ZwlrGammaControlV1;

use crate::protocols::EmptyData;

const VERSION: u32 = 1;

/// Most gamma ramps that can be read on threads at once.
///
/// Reading can wait for a long time on a FUSE or network file system, and each read holds a
/// thread until it finishes.
const MAX_READS_IN_FLIGHT: usize = 16;

pub struct GammaControlManagerState {
    // Active gamma controls only. Failed ones are removed.
    gamma_controls: HashMap<Output, ZwlrGammaControlV1>,
    // Gamma controls with a read in progress, with the file of a later set_gamma to read next.
    reads: HashMap<ObjectId, Option<OwnedFd>>,
    // Reads in progress, including those of gamma controls that are gone.
    reads_in_flight: usize,
    // Sends gamma ramps read on threads to the event loop.
    read_sender: Sender<GammaRead>,
}

pub struct GammaControlManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait GammaControlHandler {
    fn gamma_control_manager_state(&mut self) -> &mut GammaControlManagerState;
    fn get_gamma_size(&mut self, output: &Output) -> Option<u32>;
    fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> Option<()>;
}

pub struct GammaControlState {
    gamma_size: u32,
}

/// A gamma ramp read on a thread.
struct GammaRead {
    gamma_control: ObjectId,
    ramp: Result<Vec<u16>, String>,
}

impl GammaControlManagerState {
    pub fn new<D, F>(
        display: &DisplayHandle,
        event_loop: &LoopHandle<'static, D>,
        filter: F,
    ) -> Self
    where
        D: GlobalDispatch<ZwlrGammaControlManagerV1, GammaControlManagerGlobalData>,
        D: GammaControlHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = GammaControlManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ZwlrGammaControlManagerV1, _>(VERSION, global_data);

        let (read_sender, read_receiver) = channel::channel();
        event_loop
            .insert_source(read_receiver, |event, _, state| {
                if let Event::Msg(read) = event {
                    finish_read(state, read);
                }
            })
            .unwrap();

        Self {
            gamma_controls: HashMap::new(),
            reads: HashMap::new(),
            reads_in_flight: 0,
            read_sender,
        }
    }

    pub fn output_removed(&mut self, output: &Output) {
        if let Some(gamma_control) = self.gamma_controls.remove(output) {
            gamma_control.failed();
            self.reads.remove(&gamma_control.id());
        }
    }
}

impl<D> GlobalDispatch2<ZwlrGammaControlManagerV1, D> for GammaControlManagerGlobalData
where
    D: Dispatch<ZwlrGammaControlManagerV1, EmptyData>,
    D: GammaControlHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrGammaControlManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, EmptyData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ZwlrGammaControlManagerV1, D> for EmptyData
where
    D: Dispatch<ZwlrGammaControlV1, GammaControlState>,
    D: GammaControlHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: <ZwlrGammaControlManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } => {
                if let Some(output) = Output::from_resource(&output) {
                    // We borrow state in the middle.
                    #[allow(clippy::map_entry)]
                    if !state
                        .gamma_control_manager_state()
                        .gamma_controls
                        .contains_key(&output)
                    {
                        if let Some(gamma_size) = state.get_gamma_size(&output) {
                            let zwlr_gamma_control =
                                data_init.init(id, GammaControlState { gamma_size });
                            zwlr_gamma_control.gamma_size(gamma_size);
                            state
                                .gamma_control_manager_state()
                                .gamma_controls
                                .insert(output, zwlr_gamma_control);
                            return;
                        }
                    }
                }

                data_init
                    .init(id, GammaControlState { gamma_size: 0 })
                    .failed();
            }
            zwlr_gamma_control_manager_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch2<ZwlrGammaControlV1, D> for GammaControlState
where
    D: GammaControlHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: <ZwlrGammaControlV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwlr_gamma_control_v1::Request::SetGamma { fd } => {
                let manager = state.gamma_control_manager_state();
                let Some((output, _)) = manager.gamma_controls.iter().find(|(_, x)| *x == resource)
                else {
                    return;
                };
                let output = output.clone();

                trace!("setting gamma for output {}", output.name());

                if let Some(next) = manager.reads.get_mut(&resource.id()) {
                    // Read this ramp when the current read finishes, instead of any earlier one
                    // that waits.
                    *next = Some(fd);
                    return;
                }

                start_read(state, resource, &output, self.gamma_size, fd);
            }
            zwlr_gamma_control_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrGammaControlV1) {
        let manager = state.gamma_control_manager_state();
        manager.reads.remove(&resource.id());
        let gamma_controls = &mut manager.gamma_controls;
        let Some((output, _)) = gamma_controls.iter().find(|(_, x)| *x == resource) else {
            return;
        };
        let output = output.clone();
        gamma_controls.remove(&output);

        let _ = state.set_gamma(&output, None);
    }
}

/// Reads a gamma ramp with `gamma_size` entries for each of red, green and blue.
fn read_gamma(file: File, gamma_size: u32) -> Result<Vec<u16>, String> {
    let _span = tracy_client::span!("read gamma from fd");

    // Start with a u16 slice so it's aligned correctly.
    let mut gamma = vec![0u16; gamma_size as usize * 3];
    let buf = bytemuck::cast_slice_mut(&mut gamma);

    // pread fails on pipes instead of waiting for a client that never writes, and doesn't depend
    // on the client's file offset.
    if let Err(err) = file.read_exact_at(buf, 0) {
        return Err(format!("failed to read gamma data: {err:?}"));
    }

    // Verify that there's no more data.
    match file.read_at(&mut [0], buf.len() as u64) {
        Ok(0) => Ok(gamma),
        Ok(_) => Err(String::from("gamma data is too large")),
        Err(err) => Err(format!("error reading gamma data: {err:?}")),
    }
}

/// Starts reading a gamma ramp on a thread.
///
/// Reading on the main thread would freeze the compositor while pread waits on a FUSE or network
/// file system. The thread also closes the file, which can wait as well.
fn start_read<D: GammaControlHandler>(
    state: &mut D,
    gamma_control: &ZwlrGammaControlV1,
    output: &Output,
    gamma_size: u32,
    fd: OwnedFd,
) {
    let manager = state.gamma_control_manager_state();
    if manager.reads_in_flight >= MAX_READS_IN_FLIGHT {
        warn!("too many gamma ramps are being read");
        fail(state, output);
        return;
    }

    let sender = manager.read_sender.clone();
    let id = gamma_control.id();
    let res = thread::Builder::new()
        .name("Gamma Reader".to_owned())
        .spawn(move || {
            let read = GammaRead {
                gamma_control: id,
                ramp: read_gamma(File::from(fd), gamma_size),
            };
            if let Err(err) = sender.send(read) {
                warn!("error sending gamma ramp to the event loop: {err:?}");
            }
        });

    match res {
        Ok(_) => {
            manager.reads_in_flight += 1;
            manager.reads.insert(gamma_control.id(), None);
        }
        Err(err) => {
            warn!("error spawning a thread to read gamma data: {err:?}");
            fail(state, output);
        }
    }
}

/// Applies a gamma ramp read on a thread, unless a later set_gamma replaced it.
fn finish_read<D: GammaControlHandler>(state: &mut D, read: GammaRead) {
    let manager = state.gamma_control_manager_state();
    manager.reads_in_flight -= 1;

    // The gamma control could have failed or been destroyed meanwhile.
    let Some(next) = manager.reads.remove(&read.gamma_control) else {
        return;
    };
    let Some((output, gamma_control)) = manager
        .gamma_controls
        .iter()
        .find(|(_, x)| x.id() == read.gamma_control)
    else {
        return;
    };
    let output = output.clone();
    let gamma_control = gamma_control.clone();

    if let Some(fd) = next {
        // A later set_gamma came in meanwhile, read its ramp instead.
        if let Some(data) = gamma_control.data::<GammaControlState>() {
            start_read(state, &gamma_control, &output, data.gamma_size, fd);
        }
        return;
    }

    match read.ramp {
        Ok(ramp) => {
            if state.set_gamma(&output, Some(ramp)).is_none() {
                fail(state, &output);
            }
        }
        Err(err) => {
            warn!("{err}");
            fail(state, &output);
        }
    }
}

/// Sends failed to the output's gamma control, removes it, and resets the output's gamma.
fn fail<D: GammaControlHandler>(state: &mut D, output: &Output) {
    let manager = state.gamma_control_manager_state();
    if let Some(gamma_control) = manager.gamma_controls.remove(output) {
        gamma_control.failed();
        manager.reads.remove(&gamma_control.id());
    }
    let _ = state.set_gamma(output, None);
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    use smithay::reexports::rustix::fs::{memfd_create, MemfdFlags};

    use super::*;

    fn gamma_file(bytes: &[u8]) -> File {
        let fd = memfd_create("nori-test-gamma", MemfdFlags::CLOEXEC).unwrap();
        let mut file = File::from(fd);
        file.write_all(bytes).unwrap();
        file
    }

    #[test]
    fn read_gamma_checks_the_size() {
        let ramp: Vec<u16> = (0..6).collect();
        let bytes: &[u8] = bytemuck::cast_slice(&ramp);

        // The file offset is at the end after writing; reading starts at 0 anyway.
        assert_eq!(read_gamma(gamma_file(bytes), 2), Ok(ramp.clone()));

        assert!(read_gamma(gamma_file(&bytes[..11]), 2).is_err());
        assert!(read_gamma(gamma_file(&[bytes, &[0]].concat()), 2).is_err());
    }

    #[test]
    fn read_gamma_fails_on_a_socket_instead_of_waiting() {
        let (socket, _other_end) = UnixStream::pair().unwrap();
        let file = File::from(OwnedFd::from(socket));
        assert!(read_gamma(file, 2).is_err());
    }
}
