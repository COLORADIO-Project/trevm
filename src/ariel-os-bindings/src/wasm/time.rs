use wasmtime::component::bindgen;

use ariel_os_embassy::api::time::{Instant, Timer};

use super::ArielOSHost;

bindgen!({
    world: "ariel:wasm-bindings/time",
    path: "../../wit/",

    imports: {
        "ariel:wasm-bindings/time-api.sleep": async,
    }
});

pub use ariel::wasm_bindings::time_api::{Host, HostWithStore, add_to_linker};

impl Host for ArielOSHost {
    async fn sleep(&mut self, millis: u64) {
        Timer::after_millis(millis).await;
    }

    fn now_as_millis(&mut self) -> u64 {
        Instant::now().as_millis()
    }
}
