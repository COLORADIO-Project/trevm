#![no_std]

#[global_allocator]
static ALLOCATOR: talc::Talck<talc::locking::AssumeUnlockable, talc::ClaimOnOom> = {
    static mut MEMORY: [u8; 0x4000] = [0; 0x4000]; // 16KiB of memory
    let span = talc::Span::from_array((&raw const MEMORY).cast_mut());
    talc::Talc::new(unsafe { talc::ClaimOnOom::new(span) }).lock()
};

use wit_bindgen::generate;

extern crate alloc;
use alloc::format;

generate!({
    world: "example-ephemeral-with-bindings ",
    path: "../../wit",
    generate_all,
});

use ariel::wasm_bindings::log_api::info;
use ariel::wasm_bindings::sensors_api::*;
use ariel::wasm_bindings::rng_api::RNG;

pub struct MyComponent;

impl Guest for MyComponent {
    fn mess_with_temperature() {
        trigger_measurements(Some(Category::Temperature)).unwrap();
        for (sample, reading_channel) in wait_for_reading(Some(Label::Temperature)).unwrap() {
            match reading_channel.label {
                Label::Temperature => {}
                _ => unreachable!()
            }
            log_messed_with_measure(sample, reading_channel);
        }
    }
}

fn log_messed_with_measure(sample: Sample, reading_channel: Channel) {

    let mut value = sample.value;

    // Adding a random noise up to 100
    let noise = RNG::next_u32() % 100;
    value = value+ noise as i32;

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
        SampleMetadata::UnknownAccuracy |
        SampleMetadata::NoMeasurementError => {
            info(
                &format!("[Sensor] {}.{}{}", integer_part, decimal_part, reading_channel.unit.to_str())
            )
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
                    minus_dev as i32 * 10_i32.pow(scaling as u32), 0,
                    plus_dev as i32 * 10_i32.pow(scaling as u32), 0,
                )
            };

            info(
                &format!(
                    "[Sensor] {}.{} +{}.{} -{}.{} {}",
                    integer_part,
                    decimal_part,
                    p_int,
                    p_part,
                    m_int,
                    m_part,
                    reading_channel.unit.to_str(),
                )
            )
        },
        _ => {
            info("[Sensor] Error in the reading");
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
