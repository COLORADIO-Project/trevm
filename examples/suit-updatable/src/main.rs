#![no_main]
#![no_std]
extern crate alloc;

use core::ptr::NonNull;

use alloc::boxed::Box;
use alloc::vec::Vec;
use ariel_os::coap::coap_run;
use ariel_os::debug::log::{Debug2Format, error, info, warn};

use coap_handler::Handler;
use coap_handler_implementations::{HandlerBuilder, ReportingHandlerBuilder, new_dispatcher};

use coap_message::{Code, OptionNumber};

use coap_message_utils::Error as CoapError;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embassy_futures::select::{Either, select};

use wasmtime::component::{Component, HasSelf, Linker, bindgen};
use wasmtime::{Config, Engine, Error as WasmtimeError, Store};

use ariel_os_bindings::wasm::ArielOSHost;

use crate::suit::{UpdateError, build_and_authenticate_manifest, fetch_and_verify_update};

mod coap_fetch;
mod suit;

bindgen!({
    world: "example-async",
    path: "../../wit/",
    with: {
        "ariel:wasm-bindings/log-api": ariel_os_bindings::wasm::log,
        "ariel:wasm-bindings/time-api": ariel_os_bindings::wasm::time,
        "ariel:wasm-bindings/rng-api": ariel_os_bindings::wasm::rng,

    },
    require_store_data_send: true,
});

static SUIT_VERIFY_SIGNAL: Signal<CriticalSectionRawMutex, Box<[u8]>> = Signal::new();
static VM_DROP_REQUESTS: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
static VM_STATUS_SIGNAL: Channel<CriticalSectionRawMutex, VmEvent, 1> = Channel::new();
static UPDATE_RESULTS: Channel<CriticalSectionRawMutex, Result<Vec<u8>, ()>, 1> = Channel::new();

#[derive(Debug)]
enum VmEvent {
    Dropped,
    Finished,
}

struct VmControl {
    payload: Vec<u8>,
}

impl VmControl {
    fn new() -> Self {
        Self {
            payload: Vec::new(),
        }
    }
}

impl Handler for VmControl {
    type RequestData = (Option<u32>, u8);

    type ExtractRequestError = coap_message_utils::Error;
    type BuildResponseError<M: coap_message::MinimalWritableMessage> = coap_message_utils::Error;

    fn extract_request_data<M: coap_message::ReadableMessage>(
        &mut self,
        request: &M,
    ) -> Result<Self::RequestData, Self::ExtractRequestError> {
        use coap_message::MessageOption;
        use coap_message_utils::OptionsExt;

        match request.code().into() {
            coap_numbers::code::DELETE => {
                info!("Received DELETE request for SUIT-Manifest");
                request.options().ignore_elective_others()?;

                self.payload.clear();

                Ok((None, coap_numbers::code::DELETED))
            }

            coap_numbers::code::PUT => {
                info!("Received PUT request for program ");
                let mut block1: Option<u32> = None;

                request
                    .options()
                    .filter(|o| {
                        if o.number() == coap_numbers::option::BLOCK1
                            && let Some(n) = o.value_uint()
                            && block1.is_none()
                        {
                            block1 = Some(n);
                            false
                        } else {
                            true
                        }
                    })
                    .ignore_elective_others()?;

                // This is a bit of a simplification, but ignoring the block size and just
                // appending is really kind'a fine IMO.
                let block1_value = block1.unwrap_or(0);

                // FIXME there's probably a Size1 option; if so, reallocate to fail early.

                let szx = block1_value & 0x7;
                let blocksize = 1usize << (4 + szx);
                let offset = (block1_value >> 4) as usize * blocksize;

                if offset == 0 {
                    self.payload.clear();
                }
                if self.payload.len() != offset {
                    return Ok((None, coap_numbers::code::REQUEST_ENTITY_INCOMPLETE));
                }

                let payload = request.payload();
                self.payload.try_reserve_exact(payload.len()).map_err(|e| {
                    info!(
                        "Failed to reserve memory for program: {:?}",
                        Debug2Format(&e)
                    );
                    CoapError::internal_server_error()
                })?;
                self.payload.extend_from_slice(payload);

                if (block1_value & 0x8) == 0x8 {
                    Ok((block1, coap_numbers::code::CONTINUE))
                } else {
                    let image = core::mem::take(&mut self.payload);
                    SUIT_VERIFY_SIGNAL.signal(image.into_boxed_slice());
                    Ok((block1, coap_numbers::code::CHANGED))
                }
            }

            _ => Err(CoapError::method_not_allowed()),
        }
    }

    fn estimate_length(&mut self, _request: &Self::RequestData) -> usize {
        1
    }

    fn build_response<M: coap_message::MutableWritableMessage>(
        &mut self,
        response: &mut M,
        request: Self::RequestData,
    ) -> Result<(), Self::BuildResponseError<M>> {
        let (block1, code) = request;
        response.set_code(M::Code::new(code).map_err(CoapError::from_unionerror)?);

        if let Some(block1) = block1 {
            response
                .add_option_uint(
                    M::OptionNumber::new(coap_numbers::option::BLOCK1)
                        .map_err(CoapError::from_unionerror)?,
                    block1 as u32,
                )
                .map_err(CoapError::from_unionerror)?;
        }
        Ok(())
    }
}

#[ariel_os::task(autostart)]
async fn coap_task() {
    let control = VmControl::new();

    let handler = new_dispatcher()
        .at_with_attributes(&["vm-control"], &[], control)
        .with_wkc();

    info!("Starting CoAP handler");
    coap_run(handler).await;
}

