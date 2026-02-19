#![no_main]
#![no_std]

mod i2c_bus;
mod pins;
mod sensors;

use ariel_os::{
    debug::{
        ExitCode, exit,
        log::{error, info},
    },
    sensors::REGISTRY,
    time::Timer,
};

use wasmtime::component::{Component, HasSelf, Linker, bindgen};
use wasmtime::{Config, Engine, Store};

use ariel_os_bindings::wasm::ArielOSHost;

use ariel_os::sensors::Category;
use ariel_os::sensors::Reading;

use crate::pins::Peripherals;

bindgen!({
    world: "example-sensors",
    path: "../../wit/",
    with: {
        "ariel:wasm-bindings/log-api": ariel_os_bindings::wasm::log,
        "ariel:wasm-bindings/sensors-api": ariel_os_bindings::wasm::sensors,
        "ariel:wasm-bindings/time-api": ariel_os_bindings::wasm::time,
    },
    // Required because the example is asynchronous
    require_store_data_send: true,
});

#[ariel_os::task(autostart, peripherals)]
async fn main(peripherals: Peripherals) {
    i2c_bus::init(peripherals.i2c);
    sensors::init().await;

    let r = run_wasm().await;
    info!("{:?}", defmt::Debug2Format(&r));
    Timer::after_millis(100).await;
    exit(ExitCode::SUCCESS);
}

async fn run_wasm() -> wasmtime::Result<()> {
    let mut config = Config::new();

    // Options that must conform with the precompilation step
    config.wasm_custom_page_sizes(true);
    config.target("pulley32").unwrap();

    config.table_lazy_init(false);
    config.memory_reservation(0);
    config.memory_init_cow(false);
    config.memory_may_move(false);

    // Options that can be changed without changing the payload
    config.max_wasm_stack(2048);
    config.memory_reservation_for_growth(0);

    // Options relating to async
    config.async_stack_size(4096);

    let engine = Engine::new(&config)?;

    let component_bytes = include_bytes!("../payload.cwasm");

    let component =
        unsafe { Component::deserialize_raw(&engine, component_bytes.as_slice().into()) }?;

    let host = ArielOSHost::default();
    let mut store = Store::new(&engine, host);

    let mut linker = Linker::new(&engine);

    ExampleSensors::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
    let bindings = ExampleSensors::instantiate_async(&mut store, &component, &linker).await?;

    bindings
        .monitor_temperature
        .call_async(&mut store, &[], &mut [])
        .await
        .unwrap();
    Ok(())
}
