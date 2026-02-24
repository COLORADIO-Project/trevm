#![no_std]
#![feature(type_alias_impl_trait)]

use core::cell::RefCell;

use coap_handler::Record as _;
use coap_handler::{Handler, Reporting};
use coap_handler_implementations::{
    HandlerBuilder, SimpleRenderable, SimpleRendered, new_dispatcher,
};
use coap_message_implementations::inmemory::Message;
use coap_message_implementations::inmemory_write::GenericMessage;

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[global_allocator]
static ALLOCATOR: talc::Talck<talc::locking::AssumeUnlockable, talc::ClaimOnOom> = {
    static mut MEMORY: [u8; 0x4000] = [0; 0x4000]; // 16KiB of memory
    let span = talc::Span::from_array((&raw const MEMORY).cast_mut());
    talc::Talc::new(unsafe { talc::ClaimOnOom::new(span) }).lock()
};

use wit_bindgen::generate;

generate!({
    world: "example-persistent-with-bindings",
    path: "../../wit",
    generate_all,
});

use ariel::wasm_bindings::log_api::info;
use ariel::wasm_bindings::rng_api::RNG;
use ariel::wasm_bindings::sensors_api::*;

struct SendCell<T>(RefCell<T>);

/// # Safety
/// Wasm is single threaded
unsafe impl<T> Send for SendCell<T> {}
unsafe impl<T> Sync for SendCell<T> {}

use exports::ariel::wasm_bindings::coap_server_guest::{CoapErr, Guest};

struct MyComponent;

impl Guest for MyComponent {
    fn coap_run(code: u8, observed_len: u32, message: Vec<u8>) -> Result<(u8, Vec<u8>), CoapErr> {
        coap_run(code, observed_len, message)
    }

    fn initialize_handler() -> Result<(), ()> {
        initialize_handler()
    }

    fn report() -> Result<Vec<String>, CoapErr> {
        report_resource()
    }
}

type HandlerType = impl Handler + Reporting;
static HANDLER: SendCell<Option<HandlerType>> = SendCell(RefCell::new(None));

fn coap_run(
    mut code: u8,
    observed_len: u32,
    mut message: Vec<u8>,
) -> Result<(u8, Vec<u8>), CoapErr> {
    let mut handler = HANDLER.0.borrow_mut();
    if handler.is_none() {
        return Err(CoapErr::HandlerNotBuilt);
    }
    let handler = handler.as_mut().unwrap();

    let reencoded = Message::new(code, &message[..observed_len as usize]);

    let extracted = match handler.extract_request_data(&reencoded) {
        Ok(ex) => ex,
        // Assume that if it failed it's because it wasn't found
        Err(_) => {
            return Err(CoapErr::NotFound);
        }
    };
    drop(reencoded);
    message.as_mut_slice().fill(0);

    let mut response = GenericMessage::new(&mut code, message.as_mut_slice());

    match handler.build_response(&mut response, extracted) {
        Err(_) => {
            return Err(CoapErr::InternalServerError);
        }
        _ => {}
    }

    let outgoing_len = response.finish();
    message.truncate(outgoing_len);
    return Ok((code, message));
}

#[define_opaque(HandlerType)]
fn initialize_handler() -> Result<(), ()> {
    match HANDLER.0.borrow_mut() {
        mut h if h.is_none() => {
            *h = Some(build_handler());
        }
        _ => {}
    }
    Ok(())
}

struct LogTemp;

impl SimpleRenderable for LogTemp {
    fn render<W: core::fmt::Write>(&mut self, writer: &mut W) {
        write!(writer, "{}", mess_with_temperature()).unwrap()
    }
}

fn build_handler() -> impl Handler + Reporting {
    new_dispatcher().at(&["temperature_sensor"], SimpleRendered(LogTemp))
}

fn report_resource() -> Result<Vec<String>, CoapErr> {
    let mut handler = HANDLER.0.borrow_mut();
    if handler.is_none() {
        return Err(CoapErr::HandlerNotBuilt);
    }
    let handler = handler.as_mut().unwrap();

    let mut resources = Vec::new();

    for record in handler.report() {
        // intersperse with "/";
        let mut complete_path = record
            .path()
            .fold(String::new(), |a, b| a + b.as_ref() + "/");
        // remove the trailing "/";
        complete_path.truncate(complete_path.len() - 1);
        resources.push(complete_path);
    }

    Ok(resources)
}

fn mess_with_temperature() -> String {
    trigger_measurements(Some(Category::Temperature)).unwrap();
    let (sample, reading_channel) = wait_for_reading(Some(Label::Temperature))
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    match reading_channel.label {
        Label::Temperature => {}
        _ => unreachable!(),
    }
    return log_messed_with_measure(sample, reading_channel);
}

fn log_messed_with_measure(sample: Sample, reading_channel: Channel) -> String {
    let mut value = sample.value;

    // Adding a random noise up to 100
    let noise = RNG::next_u32() % 100;
    value = value + noise as i32;

    let scaling = reading_channel.scaling;

    let (integer_part, decimal_part) = if scaling < 0 {
        let int_part = value as i32 / 10_i32.pow(-scaling as u32);
        (
            int_part,
            value.unsigned_abs() - int_part.unsigned_abs() * 10_u32.pow(-scaling as u32),
        )
    } else {
        // Just multiply
        (value as i32 * 10_i32.pow(scaling as u32), 0)
    };

    match sample.metadata {
        SampleMetadata::UnknownAccuracy | SampleMetadata::NoMeasurementError => {
            let str_to_log = format!(
                "[Sensor] {}.{}{}",
                integer_part,
                decimal_part,
                reading_channel.unit.to_str()
            );
            info(&str_to_log);
            return str_to_log;
        }
        SampleMetadata::SymmetricalError((dev, bias, error_scaling)) => {
            let minus_dev = bias as i32 - dev as i32;
            let plus_dev = bias as i32 + dev as i32;
            let (m_int, m_part, p_int, p_part) = if scaling < 0 {
                let m_int = minus_dev as i32 / 10_i32.pow(-scaling as u32);
                let p_int = plus_dev as i32 / 10_i32.pow(-scaling as u32);
                (
                    m_int,
                    minus_dev.unsigned_abs() - m_int.unsigned_abs() * 10_u32.pow(-scaling as u32),
                    p_int,
                    plus_dev.unsigned_abs() - p_int.unsigned_abs() * 10_u32.pow(-scaling as u32),
                )
            } else {
                // Just multiply
                (
                    minus_dev as i32 * 10_i32.pow(scaling as u32),
                    0,
                    plus_dev as i32 * 10_i32.pow(scaling as u32),
                    0,
                )
            };

            let str_to_log = format!(
                "[Sensor] {}.{} +{}.{} -{}.{} {}",
                integer_part,
                decimal_part,
                p_int,
                p_part,
                m_int,
                m_part,
                reading_channel.unit.to_str(),
            );
            info(&str_to_log);
            return str_to_log;
        }
        _ => {
            let str_to_log = String::from("[Sensor] Error in the reading");
            info(&str_to_log);
            return str_to_log;
        }
    }
}

impl MeasurementUnit {
    fn to_str(self) -> &'static str {
        match self {
            MeasurementUnit::Celsius => "°C",
            _ => unimplemented!(),
        }
    }
}

export!(MyComponent);

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable();
}