#[ariel_os::task(autostart)]
async fn suit_update_task() {
    let mut accepted_sequence_number = None;
    loop {
        let envelope = SUIT_VERIFY_SIGNAL.wait().await;
        info!("[SUIT] Received update request");

        let (manifest, sequence_number) = match build_and_authenticate_manifest(&envelope) {
            Ok(manifest) => manifest,
            Err(e) => {
                info!("[SUIT] Update rejected: {:?}", Debug2Format(&e));
                continue;
            }
        };

        if let Some(current) = accepted_sequence_number {
            if sequence_number < current {
                warn!(
                    "[SUIT] Update rejected: {:?}",
                    Debug2Format(&UpdateError::RollbackDetected {
                        current,
                        attempted: sequence_number,
                    })
                );
                continue;
            }

            if sequence_number == current {
                warn!(
                    "[SUIT] accepting repeated manifest sequence number {:?} for testing",
                    sequence_number
                );
            }
        }

        info!("[SUIT] Update authenticated. Requesting drop of old capsule...");
        VM_DROP_REQUESTS.send(()).await;
        match VM_STATUS_SIGNAL.receive().await {
            VmEvent::Dropped => {
                info!("[SUIT] Capsule dropped. Fetching new capsule...");
            }
            other => {
                info!("[SUIT] Unexpected VM event {:?}", Debug2Format(&other));
                continue;
            }
        }

        match fetch_and_verify_update(manifest).await {
            Ok(capsule) => {
                accepted_sequence_number = Some(
                    accepted_sequence_number
                        .map_or(sequence_number, |current| current.max(sequence_number)),
                );

                info!(
                    "[SUIT] Successfully fetched capsule with a length of {} bytes. Requesting install...",
                    capsule.len()
                );
                UPDATE_RESULTS.send(Ok(capsule)).await
            }
            Err(e) => {
                warn!("[SUIT] Failed to retrieve capsule: {:?}", Debug2Format(&e));
                UPDATE_RESULTS.send(Err(())).await
            }
        }
    }
}

#[ariel_os::task(autostart)]
async fn runner_task() {
    let engine = make_engine();
    let initial_capsule = include_bytes!("../payload.cwasm").as_slice();
    let mut capsule: Vec<u8> = Vec::from(initial_capsule);

    let mut linker = Linker::new(&engine);
    ExampleAsync::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).unwrap();

    let mut host = ArielOSHost::default();

    loop {
        let (returned_host, result) = run_capsule(&engine, &linker, capsule, host).await;
        match result {
            Ok(VmEvent::Dropped) => {
                info!("Capsule stopped externally");
            }
            Ok(VmEvent::Finished) => {
                info!("Capsule finished on its own");
            }
            Err(e) => {
                error!("run_capsule crashed: {:?}", Debug2Format(&e));
            }
        }

        host = returned_host;

        info!("Waiting for new capsule...");
        capsule = wait_for_capsule().await;
    }
}

fn make_engine() -> Engine {
    let mut cfg = Config::default();
    cfg.wasm_custom_page_sizes(true);
    cfg.target("pulley32").unwrap();

    // Must match precompilation
    cfg.table_lazy_init(false);
    cfg.memory_reservation(0);
    cfg.memory_init_cow(false);
    cfg.memory_may_move(false);

    // Runtime-only tuning
    cfg.max_wasm_stack(2048);
    cfg.memory_reservation_for_growth(0);
    cfg.async_stack_size(4096);

    cfg.consume_fuel(true);

    Engine::new(&cfg).unwrap()
}

async fn wait_for_capsule() -> Vec<u8> {
    loop {
        let cmd_fut = VM_DROP_REQUESTS.receive();
        let update_fut = UPDATE_RESULTS.receive();

        match select(cmd_fut, update_fut).await {
            Either::First(()) => {
                info!("No capsule loaded; acknowledging drop request");
                VM_STATUS_SIGNAL.send(VmEvent::Dropped).await;
            }
            Either::Second(Ok(capsule)) => {
                info!("Received new capsule");
                return capsule;
            }
            Either::Second(Err(())) => {
                info!("Update failed; still waiting for capsule");
            }
        }
    }
}

async fn run_capsule(
    engine: &Engine,
    linker: &Linker<ArielOSHost>,
    mut capsule: Vec<u8>,
    host: ArielOSHost,
) -> (ArielOSHost, Result<VmEvent, WasmtimeError>) {
    let component =
        match unsafe { Component::deserialize_raw(&engine, NonNull::from(capsule.as_mut())) } {
            Ok(component) => component,
            Err(e) => {
                error!("Failed to deserialize component: {:?}", Debug2Format(&e));

                return (host, Err(e));
            }
        };

    let mut store = Store::new(&engine, host);

    store.set_fuel(u64::MAX).expect("failed to set fuel");

    store
        .fuel_async_yield_interval(Some(1_000))
        .expect("failed to set fuel async yield interval");

    let bindings = match ExampleAsync::instantiate_async(&mut store, &component, linker).await {
        Ok(bindings) => bindings,
        Err(e) => {
            let host = store.into_data();
            return (host, Err(e));
        }
    };

    let run_fut = bindings.run.call_async(&mut store, &[], &mut []);
    let drop_requested_fut = VM_DROP_REQUESTS.receive();

    let result = match select(drop_requested_fut, run_fut).await {
        Either::First(_) => Ok(VmEvent::Dropped),
        Either::Second(Ok(_)) => Ok(VmEvent::Finished),
        Either::Second(Err(e)) => Err(e),
    };

    let host = store.into_data();

    if matches!(result, Ok(VmEvent::Dropped)) {
        VM_STATUS_SIGNAL.send(VmEvent::Dropped).await;
    }

    (host, result)
}
